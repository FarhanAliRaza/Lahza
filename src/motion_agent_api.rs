//! A runner that needs only an API key: our own loop on the Claude
//! Messages API with the engine's `Toolbox` in process, for a machine
//! without Claude Code. Raw HTTP (there is no official SDK for Rust).
//!
//! One request to the model per turn; the tools it asks for run here, in
//! this process, and their results go back in one message. Stop is looked
//! at between calls. The key is read from the environment or the app's
//! settings when a request starts and goes nowhere but the `x-api-key`
//! header: not into a project, a log, an error or `requests.json`.

use crate::motion_agent::{describe, Availability, Event, Failure, Request, Runner, Running, SYSTEM};
use base64::Engine;
use lahza_motion_mcp::tools::{tool_defs, Toolbox};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The model the loop asks, unless `LAHZA_AGENT_MODEL` names another.
const MODEL: &str = "claude-opus-5-5";
const API_VERSION: &str = "2026-10-21";
/// `fallbacks: "default"`: a request the model declines is re-run on the
/// model the API picks for that kind of refusal.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// Turns (requests to the model) one prompt may take.
const MAX_TURNS: usize = 80;
/// Tries of one request that failed for a reason worth trying again
/// (no connection, 429, 5xx).
const TRIES: usize = 3;

/// Documents the tools' descriptions point at; an MCP client reads them
/// as resources, this loop through the `read_doc` tool.
const DOCS: &[&str] = &["lahza://guide", "lahza://guide/breakdown", "lahza://schema", "lahza://catalog", "lahza://styles", "lahza://slang"];

/// Where the key is kept when it is not in the environment: one line in
/// the app's settings folder, readable by the owner only.
pub(crate) fn key_file() -> PathBuf {
    crate::recording::presets::config_root().join("anthropic-api-key")
}

/// The API key: `ANTHROPIC_API_KEY`, else the settings file.
fn key_from(env: Option<String>, file: &std::path::Path) -> Option<String> {
    env.or_else(|| std::fs::read_to_string(file).ok()).map(|k| k.trim().to_string()).filter(|k| !k.is_empty())
}

pub(crate) struct ApiLoop {
    /// `https://api.anthropic.com`, or `ANTHROPIC_BASE_URL`.
    pub base_url: String,
    pub model: String,
    /// Reads the key when a request starts (never kept).
    pub key: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    /// The conversations of this run of the app, by id: what a follow-up
    /// continues. In memory only.
    conversations: Arc<Mutex<HashMap<String, Vec<Value>>>>,
    /// Waits between tries (tests shorten it).
    pub backoff: Duration,
    /// What the tools draw with (None: the toolbox's own choice, as for
    /// `lahza mcp`).
    pub backend: Option<lahza_motion_render::Backend>,
}

impl Default for ApiLoop {
    fn default() -> Self {
        Self::new(
            std::env::var("ANTHROPIC_BASE_URL").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| "https://api.anthropic.com".into()),
            Arc::new(|| key_from(std::env::var("ANTHROPIC_API_KEY").ok(), &key_file())),
        )
    }
}

impl ApiLoop {
    pub(crate) fn new(base_url: String, key: Arc<dyn Fn() -> Option<String> + Send + Sync>) -> Self {
        let model = std::env::var("LAHZA_AGENT_MODEL").ok().filter(|m| !m.is_empty()).unwrap_or_else(|| MODEL.into());
        Self { base_url, model, key, conversations: Arc::default(), backoff: Duration::from_secs(2), backend: None }
    }
}

/// The engine's tools as the Messages API takes them, plus `read_doc`.
/// The same list every time, in the same order, so the prefix caches.
fn tools() -> Vec<Value> {
    let mut tools: Vec<Value> = tool_defs().into_iter().map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.schema})).collect();
    tools.push(json!({
        "name": "read_doc",
        "description": "Read one of the engine's documents: lahza://guide (the production loop, timing rules, review checklist), lahza://guide/breakdown, lahza://schema (the project format), lahza://catalog (presets, layer kinds, ops, time references), lahza://styles, lahza://slang. Read lahza://schema and lahza://catalog before the first edit of a conversation.",
        "input_schema": {"type": "object", "properties": {"uri": {"type": "string", "enum": DOCS}}, "required": ["uri"]},
    }));
    // Tools and system prompt are the stable prefix of every request.
    if let Some(last) = tools.last_mut() {
        last["cache_control"] = json!({"type": "ephemeral"});
    }
    tools
}

/// One request's body.
fn body(model: &str, messages: &[Value]) -> Value {
    json!({
        "model": model,
        "max_tokens": 16000,
        "system": [{"type": "text", "text": SYSTEM, "cache_control": {"type": "ephemeral"}}],
        "tools": tools(),
        "fallbacks": "default",
        "messages": messages,
    })
}

/// A tool's result as a `tool_result` block: its JSON, and any pictures
/// (a snapshot) for the model to look at.
fn tool_result(id: &str, result: Result<lahza_motion_mcp::tools::Output, String>) -> Value {
    match result {
        Ok(out) => {
            let mut content = vec![json!({"type": "text", "text": out.json.to_string()})];
            for png in &out.images {
                content.push(json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": base64::engine::general_purpose::STANDARD.encode(png)}}));
            }
            json!({"type": "tool_result", "tool_use_id": id, "content": content})
        }
        Err(e) => json!({"type": "tool_result", "tool_use_id": id, "content": e, "is_error": true}),
    }
}

/// Why a request to the API failed, and whether trying again can help.
struct Refused {
    again: bool,
    said: String,
}

/// Sends one request. The key is in a header and in nothing this returns.
fn post(agent: &ureq::Agent, base_url: &str, key: &str, body: &Value) -> Result<Value, Refused> {
    let sent = agent
        .post(&format!("{}/v1/messages", base_url.trim_end_matches('/')))
        .set("content-type", "application/json")
        .set("x-api-key", key)
        .set("anthropic-version", API_VERSION)
        .set("anthropic-beta", FALLBACK_BETA)
        .send_string(&body.to_string());
    match sent {
        Ok(response) => response.into_string().ok().and_then(|text| serde_json::from_str(&text).ok()).ok_or(Refused { again: true, said: "The API's answer could not be read.".into() }),
        Err(ureq::Error::Status(code, response)) => {
            // (`x-should-retry: false`: the API says trying again will not help.)
            let no_retry = response.header("x-should-retry") == Some("false");
            let message = response.into_string().ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(String::from)).unwrap_or_default();
            let said = match code {
                401 => "The API key was refused (401): check ANTHROPIC_API_KEY or the key in the app's settings.".to_string(),
                402 => format!("The API account has a billing problem (402). {message}"),
                403 => format!("This API key is not allowed to do this (403). {message}"),
                404 => format!("The model was not found or is not available to this key (404). {message}"),
                429 => format!("The API's rate limit was reached (429). {message}"),
                c if c >= 500 => format!("The API is having trouble ({c}). {message}"),
                c => format!("The API refused the request ({c}). {message}"),
            };
            Err(Refused { again: !no_retry && (code == 429 || code >= 500), said: said.trim().to_string() })
        }
        Err(ureq::Error::Transport(e)) => Err(Refused { again: true, said: format!("The API could not be reached: {}", e.kind()) }),
    }
}

struct Stop(Arc<AtomicBool>);

impl Running for Stop {
    fn stop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl Runner for ApiLoop {
    fn name(&self) -> &'static str {
        "Claude API"
    }

    fn available(&self) -> Availability {
        match (self.key)() {
            Some(_) => Availability::Ready,
            None => Availability::Missing(Failure::NoKey),
        }
    }

    fn start(&self, request: Request, events: Sender<Event>) -> Result<Box<dyn Running>, Failure> {
        let key = (self.key)().ok_or(Failure::NoKey)?;
        let stopped: Arc<AtomicBool> = Arc::default();
        let (flag, base_url, model, conversations, backoff, backend) = (stopped.clone(), self.base_url.clone(), self.model.clone(), self.conversations.clone(), self.backoff, self.backend);
        std::thread::Builder::new()
            .name("lahza-agent-api".into())
            .spawn(move || {
                // A follow-up continues what this run of the app remembers.
                let id = request.conversation.clone().filter(|c| conversations.lock().unwrap().contains_key(c)).unwrap_or_else(|| format!("api-{}", uuid::Uuid::new_v4().simple()));
                let mut messages = conversations.lock().unwrap().get(&id).cloned().unwrap_or_default();
                let mut content = vec![json!({"type": "text", "text": request.text})];
                if let Some(png) = &request.picture {
                    content.push(json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": base64::engine::general_purpose::STANDARD.encode(png)}}));
                }
                messages.push(json!({"role": "user", "content": content}));
                if events.send(Event::Started { conversation: id.clone() }).is_err() {
                    return;
                }
                let mut toolbox = Toolbox::new();
                toolbox.request = Some(request.id.clone());
                if let Some(backend) = backend {
                    toolbox.backend = backend;
                }
                if request.cwd.is_dir() {
                    toolbox.cwd = request.cwd.clone();
                }
                let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(20)).timeout(Duration::from_secs(600)).build();
                let end = |event: Event| {
                    let _ = events.send(event);
                };
                for _ in 0..MAX_TURNS {
                    if flag.load(Ordering::SeqCst) {
                        return;
                    }
                    let mut answer = None;
                    for attempt in 1..=TRIES {
                        match post(&agent, &base_url, &key, &body(&model, &messages)) {
                            Ok(v) => {
                                answer = Some(Ok(v));
                                break;
                            }
                            Err(refused) if refused.again && attempt < TRIES => {
                                std::thread::sleep(backoff * attempt as u32);
                                if flag.load(Ordering::SeqCst) {
                                    return;
                                }
                            }
                            Err(refused) => {
                                answer = Some(Err(refused.said));
                                break;
                            }
                        }
                    }
                    let answer = match answer {
                        Some(Ok(v)) => v,
                        Some(Err(said)) => return end(Event::Failed(Failure::Other(said))),
                        None => return end(Event::Failed(Failure::Other("The API could not be reached.".into()))),
                    };
                    // Stopped while the model was answering: nothing of the
                    // answer is acted on.
                    if flag.load(Ordering::SeqCst) {
                        return;
                    }
                    let blocks = answer.get("content").and_then(Value::as_array).cloned().unwrap_or_default();
                    let stop = answer.get("stop_reason").and_then(Value::as_str).unwrap_or("");
                    let mut said = String::new();
                    for block in &blocks {
                        if block.get("type").and_then(Value::as_str) == Some("text") {
                            let text = block.get("text").and_then(Value::as_str).unwrap_or("").trim();
                            if !text.is_empty() {
                                said = text.to_string();
                                let _ = events.send(Event::Said(said.clone()));
                            }
                        }
                    }
                    // A refusal or a cut-off answer: its tool calls may be
                    // half written, so none of them runs.
                    if stop == "refusal" {
                        let category = answer.pointer("/stop_details/category").and_then(Value::as_str).map(|c| format!(" ({c})")).unwrap_or_default();
                        return end(Event::Done { text: format!("The model declined this request{category}."), failed: true, cost: None });
                    }
                    if stop == "max_tokens" {
                        return end(Event::Done { text: "The model's answer was cut off (it ran out of room): ask for less at a time.".into(), failed: true, cost: None });
                    }
                    // The whole answer goes back as it came (thinking blocks
                    // and all), then every result in one message.
                    messages.push(json!({"role": "assistant", "content": blocks}));
                    let calls: Vec<&Value> = blocks.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use")).collect();
                    if stop != "tool_use" || calls.is_empty() {
                        if stop == "pause_turn" {
                            continue;
                        }
                        conversations.lock().unwrap().insert(id.clone(), messages);
                        return end(Event::Done { text: said, failed: false, cost: None });
                    }
                    let mut results = Vec::new();
                    for call in &calls {
                        let (call_id, name, input) = (call.get("id").and_then(Value::as_str).unwrap_or(""), call.get("name").and_then(Value::as_str).unwrap_or(""), call.get("input").cloned().unwrap_or(json!({})));
                        // Stopped between calls: the rest do not run. (What
                        // ran stays, to keep or undo.)
                        if flag.load(Ordering::SeqCst) {
                            return;
                        }
                        if events.send(Event::Step { id: call_id.to_string(), tool: name.to_string(), text: describe(name, &input) }).is_err() {
                            return;
                        }
                        let result = match name {
                            "read_doc" => input
                                .get("uri")
                                .and_then(Value::as_str)
                                .and_then(lahza_motion_mcp::server::read_doc)
                                .map(|(text, _)| lahza_motion_mcp::tools::Output { json: Value::String(text), images: Vec::new() })
                                .ok_or_else(|| format!("no such document: one of {}", DOCS.join(", "))),
                            _ if !input.is_object() => Err("the tool's input must be a JSON object".into()),
                            _ => toolbox.call(name, input),
                        };
                        let error = result.as_ref().err().map(|e| e.lines().next().unwrap_or("").to_string());
                        results.push(tool_result(call_id, result));
                        let _ = events.send(Event::StepDone { id: call_id.to_string(), error });
                    }
                    messages.push(json!({"role": "user", "content": results}));
                }
                end(Event::Done { text: format!("Stopped after {MAX_TURNS} turns without finishing."), failed: true, cost: None });
            })
            .map_err(|e| Failure::Other(e.to_string()))?;
        Ok(Box::new(Stop(stopped)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::mpsc::{channel, Receiver};

    const KEY: &str = "sk-ant-test-0123456789-secret";

    /// A local server that plays the API: it answers each request with the
    /// next of `answers` (status, body, a wait before answering) and keeps
    /// what it was sent. No request leaves the machine.
    struct Stub {
        url: String,
        seen: Arc<Mutex<Vec<(String, Value)>>>,
    }

    fn stub(answers: Vec<(u16, Value, u64)>) -> Stub {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
        let kept = seen.clone();
        std::thread::spawn(move || {
            let mut answers = answers.into_iter();
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut bytes = Vec::new();
                let mut chunk = [0u8; 65536];
                let (head, body) = loop {
                    let n = stream.read(&mut chunk).unwrap_or(0);
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(at) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&bytes[..at]).to_lowercase();
                        let length: usize = head.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|v| v.trim().parse().ok()).unwrap_or(0);
                        if bytes.len() >= at + 4 + length {
                            break (head, bytes[at + 4..at + 4 + length].to_vec());
                        }
                    }
                    if n == 0 {
                        break (String::new(), Vec::new());
                    }
                };
                kept.lock().unwrap().push((head, serde_json::from_slice(&body).unwrap_or(Value::Null)));
                let (code, answer, wait) = answers.next().unwrap_or((500, json!({"type": "error", "error": {"type": "api_error", "message": "the stub has no more answers"}}), 0));
                std::thread::sleep(Duration::from_millis(wait));
                let text = answer.to_string();
                let _ = write!(stream, "HTTP/1.1 {code} Stub\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}", text.len());
            }
        });
        Stub { url, seen }
    }

    fn runner(url: &str) -> ApiLoop {
        let mut api = ApiLoop::new(url.to_string(), Arc::new(|| Some(KEY.to_string())));
        api.model = MODEL.into();
        api.backoff = Duration::from_millis(5);
        api.backend = Some(lahza_motion_render::Backend::Cpu);
        api
    }

    /// A project with a title, in a folder of its own.
    fn project(name: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("lahza-agent-api-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut tb = Toolbox::new();
        tb.cwd = dir.clone();
        tb.backend = lahza_motion_render::Backend::Cpu;
        tb.call("project_create", json!({"path": "p", "width": 160, "height": 90, "duration": 2})).unwrap();
        tb.call("edit_apply", json!({"ops": [{"op": "add_scene", "id": "s1"}, {"op": "add", "id": "title", "kind": "text", "text": "Hello", "size": 24, "transform": {"position": [80, 30]}}]})).unwrap();
        (dir.clone(), dir.join("p.lahza"))
    }

    fn message(stop: &str, content: Value) -> Value {
        json!({"id": "msg_1", "type": "message", "role": "assistant", "model": MODEL, "content": content, "stop_reason": stop, "usage": {"input_tokens": 10, "output_tokens": 5}})
    }

    fn set(id: &str, project: &std::path::Path, y: f64) -> Value {
        json!({"type": "tool_use", "id": id, "name": "edit_apply", "input": {"project": project, "ops": [{"op": "set", "target": "title.transform.position", "value": [80, y]}]}})
    }

    fn collect(rx: &Receiver<Event>) -> Vec<Event> {
        let mut out = Vec::new();
        while let Ok(e) = rx.recv_timeout(Duration::from_secs(60)) {
            let last = matches!(e, Event::Done { .. } | Event::Failed(_));
            out.push(e);
            if last {
                break;
            }
        }
        out
    }

    fn title_y(project: &std::path::Path) -> f64 {
        let p = lahza_motion_core::io::load(project).unwrap();
        serde_json::to_value(&p.scenes[0].layers[0]).unwrap()["transform"]["position"][1].as_f64().unwrap()
    }

    #[test]
    fn a_tool_use_conversation_runs_the_tools_here_and_a_follow_up_continues_it() {
        let (dir, p) = project("loop");
        let server = stub(vec![
            (200, message("tool_use", json!([
                {"type": "thinking", "thinking": "", "signature": "sig"},
                {"type": "text", "text": "I'll move it and look."},
                set("t1", &p, 60.0),
                {"type": "tool_use", "id": "t2", "name": "snapshot", "input": {"project": p, "at": [0.5], "width": 80}},
                {"type": "tool_use", "id": "t3", "name": "read_doc", "input": {"uri": "lahza://guide/breakdown"}},
                {"type": "tool_use", "id": "t4", "name": "edit_apply", "input": {"project": p, "ops": [{"op": "remove", "id": "nope"}]}}
            ])), 0),
            (200, message("end_turn", json!([{"type": "text", "text": "Moved the title down."}])), 0),
            (200, message("end_turn", json!([{"type": "text", "text": "It is at 60."}])), 0),
        ]);
        let api = runner(&server.url);
        assert_eq!((api.name(), api.available()), ("Claude API", Availability::Ready));
        let (tx, rx) = channel();
        let _running = api.start(Request { id: "r5".into(), text: "Selected layer: title\n\nmove it down".into(), picture: Some(vec![1, 2, 3]), cwd: dir.clone(), conversation: None }, tx).unwrap();
        let events = collect(&rx);
        let Event::Started { conversation } = &events[0] else { panic!("{events:?}") };
        assert!(conversation.starts_with("api-"));
        assert_eq!(events[1], Event::Said("I'll move it and look.".into()));
        let steps: Vec<(&str, &str)> = events.iter().filter_map(|e| if let Event::Step { tool, text, .. } = e { Some((tool.as_str(), text.as_str())) } else { None }).collect();
        assert_eq!(steps, [("edit_apply", "Editing: set title.transform.position"), ("snapshot", "Looking at the frame at [0.5]"), ("read_doc", "Read doc"), ("edit_apply", "Editing: remove nope")]);
        let ended: Vec<Option<&str>> = events.iter().filter_map(|e| if let Event::StepDone { error, .. } = e { Some(error.as_deref()) } else { None }).collect();
        assert_eq!(ended.len(), 4);
        assert!(ended[..3].iter().all(Option::is_none) && ended[3].is_some_and(|e| e.contains("nope")), "{ended:?}");
        assert_eq!(events.last(), Some(&Event::Done { text: "Moved the title down.".into(), failed: false, cost: None }));
        // The edit was made here, and carries the request.
        assert_eq!(title_y(&p), 60.0);
        let history = lahza_motion_core::io::read_history(&p).unwrap();
        assert_eq!(history.last().unwrap().request.as_deref(), Some("r5"));

        let seen = server.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 2);
        // The first request: the headers, the model, every tool of the
        // engine, the words and the picture.
        let (head, body) = &seen[0];
        assert!(head.starts_with("post /v1/messages"), "{head}");
        for line in [format!("x-api-key: {}", KEY.to_lowercase()), "anthropic-version: 2026-10-21".into(), "anthropic-beta: server-side-fallback-2026-07-01".into(), "content-type: application/json".into()] {
            assert!(head.contains(&line), "{line} in {head}");
        }
        assert_eq!((body["model"].as_str(), body["max_tokens"].as_u64(), body["fallbacks"].as_str()), (Some("claude-opus-5-5"), Some(16000), Some("default")));
        assert!(body.get("thinking").is_none() && body.get("temperature").is_none() && body.get("tool_choice").is_none());
        let names: Vec<&str> = body["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names.len(), tool_defs().len() + 1);
        assert!(names.contains(&"edit_apply") && names.contains(&"sim_bake") && names.last() == Some(&"read_doc"));
        assert!(body["tools"].as_array().unwrap().iter().all(|t| t["input_schema"]["type"] == "object" && t["description"].is_string()));
        assert_eq!(body["tools"].as_array().unwrap().last().unwrap()["cache_control"], json!({"type": "ephemeral"}));
        assert!(body["system"][0]["text"].as_str().unwrap().contains("Lahza motion editor"));
        assert_eq!(body["messages"], json!([{"role": "user", "content": [
            {"type": "text", "text": "Selected layer: title\n\nmove it down"},
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AQID"}}
        ]}]));
        // The second: the answer as it came (its thinking block too), then
        // every result in one message, in order.
        let messages = seen[1].1["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!((messages[1]["role"].as_str(), messages[1]["content"][0]["type"].as_str(), messages[1]["content"].as_array().unwrap().len()), (Some("assistant"), Some("thinking"), 6));
        let results = messages[2]["content"].as_array().unwrap();
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(results.iter().map(|r| (r["type"].as_str().unwrap(), r["tool_use_id"].as_str().unwrap())).collect::<Vec<_>>(), [("tool_result", "t1"), ("tool_result", "t2"), ("tool_result", "t3"), ("tool_result", "t4")]);
        assert!(results[0]["content"][0]["text"].as_str().unwrap().contains("\"history\""));
        assert_eq!(results[1]["content"][1]["source"]["media_type"], "image/png", "the snapshot goes back as a picture");
        assert!(results[2]["content"][0]["text"].as_str().unwrap().contains("Breakdown"));
        assert!(results[3]["is_error"] == true && results[3]["content"].as_str().unwrap().contains("nope"));
        assert!(results[..3].iter().all(|r| r.get("is_error").is_none()));

        // A follow-up continues the conversation; one it does not know
        // starts anew.
        let (tx, rx) = channel();
        let _running = api.start(Request { id: "r6".into(), text: "where is it now?".into(), picture: None, cwd: dir.clone(), conversation: Some(conversation.clone()) }, tx).unwrap();
        let events = collect(&rx);
        assert_eq!(events.first(), Some(&Event::Started { conversation: conversation.clone() }));
        assert_eq!(events.last(), Some(&Event::Done { text: "It is at 60.".into(), failed: false, cost: None }));
        let third = server.seen.lock().unwrap()[2].1["messages"].as_array().unwrap().clone();
        assert_eq!(third.len(), 5, "what was said before, then the new words");
        assert_eq!(third[4]["content"][0]["text"], "where is it now?");

        // The key is in the header and nowhere else: not in the project,
        // not in what is reported.
        let mut pending = vec![dir.clone()];
        while let Some(folder) = pending.pop() {
            for entry in std::fs::read_dir(&folder).unwrap().flatten() {
                match entry.path() {
                    path if path.is_dir() => pending.push(path),
                    path => assert!(!String::from_utf8_lossy(&std::fs::read(&path).unwrap()).contains(KEY), "{}", path.display()),
                }
            }
        }
        assert!(!format!("{events:?}{:?}", server.seen.lock().unwrap().iter().map(|(_, b)| b.to_string()).collect::<Vec<_>>()).contains(KEY));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_refusal_or_a_cut_off_answer_runs_none_of_its_tools() {
        let (dir, p) = project("refusal");
        for (stop, details, expect) in [("refusal", json!({"type": "refusal", "category": "cyber", "explanation": null}), "The model declined this request (cyber)."), ("max_tokens", Value::Null, "The model's answer was cut off (it ran out of room): ask for less at a time.")] {
            let mut answer = message(stop, json!([{"type": "text", "text": "Starting."}, set("t1", &p, 5.0)]));
            answer["stop_details"] = details;
            let server = stub(vec![(200, answer, 0)]);
            let (tx, rx) = channel();
            let _running = runner(&server.url).start(Request { id: "r".into(), text: "x".into(), cwd: dir.clone(), ..Default::default() }, tx).unwrap();
            let events = collect(&rx);
            assert_eq!(events.last(), Some(&Event::Done { text: expect.into(), failed: true, cost: None }), "{stop}");
            assert!(!events.iter().any(|e| matches!(e, Event::Step { .. })), "{stop}: {events:?}");
            assert_eq!((title_y(&p), server.seen.lock().unwrap().len()), (30.0, 1), "{stop}: nothing ran, nothing was asked again");
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn errors_are_said_and_only_the_passing_ones_are_tried_again() {
        let (dir, _) = project("errors");
        let ask = |api: &ApiLoop| {
            let (tx, rx) = channel();
            let _running = api.start(Request { id: "r".into(), text: "x".into(), cwd: dir.clone(), ..Default::default() }, tx).unwrap();
            collect(&rx)
        };
        let error = |kind: &str, text: &str| json!({"type": "error", "error": {"type": kind, "message": text}});
        // Nothing listening: tried three times, then said.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let events = ask(&runner(&format!("http://127.0.0.1:{port}")));
        assert!(matches!(events.last(), Some(Event::Failed(Failure::Other(e))) if e.starts_with("The API could not be reached")), "{events:?}");
        // Overloaded, then rate limited, then it answers.
        let server = stub(vec![(529, error("overloaded_error", "Overloaded"), 0), (429, error("rate_limit_error", "slow down"), 0), (200, message("end_turn", json!([{"type": "text", "text": "Done."}])), 0)]);
        assert_eq!(ask(&runner(&server.url)).last(), Some(&Event::Done { text: "Done.".into(), failed: false, cost: None }));
        assert_eq!(server.seen.lock().unwrap().len(), 3);
        // Three failures in a row: said, with the API's words.
        let server = stub(vec![(500, error("api_error", "Internal server error"), 0); 3]);
        assert_eq!(ask(&runner(&server.url)).last(), Some(&Event::Failed(Failure::Other("The API is having trouble (500). Internal server error".into()))));
        assert_eq!(server.seen.lock().unwrap().len(), 3);
        // A refused key, a bad request: not tried again, and the key is
        // not in what is said.
        for (code, kind, text, expect) in [
            (401, "authentication_error", "invalid x-api-key", "The API key was refused (401): check ANTHROPIC_API_KEY or the key in the app's settings."),
            (400, "invalid_request_error", "messages: bad", "The API refused the request (400). messages: bad"),
            (404, "not_found_error", "model: nope", "The model was not found or is not available to this key (404). model: nope"),
        ] {
            let server = stub(vec![(code, error(kind, text), 0)]);
            let events = ask(&runner(&server.url));
            assert_eq!(events.last(), Some(&Event::Failed(Failure::Other(expect.into()))));
            assert_eq!(server.seen.lock().unwrap().len(), 1, "{code} is not tried again");
            assert!(!format!("{events:?}").contains(KEY));
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn stop_in_the_middle_ends_it_between_calls() {
        let (dir, p) = project("stop");
        // The second answer takes a while and asks for another edit.
        let server = stub(vec![(200, message("tool_use", json!([set("t1", &p, 60.0)])), 0), (200, message("tool_use", json!([set("t2", &p, 5.0)])), 400), (200, message("end_turn", json!([{"type": "text", "text": "never"}])), 0)]);
        let (tx, rx) = channel();
        let mut running = runner(&server.url).start(Request { id: "r".into(), text: "x".into(), cwd: dir.clone(), ..Default::default() }, tx).unwrap();
        // Stop once the first edit is made: the model is being asked again.
        loop {
            match rx.recv_timeout(Duration::from_secs(60)).unwrap() {
                Event::StepDone { id, error } => {
                    assert_eq!((id.as_str(), error), ("t1", None));
                    break;
                }
                Event::Done { .. } | Event::Failed(_) => panic!("it ended"),
                _ => {}
            }
        }
        running.stop();
        // Nothing more is reported, the second edit is not made, and the
        // model is not asked a third time. What ran stays, to keep or undo.
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_err());
        assert_eq!(title_y(&p), 60.0);
        // (Stop lands before the second question or while it is answered.)
        std::thread::sleep(Duration::from_millis(600));
        assert!(server.seen.lock().unwrap().len() <= 2);
        assert_eq!(title_y(&p), 60.0, "the second answer's edit was not made");
        // Stopped before the first answer: nothing at all.
        let server = stub(vec![(200, message("tool_use", json!([set("t1", &p, 5.0)])), 300)]);
        let (tx, rx) = channel();
        let mut running = runner(&server.url).start(Request { id: "r".into(), text: "x".into(), cwd: dir.clone(), ..Default::default() }, tx).unwrap();
        assert!(matches!(rx.recv_timeout(Duration::from_secs(60)).unwrap(), Event::Started { .. }));
        running.stop();
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_err());
        assert_eq!(title_y(&p), 60.0);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_key_comes_from_the_environment_or_the_settings_and_its_lack_is_said() {
        let dir = std::env::temp_dir().join(format!("lahza-agent-api-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("anthropic-api-key");
        assert_eq!(key_from(None, &file), None);
        std::fs::write(&file, "  sk-from-file\n").unwrap();
        assert_eq!(key_from(None, &file).as_deref(), Some("sk-from-file"));
        assert_eq!(key_from(Some("sk-from-env".into()), &file).as_deref(), Some("sk-from-env"));
        std::fs::write(&file, "\n").unwrap();
        assert_eq!(key_from(None, &file), None);
        let none = ApiLoop::new("http://127.0.0.1:1".into(), Arc::new(|| None));
        assert_eq!(none.available(), Availability::Missing(Failure::NoKey));
        let (tx, _rx) = channel();
        assert_eq!(none.start(Request::default(), tx).err(), Some(Failure::NoKey));
        let said = Failure::NoKey.message("Claude API");
        assert!(said.starts_with("Claude API has no API key. Set ANTHROPIC_API_KEY, or put the key in ") && said.contains("anthropic-api-key"), "{said}");
        std::fs::remove_dir_all(dir).ok();
    }
}
