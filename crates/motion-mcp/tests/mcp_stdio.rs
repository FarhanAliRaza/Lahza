//! Drives `lahza-motion mcp` over stdio with raw JSON-RPC, as an MCP
//! client would.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

struct Client {
    child: std::process::Child,
    out: BufReader<std::process::ChildStdout>,
    id: u64,
}

impl Client {
    fn send(&mut self, v: Value) {
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "{v}").unwrap();
        stdin.flush().unwrap();
    }
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let id = self.id;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let mut line = String::new();
            assert!(self.out.read_line(&mut line).unwrap() > 0, "server closed");
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == id {
                return v;
            }
        }
    }
}

#[test]
fn mcp_server_lists_tools_reads_resources_and_runs_tools() {
    let dir = std::env::temp_dir().join(format!("lahza-mcp-stdio-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_lahza-motion"))
        .args(["mcp", "--cpu"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let out = BufReader::new(child.stdout.take().unwrap());
    let mut c = Client { child, out, id: 0 };
    let init = c.request("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}));
    assert_eq!(init["result"]["serverInfo"]["name"], "lahza-motion", "{init}");
    assert!(init["result"]["instructions"].as_str().unwrap().contains("audio_analyze"));
    c.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    let tools = c.request("tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for want in ["project_create", "project_open", "project_info", "timeline_get", "markers_query", "edit_apply", "check", "snapshot", "render", "render_status", "history", "audio_analyze"] {
        assert!(names.contains(&want), "missing {want}: {names:?}");
    }
    let catalog = c.request("resources/read", json!({"uri": "lahza://catalog"}));
    let text = catalog["result"]["contents"][0]["text"].as_str().unwrap();
    let catalog: Value = serde_json::from_str(text).unwrap();
    assert!(catalog["presets"].as_array().unwrap().iter().any(|p| p["name"] == "glide"));
    let guide = c.request("resources/read", json!({"uri": "lahza://guide"}));
    assert!(guide["result"]["contents"][0]["text"].as_str().unwrap().contains("Sync the peak"));
    let prompts = c.request("prompts/get", json!({"name": "make_promo", "arguments": {"brief": "a launch"}}));
    assert!(prompts["result"]["messages"][0]["content"]["text"].as_str().unwrap().contains("promo.kinetic"));

    let created = c.request("tools/call", json!({"name": "project_create", "arguments": {"path": "demo", "width": 640, "height": 360, "duration": 2, "style": "promo.kinetic"}}));
    assert_eq!(created["result"]["isError"], false, "{created}");
    let edit = c.request("tools/call", json!({"name": "edit_apply", "arguments": {"ops": [
        {"op": "add_scene", "id": "s1"},
        {"op": "add", "scene": "s1", "id": "title", "kind": "text", "text": "HELLO", "size": 64, "transform": {"position": [320, 180]},
         "animators": [{"at": 0.2, "from": {"y": 40, "opacity": 0}, "stagger": "1f", "dur": "8f"}]}
    ]}}));
    assert_eq!(edit["result"]["isError"], false, "{edit}");
    let shot = c.request("tools/call", json!({"name": "snapshot", "arguments": {"at": [1.0], "width": 320}}));
    let content = shot["result"]["content"].as_array().unwrap();
    assert!(content.iter().any(|c| c["type"] == "image" && c["mimeType"] == "image/png"), "{shot}");
    let bad = c.request("tools/call", json!({"name": "edit_apply", "arguments": {"ops": [{"op": "remove", "id": "nope"}]}}));
    assert_eq!(bad["result"]["isError"], true);
    assert!(bad["result"]["content"][0]["text"].as_str().unwrap().contains("nope"));
    drop(c.child.stdin.take());
    let _ = c.child.wait();
    std::fs::remove_dir_all(&dir).ok();
}
