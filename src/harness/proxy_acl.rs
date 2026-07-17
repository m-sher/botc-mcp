//! Tool allowlists for `botc-agent-mcp` proxies.

/// JSON-RPC error code for role/policy denials (`tools/call` exists; params invalid).
/// Do **not** use `-32601` (Method not found) — strict clients may disable all tools.
pub const ACL_DENY_JSONRPC_CODE: i64 = -32602;

/// Tools only the Storyteller proxy may call.
pub const HOST_ONLY: &[&str] = &[
    "create_game",
    "start_game",
    "get_host_state",
    "host_decide",
    "host_queue_lie",
    "skip_night_action",
    "open_nominations",
    "close_vote",
    "end_nominations",
    "st_announce",
];

/// Tools nobody should call through an agent proxy (table already created by TUI).
pub const DENY_ALL: &[&str] = &["create_game"];

/// Whether `name` is available to a host (`is_host`) or player proxy.
pub fn tool_allowed(name: &str, is_host: bool) -> bool {
    if DENY_ALL.contains(&name) {
        return false;
    }
    if !is_host && HOST_ONLY.contains(&name) {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_denied_host_tools() {
        assert!(!tool_allowed("get_host_state", false));
        assert!(!tool_allowed("host_decide", false));
        assert!(!tool_allowed("create_game", false));
        assert!(tool_allowed("say", false));
        assert!(tool_allowed("get_private_state", false));
        assert!(tool_allowed("nominate", false));
    }

    #[test]
    fn host_allowed_host_tools_but_not_create() {
        assert!(tool_allowed("get_host_state", true));
        assert!(tool_allowed("host_decide", true));
        assert!(!tool_allowed("create_game", true));
        assert!(tool_allowed("say", true));
        assert!(tool_allowed("skip_night_action", true));
    }

    #[test]
    fn acl_deny_code_is_invalid_params_not_method_not_found() {
        assert_eq!(ACL_DENY_JSONRPC_CODE, -32602);
        assert_ne!(ACL_DENY_JSONRPC_CODE, -32601);
    }
}

// Bare unknown method must not pass a "known tool" gate (mirrors proxy other-arm).
#[cfg(test)]
mod bare_method_gate_tests {
    use crate::mcp_server;

    #[test]
    fn unknown_bare_names_are_not_known_tools() {
        assert!(!mcp_server::is_known_tool("foobar"));
        assert!(!mcp_server::is_known_tool("nominatee"));
        assert!(!mcp_server::is_known_tool("resources/list"));
        assert!(mcp_server::is_known_tool("nominate"));
        assert!(mcp_server::is_known_tool("say"));
    }
}
