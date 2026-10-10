//! `roomy mcp`: a Model Context Protocol server over stdio (newline-delimited JSON-RPC 2.0).
//!
//! The tools return the same JSON as the CLI's `--json` (see `api`). Removal goes through the
//! unattended policy: "review" candidates need `allow_review`, and the total size is capped.
//! stdout carries only protocol messages; diagnostics go to stderr.

use crate::api;
use crate::clean::{self, Policy};
use crate::config::Config;
use crate::model::Report;
use crate::state;
use crate::sys;
use serde_json::{Value, json};
use std::io::{BufRead, Write};

/// Newest first. We answer with the client's version when we know it.
const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "roomy finds what is safe to delete on this Mac and removes it with \
safety checks. Start with roomy_status or roomy_report (roomy_scan if the report is old), explain \
candidates to the user with their `why` and `notes`, call roomy_plan_clean, and call roomy_clean \
only after the user approves. Never delete files any other way. Candidates with safety \"review\" \
need the user's explicit approval and allow_review=true.";

pub fn serve(cfg: &Config, read_only: bool) -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle_line(cfg, read_only, &line) {
            writeln!(stdout, "{reply}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// One incoming message to at most one reply (notifications get none).
pub fn handle_line(cfg: &Config, read_only: bool, line: &str) -> Option<Value> {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Some(rpc_error(
                &Value::Null,
                -32700,
                &format!("parse error: {e}"),
            ));
        }
    };
    let id = msg.get("id").cloned();
    let method = msg["method"].as_str().unwrap_or_default();
    let id = id?; // notifications (no id) need no reply
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => Ok(initialize(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools(read_only) })),
        "tools/call" => Ok(call(cfg, read_only, &params)),
        _ => Err((-32601, format!("method not found: {method}"))),
    };
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, m)) => rpc_error(&id, code, &m),
    })
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn initialize(params: &Value) -> Value {
    let asked = params["protocolVersion"].as_str().unwrap_or_default();
    let version = PROTOCOL_VERSIONS
        .iter()
        .find(|v| **v == asked)
        .copied()
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": crate::app::NAME, "title": "roomy", "version": crate::app::VERSION },
        "instructions": INSTRUCTIONS,
    })
}

fn tool(
    name: &str,
    title: &str,
    description: &str,
    schema: &Value,
    read_only: bool,
    destructive: bool,
) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": schema,
        "annotations": {
            "readOnlyHint": read_only,
            "destructiveHint": destructive,
            "idempotentHint": read_only,
            "openWorldHint": false,
        },
    })
}

fn tools(read_only_server: bool) -> Vec<Value> {
    let none = json!({ "type": "object", "properties": {} });
    let ids = json!({
        "type": "object",
        "properties": {
            "ids": { "type": "array", "items": { "type": "string" }, "description": "Candidate ids (or numbers) from roomy_report" },
            "all_safe": { "type": "boolean", "description": "Also every candidate marked safe" },
            "allow_review": { "type": "boolean", "description": "Include 'review' candidates. Only after the user explicitly approved them." },
        },
    });
    let mut v = vec![
        tool(
            "roomy_status",
            "Status",
            "Free disk space (and Finder's available space incl. purgeable), memory pressure, swap, top memory users and a summary of the last scan.",
            &none,
            true,
            false,
        ),
        tool(
            "roomy_report",
            "Last report",
            "The last scan's removal candidates (id, size, what removal really frees, safety, reasons, action) and folders that grew. Instant.",
            &none,
            true,
            false,
        ),
        tool(
            "roomy_scan",
            "Scan",
            "Walk the disk now (about 30 seconds) and return a fresh report. Use when the report is old or missing.",
            &none,
            false,
            false,
        ),
        tool(
            "roomy_explain",
            "Explain candidate",
            "Details of one candidate: the paths it would remove, the command it would run, caveats.",
            &json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Candidate id or number" },
                    "max_paths": { "type": "integer", "description": "How many paths to list (default 20)" },
                },
                "required": ["id"],
            }),
            true,
            false,
        ),
        tool(
            "roomy_top",
            "Largest folders",
            "Largest folders inside a folder, from the last scan (no disk walk), with growth since the scan before.",
            &json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Folder, e.g. ~/Library (default: home)" },
                    "n": { "type": "integer", "description": "How many (default 15)" },
                },
            }),
            true,
            false,
        ),
        tool(
            "roomy_memory",
            "Memory and swap",
            "Swap in use, space used outside the home folder (swap files, simulator runtimes), and the apps holding the most compressed or swapped memory. Read-only: suggest quitting apps to the user; they quit them with `roomy memory quit`.",
            &none,
            true,
            false,
        ),
        tool(
            "roomy_history",
            "Free space history",
            "Lowest free space per day for the last 14 days.",
            &none,
            true,
            false,
        ),
        tool(
            "roomy_doctor",
            "Doctor",
            "Permissions, available tools, background service and configuration checks.",
            &none,
            true,
            false,
        ),
        tool(
            "roomy_plan_clean",
            "Plan removal",
            "Re-check candidates and show exactly what roomy_clean would do, and what it would refuse. Removes nothing.",
            &ids,
            true,
            false,
        ),
    ];
    if !read_only_server {
        v.push(tool(
            "roomy_clean",
            "Remove candidates",
            "Remove candidates after re-checking each one. Call only after the user approved this exact list. Logged to the audit log.",
            &ids,
            false,
            true,
        ));
    }
    v
}

fn call(cfg: &Config, read_only: bool, params: &Value) -> Value {
    let name = params["name"].as_str().unwrap_or_default();
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    match run_tool(cfg, read_only, name, &args) {
        Ok(v) => json!({
            "content": [{ "type": "text", "text": v.to_string() }],
            "structuredContent": v,
            "isError": false,
        }),
        Err(e) => json!({
            "content": [{ "type": "text", "text": e }],
            "isError": true,
        }),
    }
}

fn last_report() -> Result<Report, String> {
    state::load::<Report>(&state::report_path())
        .ok_or_else(|| "no scan yet; call roomy_scan".to_string())
}

fn run_tool(cfg: &Config, read_only: bool, name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "roomy_status" => Ok(api::status(cfg).0),
        "roomy_report" => last_report().map(|r| api::summary(&r)),
        "roomy_scan" => {
            let _lock = sys::try_lock(&state::lock_path())
                .ok_or("another roomy scan or clean is running")?;
            // Like the background service: no TCC dialogs on behalf of the host app.
            crate::scan::run(cfg, true)
                .map(|r| api::summary(&r))
                .map_err(|e| e.to_string())
        }
        "roomy_explain" => {
            let id = args["id"]
                .as_str()
                .map(str::to_string)
                .or_else(|| args["id"].as_u64().map(|n| n.to_string()));
            let id = id.ok_or("missing id")?;
            let max = args["max_paths"].as_u64().unwrap_or(20) as usize;
            api::explain(&last_report()?, &id, max)
        }
        "roomy_top" => {
            let n = args["n"].as_u64().unwrap_or(15) as usize;
            api::top_from_state(args["path"].as_str(), n)
        }
        "roomy_memory" => Ok(api::memory(
            &crate::memory::holders(15),
            &crate::memory::outside_space(),
        )),
        "roomy_history" => Ok(api::history(&state::read_lines(&state::history_path()), 14)),
        "roomy_doctor" => Ok(crate::doctor::json().0),
        "roomy_plan_clean" | "roomy_clean" => {
            let execute = name == "roomy_clean";
            if execute && read_only {
                return Err("this server is read-only".into());
            }
            let ids: Vec<String> = args["ids"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| {
                            v.as_str()
                                .map(str::to_string)
                                .or_else(|| v.as_u64().map(|n| n.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let all_safe = args["all_safe"].as_bool().unwrap_or(false);
            let policy = Policy {
                unattended: true,
                allow_review: args["allow_review"].as_bool().unwrap_or(false),
            };
            let _lock = sys::try_lock(&state::lock_path())
                .ok_or("another roomy scan or clean is running")?;
            let mut report = last_report()?;
            let picked = clean::select(&report, &ids, all_safe)?;
            if picked.is_empty() {
                return Err("nothing selected: pass ids from roomy_report or all_safe=true".into());
            }
            let plan = clean::prepare(cfg, &report, &picked, policy);
            if !execute || plan.items.is_empty() {
                return Ok(api::plan(&plan));
            }
            let out =
                clean::apply(&mut report, &plan, false, true, true).map_err(|e| e.to_string())?;
            Ok(api::outcome(&plan, &out))
        }
        _ => Err(format!("unknown tool: {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle(line: &str) -> Value {
        handle_line(&Config::default(), false, line).expect("a reply")
    }

    #[test]
    fn initialize_negotiates_the_version() {
        let r = handle(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
        );
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(r["result"]["serverInfo"]["name"], "roomy");
        let r = handle(
            r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
        );
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
    }

    #[test]
    fn notifications_get_no_reply() {
        assert!(
            handle_line(
                &Config::default(),
                false,
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
            )
            .is_none()
        );
    }

    #[test]
    fn errors_follow_json_rpc() {
        assert_eq!(handle("not json")["error"]["code"], -32700);
        assert_eq!(
            handle(r#"{"jsonrpc":"2.0","id":3,"method":"nope"}"#)["error"]["code"],
            -32601
        );
    }

    #[test]
    fn read_only_server_has_no_clean_tool() {
        let names = |ro: bool| -> Vec<String> {
            tools(ro)
                .iter()
                .map(|t| t["name"].as_str().unwrap_or_default().to_string())
                .collect()
        };
        assert!(names(false).contains(&"roomy_clean".to_string()));
        assert!(!names(true).contains(&"roomy_clean".to_string()));
        let r = handle_line(&Config::default(), true, r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"roomy_clean","arguments":{"ids":["x"]}}}"#).expect("reply");
        assert_eq!(r["result"]["isError"], true);
    }

    #[test]
    fn every_tool_has_an_object_schema_and_annotations() {
        for t in tools(false) {
            assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
            assert!(t["annotations"]["destructiveHint"].is_boolean());
        }
    }
}
