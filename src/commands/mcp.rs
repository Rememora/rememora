//! `rememora mcp`: a stdio Model Context Protocol server.
//!
//! Newline-delimited JSON-RPC 2.0 over stdin/stdout, hand-rolled on
//! serde_json so no new dependency is needed. Every tool runs the rememora
//! CLI itself as a subprocess (`current_exe()` plus the matching subcommand),
//! so CLI and MCP share one behaviour and pick up fixes to those commands for
//! free. Arguments are always built as argv vectors, never shell strings.
//!
//! stdout carries protocol messages only. Anything diagnostic goes to stderr.
//!
//! Tool results are written for the model that reads them: `memory_context`,
//! `memory_search` and `memory_get` return the CLI's markdown or compact text,
//! a fraction of the tokens of pretty-printed JSON. Only the tools whose ids
//! the caller must parse (`memory_save`, `memory_supersede`, `session_start`,
//! `session_end`) return JSON.

use anyhow::Result;
use std::cell::RefCell;
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::process::Command;

/// Protocol revisions this server speaks, newest first. `initialize` echoes
/// the client's version when it is in this list, else answers with the first.
///
/// 2026-07-28 is the current spec revision (modelcontextprotocol.io/specification/latest,
/// checked 2026-11-17) but it replaces the `initialize` handshake with
/// `server/discover`, so it is deliberately not listed: 2025-11-25 is the
/// newest revision with the lifecycle implemented here.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] =
    &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

const CATEGORIES: &[&str] = &["preference", "entity", "decision", "event", "case", "pattern"];

pub fn run(no_encryption: bool) -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let server = Server::new(no_encryption);
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = server.handle_line(&line) {
            let mut out = stdout.lock();
            writeln!(out, "{}", serde_json::to_string(&reply)?)?;
            out.flush()?;
        }
    }
    Ok(())
}

pub struct Server {
    pub no_encryption: bool,
    /// `clientInfo.name` from `initialize`, the default `--agent` for saves.
    client_name: RefCell<Option<String>>,
}

/// Source agent recorded for `memory_save` when neither the call nor the
/// client's `initialize` names one.
pub const DEFAULT_AGENT: &str = "mcp";

fn error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message.into() } })
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

impl Server {
    pub fn new(no_encryption: bool) -> Self {
        Server { no_encryption, client_name: RefCell::new(None) }
    }

    /// The client's name from `initialize`, if it sent one.
    pub fn client_name(&self) -> Option<String> {
        self.client_name.borrow().clone()
    }

    /// Handle one line of input. Returns `None` for notifications.
    pub fn handle_line(&self, line: &str) -> Option<Value> {
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => return Some(error(Value::Null, PARSE_ERROR, format!("Parse error: {e}"))),
        };
        let Some(obj) = msg.as_object() else {
            return Some(error(Value::Null, INVALID_REQUEST, "Expected a JSON-RPC object"));
        };
        let id = obj.get("id").cloned();
        let Some(method) = obj.get("method").and_then(|m| m.as_str()) else {
            // A response from the client, or junk. Nothing to answer.
            return id.map(|id| error(id, INVALID_REQUEST, "Missing method"));
        };
        let params = obj.get("params").cloned().unwrap_or(Value::Null);

        let Some(id) = id else {
            // Notifications (`notifications/initialized`, cancellations) need no reply.
            if method != "notifications/initialized" {
                eprintln!("rememora mcp: ignoring notification {method}");
            }
            return None;
        };

        Some(match method {
            "initialize" => {
                let name = params
                    .pointer("/clientInfo/name")
                    .and_then(|n| n.as_str())
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .map(String::from);
                *self.client_name.borrow_mut() = name;
                ok(id, initialize_result(&params))
            }
            "ping" => ok(id, json!({})),
            "tools/list" => ok(id, json!({ "tools": tool_definitions() })),
            "tools/call" => self.call_tool(id, &params),
            other => error(id, METHOD_NOT_FOUND, format!("Method not found: {other}")),
        })
    }

    fn call_tool(&self, id: Value, params: &Value) -> Value {
        let Some(name) = params.get("name").and_then(|n| n.as_str()) else {
            return error(id, INVALID_PARAMS, "tools/call needs a tool name");
        };
        let empty = json!({});
        let args = params.get("arguments").unwrap_or(&empty);
        let agent = self.client_name();
        let argv = match build_argv_for(name, args, agent.as_deref().unwrap_or(DEFAULT_AGENT)) {
            Ok(Some(a)) => a,
            Ok(None) => return error(id, INVALID_PARAMS, format!("Unknown tool: {name}")),
            Err(msg) => return ok(id, tool_result(msg, true)),
        };
        ok(id, self.exec(argv))
    }

    fn exec(&self, mut argv: Vec<String>) -> Value {
        if self.no_encryption {
            argv.push("--no-encryption".into());
        }
        let exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => return tool_result(format!("Cannot locate rememora binary: {e}"), true),
        };
        match Command::new(exe).args(&argv).stdin(std::process::Stdio::null()).output() {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
                if out.status.success() {
                    tool_result(stdout, false)
                } else {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    tool_result(format!("{stdout}{stderr}").trim().to_string(), true)
                }
            }
            Err(e) => tool_result(format!("Failed to run rememora: {e}"), true),
        }
    }
}

fn tool_result(text: String, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

fn initialize_result(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("");
    let version = SUPPORTED_PROTOCOL_VERSIONS
        .iter()
        .find(|v| **v == requested)
        .copied()
        .unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "rememora", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "Rememora is a memory database shared by every coding agent on this machine. \
Call memory_context at the start of work, memory_search before non-trivial changes, and memory_save \
for decisions, root causes, user corrections and quirks as they happen, one fact per memory."
    })
}

pub fn tool_definitions() -> Value {
    let project = json!({ "type": "string", "description": "Project name, usually the repository name. Omit for global scope." });
    json!([
        {
            "name": "memory_context",
            "description": "Load the memories for a project as markdown: the last handoff, active sessions, memories relevant to your task, recent and top memories. Each memory line ends with its id for memory_get. Call once at the start of work, with query set to your task.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Project name. Omit to detect it from the server's working directory." },
                    "query": { "type": "string", "description": "What you are about to do. Adds a section of the memories most relevant to it." },
                    "max_tokens": { "type": "integer", "minimum": 0, "description": "Token budget for the output (default 2500, 0 for unlimited)." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "memory_search",
            "description": "Full-text search over saved memories. Use before non-trivial work, when stuck, or when the user refers to past work. A plain question works. Returns one line per match with its category, abstract, URI and id; pass the id to memory_get for the full text.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "minLength": 1, "description": "Words to search for." },
                    "project": project,
                    "category": { "type": "string", "enum": CATEGORIES, "description": "Only return this category." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "description": "Maximum results (default 10)." }
                },
                "required": ["query"],
                "additionalProperties": false
            }
        },
        {
            "name": "memory_save",
            "description": "Save one fact so later sessions and other agents can use it. One fact per call, written to stand on its own. Categories: decision (choice and reason), case (bug root cause and fix), preference (user correction or preference), pattern (convention or quirk), entity (where something lives), event. Never save secrets or what the code or git history already records.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string", "minLength": 1, "description": "The fact, self-contained." },
                    "category": { "type": "string", "enum": CATEGORIES },
                    "project": project,
                    "importance": { "type": "number", "minimum": 0, "maximum": 1, "description": "0.5 default, 0.8 for decisions and corrections, 0.9+ only if forgetting it would cause damage." },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "agent": { "type": "string", "description": "Source agent recorded on the memory. Defaults to the MCP client's name." }
                },
                "required": ["text", "category"],
                "additionalProperties": false
            }
        },
        {
            "name": "memory_get",
            "description": "Fetch one memory in full, as markdown, by the id or rememora:// URI that memory_context, memory_search or memory_save returned. Pass id or uri.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "The memory's 26-character id." },
                    "uri": { "type": "string", "pattern": "^rememora://", "description": "The memory's rememora:// URI." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "memory_supersede",
            "description": "Replace an outdated memory instead of leaving two versions. Pass text to save the new fact and link it in one step (it inherits category, project, tags and importance), or by to link a memory you already saved.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "old": { "type": "string", "description": "ID or URI of the outdated memory." },
                    "text": { "type": "string", "description": "The new fact that replaces it. Use this or by." },
                    "by": { "type": "string", "description": "ID or URI of an existing memory that replaces it. Use this or text." }
                },
                "required": ["old"],
                "additionalProperties": false
            }
        },
        {
            "name": "session_start",
            "description": "Start a work session so the next agent can pick up where this one stopped.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent": { "type": "string", "description": "Your agent name, for example claude-code, codex, gemini, cursor. Defaults to the MCP client's name." },
                    "project": project,
                    "intent": { "type": "string", "description": "What you are trying to accomplish." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "session_end",
            "description": "End a session with a summary and the working state (blockers, next steps). Pass session_id, or omit it to end your own active session on the project.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "Session ID from session_start. Omit to end the active session." },
                    "project": { "type": "string", "description": "Project whose active session to end, when session_id is omitted." },
                    "agent": { "type": "string", "description": "Agent whose active session to end, when session_id is omitted. Defaults to the MCP client's name." },
                    "summary": { "type": "string", "description": "What was accomplished." },
                    "working_state": { "type": "string", "description": "Blockers and next steps for whoever continues." }
                },
                "required": ["summary"],
                "additionalProperties": false
            }
        }
    ])
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str()).filter(|s| !s.is_empty())
}

fn required<'a>(args: &'a Value, key: &str) -> std::result::Result<&'a str, String> {
    str_arg(args, key).ok_or_else(|| format!("Missing required argument: {key}"))
}

fn push_opt(argv: &mut Vec<String>, flag: &str, value: Option<String>) {
    if let Some(v) = value {
        argv.push(flag.into());
        argv.push(v);
    }
}

fn num_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_f64()).map(|n| {
        if n.fract() == 0.0 { format!("{}", n as i64) } else { n.to_string() }
    })
}

/// `build_argv_for` with the default agent, for callers with no client name.
#[cfg(test)]
pub fn build_argv(name: &str, args: &Value) -> std::result::Result<Option<Vec<String>>, String> {
    build_argv_for(name, args, DEFAULT_AGENT)
}

/// Map a tool call onto a rememora argv. `agent` is the source agent used when
/// the call does not name one. `Ok(None)` means unknown tool, `Err` an invalid
/// argument (reported as a tool error the model can fix).
pub fn build_argv_for(name: &str, args: &Value, agent: &str) -> std::result::Result<Option<Vec<String>>, String> {
    let s = |k: &str| str_arg(args, k).map(String::from);
    let agent = s("agent").unwrap_or_else(|| agent.to_string());
    let mut argv: Vec<String> = Vec::new();
    // Read tools return text for the model; tools whose ids get parsed return JSON.
    let mut json = true;
    match name {
        "memory_context" => {
            argv.push("context".into());
            match s("project") {
                Some(p) => push_opt(&mut argv, "--project", Some(p)),
                None => argv.push("--auto".into()),
            }
            push_opt(&mut argv, "--query", s("query"));
            push_opt(&mut argv, "--max-tokens", num_arg(args, "max_tokens"));
            json = false;
        }
        "memory_search" => {
            argv.push("search".into());
            argv.push(required(args, "query")?.into());
            push_opt(&mut argv, "--project", s("project"));
            push_opt(&mut argv, "--category", s("category"));
            push_opt(&mut argv, "--limit", num_arg(args, "limit"));
            push_opt(&mut argv, "--format", Some("compact".into()));
            json = false;
        }
        "memory_save" => {
            argv.push("save".into());
            argv.push(required(args, "text")?.into());
            let category = required(args, "category")?;
            if !CATEGORIES.contains(&category) {
                return Err(format!("Invalid category {category}: use one of {}", CATEGORIES.join(", ")));
            }
            push_opt(&mut argv, "--category", Some(category.into()));
            push_opt(&mut argv, "--project", s("project"));
            push_opt(&mut argv, "--importance", num_arg(args, "importance"));
            if let Some(tags) = args.get("tags").filter(|t| t.is_array()) {
                push_opt(&mut argv, "--tags", Some(tags.to_string()));
            }
            push_opt(&mut argv, "--agent", Some(agent));
        }
        "memory_get" => {
            argv.push("get".into());
            let reference = s("id").or_else(|| s("uri")).ok_or("Missing required argument: id or uri")?;
            argv.push(reference);
            json = false;
        }
        "memory_supersede" => {
            argv.push("supersede".into());
            argv.push(required(args, "old")?.into());
            match (s("text"), s("by")) {
                (Some(_), Some(_)) => return Err("Pass text or by, not both".into()),
                (Some(text), None) => {
                    push_opt(&mut argv, "--text", Some(text));
                    push_opt(&mut argv, "--agent", Some(agent));
                }
                (None, Some(by)) => push_opt(&mut argv, "--by", Some(by)),
                (None, None) => return Err("Missing required argument: text or by".into()),
            }
        }
        "session_start" => {
            argv.extend(["session".into(), "start".into()]);
            push_opt(&mut argv, "--agent", Some(agent));
            push_opt(&mut argv, "--project", s("project"));
            push_opt(&mut argv, "--intent", s("intent"));
        }
        "session_end" => {
            argv.push("session".into());
            match s("session_id") {
                Some(id) => argv.extend(["end".into(), id]),
                None => {
                    argv.push("end-active".into());
                    push_opt(&mut argv, "--project", s("project"));
                    push_opt(&mut argv, "--agent", Some(agent));
                }
            }
            push_opt(&mut argv, "--summary", Some(required(args, "summary")?.into()));
            push_opt(&mut argv, "--working-state", s("working_state"));
        }
        _ => return Ok(None),
    }
    if json {
        argv.push("--json".into());
    }
    Ok(Some(argv))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_echoes_known_version_else_latest() {
        let r = initialize_result(&json!({ "protocolVersion": "2025-03-26" }));
        assert_eq!(r["protocolVersion"], "2025-03-26");
        let r = initialize_result(&json!({ "protocolVersion": "1999-01-01" }));
        assert_eq!(r["protocolVersion"], SUPPORTED_PROTOCOL_VERSIONS[0]);
    }

    #[test]
    fn notifications_get_no_reply_and_unknown_methods_error() {
        let s = Server::new(true);
        assert!(s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        let r = s.handle_line(r#"{"jsonrpc":"2.0","id":7,"method":"nope"}"#).unwrap();
        assert_eq!(r["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(r["id"], 7);
        let r = s.handle_line("not json").unwrap();
        assert_eq!(r["error"]["code"], PARSE_ERROR);
    }

    #[test]
    fn argv_omits_unsupplied_optional_flags() {
        let a = build_argv("memory_context", &json!({ "project": "p" })).unwrap().unwrap();
        assert_eq!(a, ["context", "--project", "p"]);
        let a = build_argv("session_end", &json!({ "summary": "s" })).unwrap().unwrap();
        assert_eq!(a, ["session", "end-active", "--agent", "mcp", "--summary", "s", "--json"]);
        assert!(build_argv("memory_save", &json!({ "text": "x", "category": "bogus" })).is_err());
        assert!(build_argv("nope", &json!({})).unwrap().is_none());
    }

    #[test]
    fn read_tools_return_text_and_write_tools_json() {
        let a = build_argv("memory_context", &json!({ "query": "fix login", "max_tokens": 800 })).unwrap().unwrap();
        assert_eq!(a, ["context", "--auto", "--query", "fix login", "--max-tokens", "800"]);
        let a = build_argv("memory_search", &json!({ "query": "q" })).unwrap().unwrap();
        assert_eq!(a, ["search", "q", "--format", "compact"]);
        let a = build_argv("memory_get", &json!({ "id": "01ABC" })).unwrap().unwrap();
        assert_eq!(a, ["get", "01ABC"]);
        let a = build_argv("memory_get", &json!({ "uri": "rememora://x" })).unwrap().unwrap();
        assert_eq!(a, ["get", "rememora://x"]);
        assert!(build_argv("memory_get", &json!({})).is_err());
        let a = build_argv("memory_save", &json!({ "text": "t", "category": "case" })).unwrap().unwrap();
        assert_eq!(a.last().unwrap(), "--json");
    }

    #[test]
    fn supersede_takes_text_or_by() {
        let a = build_argv_for("memory_supersede", &json!({ "old": "o", "text": "new" }), "gemini-cli").unwrap().unwrap();
        assert_eq!(a, ["supersede", "o", "--text", "new", "--agent", "gemini-cli", "--json"]);
        let a = build_argv("memory_supersede", &json!({ "old": "o", "by": "n" })).unwrap().unwrap();
        assert_eq!(a, ["supersede", "o", "--by", "n", "--json"]);
        assert!(build_argv("memory_supersede", &json!({ "old": "o" })).is_err());
        assert!(build_argv("memory_supersede", &json!({ "old": "o", "by": "n", "text": "t" })).is_err());
    }

    #[test]
    fn save_agent_is_explicit_else_client_name_else_mcp() {
        let base = json!({ "text": "t", "category": "case" });
        let agent_of = |a: &[String]| a[a.iter().position(|x| x == "--agent").unwrap() + 1].clone();
        assert_eq!(agent_of(&build_argv("memory_save", &base).unwrap().unwrap()), "mcp");
        assert_eq!(agent_of(&build_argv_for("memory_save", &base, "gemini-cli").unwrap().unwrap()), "gemini-cli");
        let explicit = json!({ "text": "t", "category": "case", "agent": "cursor" });
        assert_eq!(agent_of(&build_argv_for("memory_save", &explicit, "gemini-cli").unwrap().unwrap()), "cursor");

        let s = Server::new(true);
        assert_eq!(s.client_name(), None);
        s.handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"gemini-cli"}}}"#);
        assert_eq!(s.client_name().as_deref(), Some("gemini-cli"));
    }
}
