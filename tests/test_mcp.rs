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

#[test]
fn mcp_session_initialize_list_save_search_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("rememora.db");
    drop(rememora::db::open_with_options(&db, true).expect("init scratch db"));

    let mut child = Command::new(env!("CARGO_BIN_EXE_rememora"))
        .args(["mcp", "--no-encryption"])
        .env("HOME", dir.path())
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
    let mut c = Client { child, stdin, stdout };

    let init = c.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}));
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

    let found = call(&mut c, 4, "memory_search", json!({"query":"webhooks backoff","project":"mcptest"}));
    assert_eq!(found["result"]["isError"], false, "{found}");
    assert!(found["result"]["content"][0]["text"].as_str().unwrap().contains("exponential backoff"));

    let got = call(&mut c, 5, "memory_get", json!({"uri": uri}));
    assert_eq!(got["result"]["isError"], false, "{got}");

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
