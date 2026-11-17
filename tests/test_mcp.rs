//! Integration test for `rememora mcp`: spawn the built binary as a stdio
//! MCP server against a scratch database and drive a real session.
//!
//! The first-run gate in main.rs refuses a missing REMEMORA_DB, so the test
//! materialises the database first with `rememora::db::open_with_options`
//! (unencrypted), the same library call the rest of the suite uses. HOME is
//! pointed at the temp dir so nothing reaches the developer's real files.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

struct Client {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl Client {
    fn send(&mut self, msg: Value) {
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn request(&mut self, msg: Value) -> Value {
        self.send(msg);
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad JSON {line:?}: {e}"));
        assert_eq!(v["jsonrpc"], "2.0");
        v
    }
}

fn call(c: &mut Client, id: u64, name: &str, arguments: Value) -> Value {
    c.request(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}}))
}

fn spawn(dir: &std::path::Path, readonly: bool) -> Client {
    let db = dir.join("rememora.db");
    drop(rememora::db::open_with_options(&db, true).expect("init scratch db"));

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rememora"));
    if readonly {
        cmd.env("REMEMORA_READONLY", "1");
    } else {
        cmd.env_remove("REMEMORA_READONLY");
    }
    let mut child = cmd
        .args(["mcp", "--no-encryption"])
        .env("HOME", dir)
        .env("REMEMORA_DB", &db)
        .env("REMEMORA_TEST_NO_KEYCHAIN", "1")
        .env("REMEMORA_NO_UPDATE_CHECK", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rememora mcp");
    let stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    Client { child, stdin, stdout }
}

fn text(r: &Value) -> &str {
    r["result"]["content"][0]["text"].as_str().unwrap()
}

#[test]
fn mcp_session_initialize_list_save_search_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = spawn(dir.path(), false);

    let init = c.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"gemini-cli","version":"0"}}}));
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "rememora");
    assert!(init["result"]["capabilities"]["tools"].is_object());

    c.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));

    let list = c.request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter()
        .map(|t| t["name"].as_str().unwrap()).collect();
    for want in ["memory_context","memory_search","memory_save","memory_get","memory_supersede","session_start","session_end"] {
        assert!(names.contains(&want), "missing tool {want}");
    }
    for t in list["result"]["tools"].as_array().unwrap() {
        assert_eq!(t["inputSchema"]["type"], "object");
    }

    let saved = call(&mut c, 3, "memory_save", json!({
        "text":"The billing service retries webhooks with exponential backoff capped at 10 minutes",
        "category":"decision","project":"mcptest","importance":0.8}));
    assert_eq!(saved["result"]["isError"], false, "{saved}");
    let saved_text = saved["result"]["content"][0]["text"].as_str().unwrap();
    let saved_json: Value = serde_json::from_str(saved_text).expect("save --json output");
    let uri = saved_json["uri"].as_str().expect("uri in save output").to_string();
    let id = saved_json["id"].as_str().expect("id in save output").to_string();

    // Compact text, not JSON: one line per hit with the id for memory_get.
    let found = call(&mut c, 4, "memory_search", json!({"query":"how do webhooks retry?","project":"mcptest"}));
    assert_eq!(found["result"]["isError"], false, "{found}");
    let found = text(&found);
    assert!(found.contains("exponential backoff"), "{found}");
    assert!(found.contains(&format!("id {id}")), "{found}");
    assert!(serde_json::from_str::<Value>(found).is_err(), "search should not be JSON: {found}");

    // memory_get by id and by URI: full markdown, provenance from clientInfo.
    for arg in [json!({"id": id}), json!({"uri": uri})] {
        let got = call(&mut c, 5, "memory_get", arg);
        assert_eq!(got["result"]["isError"], false, "{got}");
        let got = text(&got);
        assert!(got.contains("exponential backoff"), "{got}");
        assert!(got.contains("gemini-cli"), "source agent should be the MCP client: {got}");
    }

    // memory_context with a query: CLI markdown with a "Relevant to" section.
    let ctx = call(&mut c, 11, "memory_context", json!({"project":"mcptest","query":"webhook retries","max_tokens":1500}));
    assert_eq!(ctx["result"]["isError"], false, "{ctx}");
    let ctx = text(&ctx);
    assert!(ctx.starts_with("# Rememora Context: mcptest"), "{ctx}");
    assert!(ctx.contains("## Relevant to: webhook retries"), "{ctx}");
    assert!(ctx.contains(&format!("`{id}`")), "{ctx}");
    assert!(!ctx.contains("rememora://projects/"), "context lines carry ids, not URIs: {ctx}");

    // One-step supersede with text.
    let sup = call(&mut c, 12, "memory_supersede", json!({"old": id,
        "text":"The billing service retries webhooks with exponential backoff capped at 15 minutes"}));
    assert_eq!(sup["result"]["isError"], false, "{sup}");

    // Unknown method: JSON-RPC error, not a crash.
    let bad = c.request(json!({"jsonrpc":"2.0","id":6,"method":"resources/list"}));
    assert_eq!(bad["error"]["code"], -32601);
    // Unknown tool: invalid params.
    let bad = call(&mut c, 7, "memory_delete_everything", json!({}));
    assert_eq!(bad["error"]["code"], -32602);
    // Missing required argument: tool error the model can correct.
    let bad = call(&mut c, 8, "memory_save", json!({"category":"decision"}));
    assert_eq!(bad["result"]["isError"], true);
    // CLI failure: stderr surfaces with isError.
    let bad = call(&mut c, 9, "memory_get", json!({"uri":"rememora://projects/mcptest/memories/decision/missing"}));
    assert_eq!(bad["result"]["isError"], true);
    assert!(bad["result"]["content"][0]["text"].as_str().unwrap().contains("not found"));

    let pong = c.request(json!({"jsonrpc":"2.0","id":10,"method":"ping"}));
    assert_eq!(pong["result"], json!({}));

    drop(c.stdin);
    let status = c.child.wait().unwrap();
    assert!(status.success());
}

/// Under REMEMORA_READONLY=1 the server starts and reads work, but every
/// write tool is refused by the CLI subprocess it spawns.
#[test]
fn mcp_readonly_mode_serves_reads_and_refuses_writes() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = spawn(dir.path(), true);
    let init = c.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}));
    assert!(init["result"].is_object(), "{init}");

    let found = call(&mut c, 2, "memory_search", json!({"query":"anything"}));
    assert_eq!(found["result"]["isError"], false, "{found}");
    let ctx = call(&mut c, 3, "memory_context", json!({"project":"p"}));
    assert_eq!(ctx["result"]["isError"], false, "{ctx}");

    for (i, (tool, args)) in [
        ("memory_save", json!({"text":"x","category":"decision","project":"p"})),
        ("memory_supersede", json!({"old":"a","by":"b"})),
        ("session_start", json!({"agent":"a","project":"p"})),
        ("session_end", json!({"summary":"s","project":"p"})),
    ].into_iter().enumerate() {
        let r = call(&mut c, 10 + i as u64, tool, args);
        assert_eq!(r["result"]["isError"], true, "{tool} must be refused: {r}");
        assert!(text(&r).contains("read-only mode"), "{tool}: {r}");
    }

    drop(c.stdin);
    assert!(c.child.wait().unwrap().success());
}
