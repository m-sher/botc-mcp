//! Unix-socket JSON-line RPC so many MCP proxies share one engine process.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::mcp_server::{self, SharedStore};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    pub id: u64,
    pub op: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    pub id: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Background accept-loop serving tool RPCs on a Unix domain socket.
pub struct SocketServer {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

impl SocketServer {
    pub fn start(store: SharedStore, path: impl Into<PathBuf>) -> std::io::Result<Self> {
        let path = path.into();
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(false)?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop_c = Arc::clone(&stop);
        let path_c = path.clone();
        let join = thread::spawn(move || {
            while !stop_c.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let store = Arc::clone(&store);
                        thread::spawn(move || {
                            if let Err(e) = handle_client(store, stream) {
                                eprintln!("botc harness client error: {e}");
                            }
                        });
                    }
                    Err(e) => {
                        if stop_c.load(Ordering::SeqCst) {
                            break;
                        }
                        eprintln!("botc harness accept error: {e}");
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                }
            }
            let _ = std::fs::remove_file(path_c);
        });
        Ok(Self {
            path,
            stop,
            join: Some(join),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Nudge accept by connecting once.
        let _ = UnixStream::connect(&self.path);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for SocketServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = UnixStream::connect(&self.path);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

fn handle_client(store: SharedStore, stream: UnixStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let req: RpcRequest = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                let resp = RpcResponse {
                    id: 0,
                    ok: false,
                    result: None,
                    error: Some(format!("bad request: {e}")),
                };
                writeln!(writer, "{}", serde_json::to_string(&resp).unwrap())?;
                writer.flush()?;
                continue;
            }
        };
        let resp = dispatch(&store, req);
        writeln!(writer, "{}", serde_json::to_string(&resp).unwrap())?;
        writer.flush()?;
    }
    Ok(())
}

fn dispatch(store: &SharedStore, req: RpcRequest) -> RpcResponse {
    match req.op.as_str() {
        "ping" => RpcResponse {
            id: req.id,
            ok: true,
            result: Some(json!({"pong": true})),
            error: None,
        },
        "tool" => {
            let name = match req.name.as_deref() {
                Some(n) => n,
                None => {
                    return RpcResponse {
                        id: req.id,
                        ok: false,
                        result: None,
                        error: Some("tool requires name".into()),
                    };
                }
            };
            match mcp_server::invoke_named_tool(store, name, req.arguments) {
                Ok(result) => RpcResponse {
                    id: req.id,
                    ok: true,
                    result: Some(result),
                    error: None,
                },
                Err(e) => RpcResponse {
                    id: req.id,
                    ok: false,
                    result: None,
                    error: Some(e),
                },
            }
        }
        other => RpcResponse {
            id: req.id,
            ok: false,
            result: None,
            error: Some(format!("unknown op: {other}")),
        },
    }
}

/// Client used by `botc-agent-mcp` to call the harness.
pub struct SocketClient {
    stream: UnixStream,
    next_id: u64,
}

impl SocketClient {
    pub fn connect(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(120)))?;
        stream.set_write_timeout(Some(std::time::Duration::from_secs(30)))?;
        Ok(Self {
            stream,
            next_id: 1,
        })
    }

    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let req = RpcRequest {
            id,
            op: "tool".into(),
            name: Some(name.into()),
            arguments,
        };
        let mut stream = &self.stream;
        writeln!(stream, "{}", serde_json::to_string(&req).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        stream.flush().map_err(|e| e.to_string())?;
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        let resp: RpcResponse =
            serde_json::from_str(line.trim()).map_err(|e| format!("bad response: {e}"))?;
        if resp.id != id {
            return Err(format!("id mismatch: expected {id}, got {}", resp.id));
        }
        if resp.ok {
            Ok(resp.result.unwrap_or(Value::Null))
        } else {
            Err(resp.error.unwrap_or_else(|| "unknown error".into()))
        }
    }
}
