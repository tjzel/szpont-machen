mod install;
mod protocol;
mod tools;

pub use install::install;

use std::io::Write;

use serde_json::{Value, json};

use crate::cli::Cli;
use crate::store::Store;

const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
pub(super) const MAX_AUDIT_ROWS: i64 = 1000;

pub fn serve(cli: &Cli) -> anyhow::Result<()> {
    let mut store = Store::open(cli.db.as_deref())?;
    prune_audit_rows(&store);
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut reader = stdin.lock();
    loop {
        let capped = crate::core::read_line_capped(&mut reader, MAX_MESSAGE_BYTES)?;
        if !capped.complete && capped.bytes.is_empty() {
            break;
        }
        if capped.truncated {
            write_parse_error(&mut out, "message exceeds the size limit")?;
        } else {
            let line = String::from_utf8_lossy(&capped.bytes);
            if !line.trim().is_empty() {
                match serde_json::from_str::<Value>(&line) {
                    Ok(message) => {
                        if let Some(response) = handle_message(&mut store, &message) {
                            let text = serde_json::to_string(&response)?;
                            writeln!(out, "{text}")?;
                            out.flush()?;
                        }
                    }
                    Err(_) => write_parse_error(&mut out, "invalid JSON")?,
                }
            }
        }
        if !capped.complete {
            break;
        }
    }
    Ok(())
}

fn write_parse_error(out: &mut impl Write, detail: &str) -> anyhow::Result<()> {
    let response = json!({
        "jsonrpc": "2.0",
        "id": Value::Null,
        "error": { "code": -32700, "message": format!("parse error: {detail}") }
    });
    writeln!(out, "{}", serde_json::to_string(&response)?)?;
    out.flush()?;
    Ok(())
}

fn prune_audit_rows(store: &Store) {
    let _ = store.conn().execute(
        "DELETE FROM mcp_events WHERE id <= (
           SELECT COALESCE(MAX(id), 0) - ?1 FROM mcp_events
         )",
        rusqlite::params![MAX_AUDIT_ROWS],
    );
}

fn handle_message(store: &mut Store, message: &Value) -> Option<Value> {
    let method = message.get("method").and_then(Value::as_str)?;
    let id = message.get("id");
    id?;
    let id = id.unwrap().clone();
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => Ok(protocol::initialize_result(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools::definitions() })),
        "tools/call" => tools::call(store, &params),
        _ => Err((-32601, format!("method not found: {method}"))),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, msg)) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": msg }
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{SessionKey, ToolId};

    fn test_store(name: &str) -> Store {
        let dir = std::path::PathBuf::from(".tmp/fixtures");
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join(format!("mcp-{name}.db"));
        let _ = std::fs::remove_file(&db);
        Store::open(Some(&db)).unwrap()
    }

    fn call_tool(store: &mut Store, name: &str, arguments: &Value) -> Value {
        let message = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        });
        handle_message(store, &message).unwrap()
    }

    fn call_tool_ok(store: &mut Store, name: &str, arguments: &Value) -> String {
        let response = call_tool(store, name, arguments);
        assert!(response["error"].is_null(), "{response}");
        assert!(response["result"]["isError"].is_null(), "{response}");
        response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("{response}"))
            .to_string()
    }

    #[test]
    fn notification_without_id_returns_none() {
        let mut store = test_store("notify");
        let message = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(handle_message(&mut store, &message).is_none());
    }

    #[test]
    fn unknown_method_returns_method_not_found() {
        let mut store = test_store("unknown-method");
        let message = json!({ "jsonrpc": "2.0", "id": 7, "method": "bogus/method" });
        let response = handle_message(&mut store, &message).unwrap();
        assert_eq!(response["error"]["code"], -32601);
        assert_eq!(response["id"], 7);
    }

    #[test]
    fn tools_list_reports_the_exact_tool_set() {
        let mut store = test_store("tools-list");
        let message = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
        let response = handle_message(&mut store, &message).unwrap();
        let tools = response["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(
            names,
            vec![
                "report_session_start",
                "mark_session_completed",
                "reopen_session"
            ]
        );
        for tool in tools {
            let schema = &tool["inputSchema"];
            assert!(schema.is_object());
            assert_ne!(*schema["required"].as_array().unwrap(), Vec::<Value>::new());
            assert_eq!(schema["additionalProperties"], false);
        }
    }

    #[test]
    fn initialize_negotiates_a_supported_protocol_version() {
        let supported = protocol::initialize_result(&json!({ "protocolVersion": "2024-11-05" }));
        assert_eq!(supported["protocolVersion"], "2024-11-05");
        let unsupported = protocol::initialize_result(&json!({ "protocolVersion": "1999-01-01" }));
        assert_eq!(unsupported["protocolVersion"], "2025-06-18");
        let missing = protocol::initialize_result(&Value::Null);
        assert_eq!(missing["protocolVersion"], "2025-06-18");
    }

    #[test]
    fn initialize_documents_every_advertised_tool() {
        let result = protocol::initialize_result(&Value::Null);
        let instructions = result["instructions"].as_str().unwrap();
        assert_ne!(instructions.trim(), "");
        for tool in tools::definitions() {
            let name = tool["name"].as_str().unwrap();
            assert!(
                instructions.contains(name),
                "{name} is advertised but not covered by the instructions"
            );
        }
    }

    #[test]
    fn report_session_start_does_not_overwrite_a_stored_cwd() {
        let mut store = test_store("report-cwd");
        crate::store::upsert_session_facts(
            store.conn(),
            ToolId::Claude,
            "sess-cwd",
            Some("/stored/path"),
            None,
            None,
            None,
            None,
            None,
            "scan",
        )
        .unwrap();
        let text = call_tool_ok(
            &mut store,
            "report_session_start",
            &json!({ "tool": "claude", "cwd": "/other/path", "session_id": "sess-cwd", "title": "task\u{1b}[31m" }),
        );
        assert!(text.contains("tracking"), "{text}");
        let meta = store.session_meta_map().unwrap();
        let stored = meta
            .get(&SessionKey {
                tool: ToolId::Claude,
                id: "sess-cwd".to_string(),
            })
            .unwrap();
        assert_eq!(stored.cwd.as_deref(), Some("/stored/path"));
        assert_eq!(stored.title.as_deref(), Some("task·[31m"));
    }

    #[test]
    fn audit_rows_store_only_whitelisted_fields() {
        let mut store = test_store("audit-whitelist");
        call_tool_ok(
            &mut store,
            "mark_session_completed",
            &json!({
                "tool": "claude",
                "session_id": "sess-audit",
                "reason": "done",
                "smuggled": "a".repeat(3000)
            }),
        );
        let payload: String = store
            .conn()
            .query_row(
                "SELECT payload_json FROM mcp_events WHERE session_id = 'sess-audit'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!payload.contains("smuggled"), "{payload}");
        let parsed: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(parsed["reason"], "done");
        assert_eq!(parsed["session_id"], "sess-audit");
    }

    #[test]
    fn tools_call_with_unknown_name_is_invalid_params() {
        let mut store = test_store("unknown-tool");
        let response = call_tool(&mut store, "does_not_exist", &json!({}));
        assert_eq!(response["error"]["code"], -32602);
    }

    #[test]
    fn mark_and_reopen_round_trip_through_the_store() {
        let mut store = test_store("round-trip");
        let key = SessionKey {
            tool: ToolId::Claude,
            id: "sess-1".to_string(),
        };
        let text = call_tool_ok(
            &mut store,
            "mark_session_completed",
            &json!({ "tool": "claude", "session_id": "sess-1" }),
        );
        assert!(text.contains("marked as completed"), "{text}");
        let meta = store.session_meta_map().unwrap();
        assert!(meta.get(&key).unwrap().completed_at.is_some());
        let text = call_tool_ok(
            &mut store,
            "reopen_session",
            &json!({ "tool": "claude", "session_id": "sess-1" }),
        );
        assert!(text.contains("reopened"), "{text}");
        let meta = store.session_meta_map().unwrap();
        assert!(meta.get(&key).unwrap().completed_at.is_none());
    }

    #[test]
    fn malformed_session_id_is_rejected() {
        let mut store = test_store("bad-id");
        let response = call_tool(
            &mut store,
            "reopen_session",
            &json!({ "tool": "claude", "session_id": "$(rm -rf /)" }),
        );
        assert_eq!(response["result"]["isError"], true);
        let meta = store.session_meta_map().unwrap();
        assert!(meta.is_empty());
    }
}
