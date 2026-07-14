//! Unix-socket JSON-line RPC so many MCP proxies share one engine process.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::Actor;
use crate::game::GameId;
use crate::harness::action_log::ActionLog;
use crate::harness::wake::{WakeActor, WakeCoordinator, AWAIT_SERVER_BUDGET_SECS};
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

/// Identity of the socket file we bound, so cleanup never unlinks a rebound peer's path (#55).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SockId {
    dev: u64,
    ino: u64,
}

fn sock_id_of(path: &Path) -> Option<SockId> {
    let m = std::fs::metadata(path).ok()?;
    Some(SockId {
        dev: m.dev(),
        ino: m.ino(),
    })
}

fn remove_if_ours(path: &Path, id: SockId) {
    if sock_id_of(path) == Some(id) {
        let _ = std::fs::remove_file(path);
    }
}

/// Background accept-loop serving tool RPCs on a Unix domain socket.
pub struct SocketServer {
    path: PathBuf,
    /// Bound socket inode; only remove path if it still matches (#55).
    sock_id: SockId,
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

impl SocketServer {
    /// Start with a throwaway action log (tests / callers that don't monitor).
    pub fn start(store: SharedStore, path: impl Into<PathBuf>) -> std::io::Result<Self> {
        Self::start_with_log(
            store,
            Arc::new(WakeCoordinator::new()),
            Arc::new(ActionLog::default()),
            path,
        )
    }

    /// Start serving, recording every dispatched tool RPC into `action_log`.
    pub fn start_with_log(
        store: SharedStore,
        wake: Arc<WakeCoordinator>,
        action_log: Arc<ActionLog>,
        path: impl Into<PathBuf>,
    ) -> std::io::Result<Self> {
        let path = path.into();
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let listener = UnixListener::bind(&path)?;
        let sock_id = sock_id_of(&path)
            .ok_or_else(|| std::io::Error::other("socket metadata missing after bind"))?;
        // Non-blocking accept so stop()/Drop never deadlocks if the socket file
        // is unlinked or rebound (#48). We poll with a short sleep + stop flag.
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop_c = Arc::clone(&stop);
        let path_c = path.clone();
        let join = thread::spawn(move || {
            while !stop_c.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let store = Arc::clone(&store);
                        let wake = Arc::clone(&wake);
                        let action_log = Arc::clone(&action_log);
                        thread::spawn(move || {
                            if let Err(e) = handle_client(store, wake, action_log, stream) {
                                eprintln!("botc harness client error: {e}");
                            }
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(50));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                        continue;
                    }
                    Err(e) => {
                        if stop_c.load(Ordering::SeqCst) {
                            break;
                        }
                        eprintln!("botc harness accept error: {e}");
                        thread::sleep(Duration::from_millis(50));
                    }
                }
            }
            // #55: only unlink if this path is still *our* socket inode.
            remove_if_ours(&path_c, sock_id);
        });
        Ok(Self {
            path,
            sock_id,
            stop,
            join: Some(join),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = UnixStream::connect(&self.path);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        // Accept thread already tried remove_if_ours; Drop path needs it too if join raced.
        remove_if_ours(&self.path, self.sock_id);
    }
}

impl Drop for SocketServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = UnixStream::connect(&self.path);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        remove_if_ours(&self.path, self.sock_id);
    }
}

fn handle_client(
    store: SharedStore,
    wake: Arc<WakeCoordinator>,
    action_log: Arc<ActionLog>,
    stream: UnixStream,
) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
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
        let resp = dispatch(&store, &wake, &action_log, req);
        writeln!(writer, "{}", serde_json::to_string(&resp).unwrap())?;
        writer.flush()?;
    }
    Ok(())
}

fn dispatch(
    store: &SharedStore,
    wake: &WakeCoordinator,
    action_log: &ActionLog,
    req: RpcRequest,
) -> RpcResponse {
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
            let token = req
                .arguments
                .get("token")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let outcome = if name == "await_turn" {
                invoke_await_turn(store, wake, &req.arguments)
            } else {
                let r = mcp_server::invoke_named_tool(store, name, req.arguments.clone());
                if r.is_ok() {
                    if let Some(ref tok) = token {
                        if let Some(actor) = resolve_wake_actor(store, tok, &req.arguments) {
                            wake.note_tool_success(actor, name);
                        }
                    }
                }
                r
            };
            let (ok, err, result_preview) = match &outcome {
                Ok(v) => (true, None, Some(v.to_string())),
                Err(e) => (false, Some(e.clone()), None),
            };
            action_log.record_rpc(
                token.as_deref(),
                name,
                &req.arguments,
                ok,
                err,
                result_preview,
            );
            match outcome {
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

fn resolve_wake_actor(store: &SharedStore, token: &str, args: &Value) -> Option<WakeActor> {
    use crate::auth::Token;
    let game_id = args.get("game_id").and_then(|v| v.as_u64())?;
    let st = store.lock().ok()?;
    let game = st.get(GameId(game_id))?;
    let actor = game.tokens.resolve(&Token::from_shared(token))?;
    Some(WakeActor::from_auth(actor))
}

fn invoke_await_turn(
    store: &SharedStore,
    wake: &WakeCoordinator,
    args: &Value,
) -> Result<Value, String> {
    use crate::auth::Token;
    let game_id = args
        .get("game_id")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "await_turn requires game_id".to_string())?;
    let token = args
        .get("token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "await_turn requires token".to_string())?;
    let budget_secs = args
        .get("budget_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(AWAIT_SERVER_BUDGET_SECS)
        .min(AWAIT_SERVER_BUDGET_SECS);

    let (wake_actor, display_name) = {
        let st = store
            .lock()
            .map_err(|_| "store lock poisoned".to_string())?;
        let game = st
            .get(GameId(game_id))
            .ok_or_else(|| format!("unknown game_id {game_id}"))?;
        let actor = game
            .tokens
            .resolve(&Token::from_shared(token))
            .ok_or_else(|| "unauthorized".to_string())?;
        let wa = WakeActor::from_auth(actor);
        let name = match actor {
            Actor::Host => "Host".to_string(),
            Actor::Player { seat } => game
                .seats
                .iter()
                .find(|s| s.id == seat)
                .map(|s| s.display_name.clone())
                .unwrap_or_else(|| format!("P{}", seat.0)),
        };
        (wa, name)
    };

    // Bind only when unbound. A mismatched id against an already-bound game must
    // not wipe rotation/outstanding for the live table (stale/hallucinated game_id).
    match wake.game_id() {
        None => wake.set_game_id(game_id),
        Some(bound) if bound == game_id => {}
        Some(bound) => {
            return Err(format!(
                "await_turn game_id={game_id} does not match bound game_id={bound}"
            ));
        }
    }

    let structured = wake.await_turn(
        store,
        wake_actor,
        &display_name,
        Duration::from_secs(budget_secs.max(1)),
    );
    // MCP-shaped envelope (same as other tools).
    let text = structured.to_string();
    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": structured,
        "isError": false
    }))
}

/// Client used by `botc-agent-mcp` to call the harness.
pub struct SocketClient {
    stream: UnixStream,
    next_id: u64,
}

impl SocketClient {
    pub fn connect(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(Duration::from_secs(120)))?;
        stream.set_write_timeout(Some(Duration::from_secs(30)))?;
        Ok(Self { stream, next_id: 1 })
    }

    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, String> {
        let read_timeout = if name == "await_turn" {
            // Server budget + skew so soft `idle` returns before the socket times out.
            Duration::from_secs(AWAIT_SERVER_BUDGET_SECS + 60)
        } else {
            Duration::from_secs(120)
        };
        self.stream
            .set_read_timeout(Some(read_timeout))
            .map_err(|e| e.to_string())?;
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let req = RpcRequest {
            id,
            op: "tool".into(),
            name: Some(name.into()),
            arguments,
        };
        let mut stream = &self.stream;
        writeln!(
            stream,
            "{}",
            serde_json::to_string(&req).map_err(|e| e.to_string())?
        )
        .map_err(|e| e.to_string())?;
        stream.flush().map_err(|e| e.to_string())?;
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|e| e.to_string())?;
        let _ = self.stream.set_read_timeout(Some(Duration::from_secs(120)));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_server;
    use std::time::Instant;

    #[test]
    fn stop_after_socket_unlinked_does_not_deadlock() {
        let store = mcp_server::new_shared_store();
        let path =
            std::env::temp_dir().join(format!("botc-sock-unlink-{}.sock", uuid::Uuid::new_v4()));
        let server = SocketServer::start(store, &path).expect("bind");
        assert!(path.exists());
        std::fs::remove_file(&path).expect("unlink");
        let start = Instant::now();
        server.stop();
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "stop() hung after socket unlink"
        );
    }

    /// #55: dropping an older server must not unlink a rebound peer's live socket.
    #[test]
    fn drop_old_server_does_not_unlink_rebound_socket() {
        let store_a = mcp_server::new_shared_store();
        let store_b = mcp_server::new_shared_store();
        let path =
            std::env::temp_dir().join(format!("botc-sock-rebind-{}.sock", uuid::Uuid::new_v4()));
        let a = SocketServer::start(store_a, &path).expect("bind a");
        let b = SocketServer::start(store_b, &path).expect("bind b (rebind)");
        assert!(path.exists());
        // Drop A: must not remove B's live socket path.
        drop(a);
        assert!(
            path.exists(),
            "rebound socket path was unlinked by old server Drop (#55)"
        );
        let client = SocketClient::connect(&path);
        assert!(
            client.is_ok(),
            "connect to rebound socket failed after drop(A): {:?}",
            client.err()
        );
        drop(b);
    }
}
