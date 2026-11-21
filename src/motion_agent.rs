//! Starting an agent from the app. A `Runner` takes one request (the words
//! and what they are about) and reports what the agent does as it does it.
//! The first runner is Claude Code without its terminal: a child process
//! with `lahza mcp` as its only tools, its stream of JSON read line by
//! line. Another client, or a loop of our own on the `Toolbox`, takes its
//! place by implementing `Runner`.

use base64::Engine;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

/// One prompt and what goes with it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Request {
    /// Edits made for it carry this id in the history.
    pub id: String,
    /// The words, with what they are about written out before them
    /// (`compose`).
    pub text: String,
    /// The frame at the playhead as it is (PNG).
    pub picture: Option<Vec<u8>>,
    /// The folder the server works in (relative paths are against it).
    pub cwd: PathBuf,
    /// The conversation to continue: a follow-up.
    pub conversation: Option<String>,
}

/// What a runner reports, in order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Event {
    /// It began; `conversation` continues it later.
    Started { conversation: String },
    /// A tool the agent called, said plainly.
    Step { id: String, tool: String, text: String },
    /// How that step ended.
    StepDone { id: String, error: Option<String> },
    /// Words between steps.
    Said(String),
    /// Tokens the request took in and gave out, where the runner knows
    /// (sent before `Done`).
    Usage { input: u64, output: u64 },
    /// The end of the request: what it said last, and what it cost in
    /// dollars where the runner is told.
    Done { text: String, failed: bool, cost: Option<f64> },
    /// It could not run or stopped short.
    Failed(Failure),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Failure {
    NotInstalled,
    NotSignedIn,
    /// The API runner has no key.
    NoKey,
    Other(String),
}

impl Failure {
    /// What to tell the person, for a runner called `name`.
    pub(crate) fn message(&self, name: &str) -> String {
        match self {
            Failure::NotInstalled => format!("{name} is not installed. Install it (claude.com/claude-code), then try again."),
            Failure::NotSignedIn => format!("{name} is not signed in. Run `claude` in a terminal and sign in, then try again."),
            Failure::NoKey => format!("{name} has no API key. Set ANTHROPIC_API_KEY, or put the key in {}.", crate::motion_agent_api::key_file().display()),
            Failure::Other(e) => e.clone(),
        }
    }
}

/// Whether a runner can run here.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Availability {
    Ready,
    Missing(Failure),
}

pub(crate) trait Running: Send {
    /// Ends the agent now.
    fn stop(&mut self);
}

pub(crate) trait Runner: Send + Sync {
    fn name(&self) -> &'static str;
    /// May take a moment: call it off the UI thread.
    fn available(&self) -> Availability;
    /// Starts the request; events arrive on `events` until `Done` or
    /// `Failed` (or `stop`).
    fn start(&self, request: Request, events: Sender<Event>) -> Result<Box<dyn Running>, Failure>;
}

// ------------------------------------------------------------- the choice

/// Which runner prompts go to: kept with the app's settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Choice {
    /// Claude Code where it is installed and signed in, else the API loop
    /// where a key is set.
    #[default]
    Auto,
    ClaudeCode,
    Api,
}

impl Choice {
    pub(crate) const ALL: [Choice; 3] = [Choice::Auto, Choice::ClaudeCode, Choice::Api];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Choice::Auto => "Auto",
            Choice::ClaudeCode => "Claude Code",
            Choice::Api => "API key",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Choice::Auto => "auto",
            Choice::ClaudeCode => "claude-code",
            Choice::Api => "api",
        }
    }
}

static CHOICE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn choice_file() -> PathBuf {
    crate::recording::presets::config_root().join("motion-agent")
}

fn read_choice(path: &Path) -> Choice {
    let kept = std::fs::read_to_string(path).unwrap_or_default();
    Choice::ALL.into_iter().find(|c| c.name() == kept.trim()).unwrap_or_default()
}

fn write_choice(path: &Path, choice: Choice) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, choice.name());
}

pub(crate) fn choice() -> Choice {
    Choice::ALL[CHOICE.load(Ordering::Relaxed) as usize % 3]
}

/// Reads the kept choice (at start).
pub(crate) fn load_choice() {
    CHOICE.store(read_choice(&choice_file()) as u8, Ordering::Relaxed);
}

pub(crate) fn set_choice(choice: Choice) {
    CHOICE.store(choice as u8, Ordering::Relaxed);
    write_choice(&choice_file(), choice);
}

/// The runner prompts go to, by the choice: Claude Code, or the loop of
/// our own on the API. Itself a `Runner`, so the studio holds one.
pub(crate) struct Chosen {
    pub claude: Arc<dyn Runner>,
    pub api: Arc<dyn Runner>,
    pub choice: Box<dyn Fn() -> Choice + Send + Sync>,
    /// What `available` last settled on: the choice it was made for, and
    /// whether it is the API loop.
    picked: Mutex<Option<(Choice, bool)>>,
}

impl Default for Chosen {
    fn default() -> Self {
        Self::new(Arc::new(ClaudeCode::default()), Arc::new(crate::motion_agent_api::ApiLoop::default()), Box::new(choice))
    }
}

impl Chosen {
    pub(crate) fn new(claude: Arc<dyn Runner>, api: Arc<dyn Runner>, choice: Box<dyn Fn() -> Choice + Send + Sync>) -> Self {
        Self { claude, api, choice, picked: Mutex::default() }
    }

    /// The runner for the choice as it is now, and whether it can run.
    /// (Asks Claude Code whether it is signed in: not for the UI thread.)
    fn pick(&self) -> (bool, Availability) {
        let choice = (self.choice)();
        let (api, availability) = match choice {
            Choice::ClaudeCode => (false, self.claude.available()),
            Choice::Api => (true, self.api.available()),
            Choice::Auto => match self.claude.available() {
                Availability::Ready => (false, Availability::Ready),
                Availability::Missing(claude) => match self.api.available() {
                    Availability::Ready => (true, Availability::Ready),
                    // Neither: say both, so either can be put right.
                    Availability::Missing(api) => (false, Availability::Missing(Failure::Other(format!("{} Or: {}", claude.message(self.claude.name()), api.message(self.api.name()))))),
                },
            },
        };
        *self.picked.lock().unwrap() = Some((choice, api));
        (api, availability)
    }

    fn uses_api(&self) -> bool {
        let picked = *self.picked.lock().unwrap();
        match picked {
            Some((choice, api)) if choice == (self.choice)() => api,
            _ => self.pick().0,
        }
    }
}

impl Runner for Chosen {
    fn name(&self) -> &'static str {
        let picked = *self.picked.lock().unwrap();
        match picked {
            Some((_, true)) => self.api.name(),
            Some((_, false)) => self.claude.name(),
            None => "The agent",
        }
    }

    fn available(&self) -> Availability {
        self.pick().1
    }

    fn start(&self, mut request: Request, events: Sender<Event>) -> Result<Box<dyn Running>, Failure> {
        let api = self.uses_api();
        // A conversation is one runner's: the other starts a new one.
        if request.conversation.as_deref().is_some_and(|c| c.starts_with("api-") != api) {
            request.conversation = None;
        }
        if api { self.api.start(request, events) } else { self.claude.start(request, events) }
    }
}

// ------------------------------------------------------------ Claude Code

/// Claude Code, headless (`claude -p`), with this app's `mcp` server as its
/// only tools.
pub(crate) struct ClaudeCode {
    /// The `claude` program.
    pub program: PathBuf,
    /// The program that serves `mcp` (this app).
    pub server: PathBuf,
}

impl Default for ClaudeCode {
    fn default() -> Self {
        Self { program: "claude".into(), server: std::env::current_exe().unwrap_or_else(|_| "lahza".into()) }
    }
}

pub(crate) const SYSTEM: &str = "You are working inside the Lahza motion editor, for the person using it. They see each edit you make appear in their preview as you make it, and they can undo everything you do for this request in one step. \
Work only with the lahza tools, on the project named in the message. Do what was asked and no more: when it is about one layer, change that layer. \
Look before and after (snapshot, review, check) rather than guessing. Do not render a video unless asked. \
Do not guess an edit's form. Every edit is edit_apply with \"ops\": a list of objects, each with \"op\". The common ones, exactly: \
{\"op\": \"set\", \"target\": \"<layer id>.<field path>\", \"value\": …} (\"target\" is one string, the id and the path joined by a dot: \"title.color\", \"box.fill\", \"box.transform.position\"; there is no separate \"id\" or \"field\"); \
{\"op\": \"key\", \"target\": \"<layer id>.<field path>\", \"at\": <seconds or a time reference>, \"v\": …}; \
{\"op\": \"add\", \"scene\": \"<scene id>\", \"id\": …, \"kind\": \"shape\" | \"text\" | …, …the layer's fields}; {\"op\": \"remove\", \"id\": …}. \
A shape layer's \"shape\" is one of circle, rect, capsule, ring, arc, polygon, star, line, path, field (an ellipse is a circle whose \"size\" has a width and a height that differ). Colours are \"#rrggbb\": a text's is \"color\", a shape's is \"fill\". \
Before changing a layer read it as it is (timeline_get with full: true shows the JSON that edit_apply edits) rather than trying a form; lahza://schema and lahza://catalog have every field, op and kind, where you can read resources. \
End with one or two plain sentences saying what you changed.";

impl ClaudeCode {
    fn command(&self, request: &Request) -> Command {
        let mut env = json!({"LAHZA_REQUEST": request.id});
        // A machine without a GPU: the server draws its snapshots as this
        // app draws its preview.
        if let Ok(software) = std::env::var("LAHZA_MOTION_SOFTWARE_GPU") {
            env["LAHZA_MOTION_SOFTWARE_GPU"] = json!(software);
        }
        let servers = json!({"mcpServers": {"lahza": {"command": self.server, "args": ["mcp"], "env": env}}});
        let mut c = Command::new(&self.program);
        c.args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"])
            // Only our server, none of Claude Code's own tools (no shell, no
            // files), and every tool of ours allowed without asking: there
            // is no terminal to ask in.
            .args(["--strict-mcp-config", "--mcp-config", &servers.to_string()])
            .args(["--tools", "", "--allowedTools", "mcp__lahza", "--permission-mode", "dontAsk"])
            .args(["--append-system-prompt", SYSTEM]);
        if let Some(id) = &request.conversation {
            c.args(["--resume", id]);
        }
        if request.cwd.is_dir() {
            c.current_dir(&request.cwd);
        }
        c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        // Its own group, so stopping it stops the server it started too.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut c, 0);
        c
    }
}

/// The message Claude Code reads on its input: the words and the picture.
fn input_line(request: &Request) -> String {
    let mut content = vec![json!({"type": "text", "text": request.text})];
    if let Some(png) = &request.picture {
        let data = base64::engine::general_purpose::STANDARD.encode(png);
        content.push(json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": data}}));
    }
    json!({"type": "user", "message": {"role": "user", "content": content}}).to_string()
}

struct RunningChild {
    child: Arc<Mutex<Child>>,
    stopped: Arc<AtomicBool>,
}

impl Running for RunningChild {
    fn stop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        let mut child = self.child.lock().unwrap();
        #[cfg(unix)]
        // SAFETY: signals the group this child leads; a pid that has
        // already been reaped makes `kill` fail and nothing else.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGTERM);
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

impl Drop for RunningChild {
    fn drop(&mut self) {
        // A finished child is only reaped here; a running one is stopped.
        if !matches!(self.child.lock().unwrap().try_wait(), Ok(Some(_))) {
            self.stop();
        }
    }
}

impl Runner for ClaudeCode {
    fn name(&self) -> &'static str {
        "Claude Code"
    }

    fn available(&self) -> Availability {
        let out = match Command::new(&self.program).args(["auth", "status", "--json"]).stdin(Stdio::null()).output() {
            Ok(out) => out,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Availability::Missing(Failure::NotInstalled),
            Err(e) => return Availability::Missing(Failure::Other(format!("Claude Code could not be started: {e}"))),
        };
        // An older Claude Code without `auth status`: let the request say.
        match serde_json::from_slice::<Value>(&out.stdout).ok().and_then(|v| v.get("loggedIn").and_then(Value::as_bool)) {
            Some(false) => Availability::Missing(Failure::NotSignedIn),
            _ => Availability::Ready,
        }
    }

    fn start(&self, request: Request, events: Sender<Event>) -> Result<Box<dyn Running>, Failure> {
        let mut child = self.command(&request).spawn().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Failure::NotInstalled,
            _ => Failure::Other(format!("Claude Code could not be started: {e}")),
        })?;
        let (mut stdin, stdout, mut stderr) = (child.stdin.take().unwrap(), child.stdout.take().unwrap(), child.stderr.take().unwrap());
        let line = input_line(&request);
        // One message, then the end of input: it answers and exits. A
        // thread, so a picture larger than the pipe cannot hold the UI.
        std::thread::spawn(move || {
            let _ = writeln!(stdin, "{line}");
        });
        let errors = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        });
        let stopped: Arc<AtomicBool> = Arc::default();
        let child = Arc::new(Mutex::new(child));
        let (flag, waited) = (stopped.clone(), child.clone());
        std::thread::Builder::new()
            .name("lahza-agent".into())
            .spawn(move || {
                let mut ended = false;
                // `LAHZA_AGENT_LOG=<file>` keeps the stream as it came
                // (turns, refusals, the tools it was given), to read after.
                let mut log = std::env::var_os("LAHZA_AGENT_LOG").and_then(|p| std::fs::OpenOptions::new().create(true).append(true).open(p).ok());
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if let Some(log) = &mut log {
                        let _ = writeln!(log, "{line}");
                    }
                    for event in parse_line(&line) {
                        ended |= matches!(event, Event::Done { .. } | Event::Failed(_));
                        if events.send(event).is_err() {
                            return;
                        }
                    }
                }
                let said = errors.join().unwrap_or_default();
                let status = waited.lock().unwrap().wait().ok();
                if !ended && !flag.load(Ordering::SeqCst) {
                    let said = said.trim();
                    let tail: String = said.chars().rev().take(400).collect::<Vec<_>>().into_iter().rev().collect();
                    let failure = if signed_out(said) {
                        Failure::NotSignedIn
                    } else {
                        Failure::Other(match (tail.is_empty(), status) {
                            (false, _) => format!("Claude Code stopped: {tail}"),
                            (true, Some(s)) => format!("Claude Code stopped without an answer ({s})"),
                            (true, None) => "Claude Code stopped without an answer".into(),
                        })
                    };
                    let _ = events.send(Event::Failed(failure));
                }
            })
            .map_err(|e| Failure::Other(e.to_string()))?;
        Ok(Box::new(RunningChild { child, stopped }))
    }
}

fn signed_out(text: &str) -> bool {
    let t = text.to_lowercase();
    ["/login", "invalid api key", "not logged in", "authentication_error", "oauth token has expired"].iter().any(|s| t.contains(s))
}

/// The events in one line of Claude Code's `stream-json` output. Lines it
/// does not know (status, usage, anything added later) are skipped.
pub(crate) fn parse_line(line: &str) -> Vec<Event> {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return Vec::new() };
    let content = || v.pointer("/message/content").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut out = Vec::new();
    match v.get("type").and_then(Value::as_str).unwrap_or("") {
        "system" if v.get("subtype").and_then(Value::as_str) == Some("init") => {
            if let Some(id) = v.get("session_id").and_then(Value::as_str) {
                out.push(Event::Started { conversation: id.to_string() });
            }
        }
        // A subagent's messages are its own business.
        "assistant" | "user" if v.get("parent_tool_use_id").is_some_and(|p| !p.is_null()) => {}
        "assistant" => {
            for block in content() {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        let text = block.get("text").and_then(Value::as_str).unwrap_or("").trim();
                        if !text.is_empty() {
                            out.push(Event::Said(text.to_string()));
                        }
                    }
                    Some("tool_use") => {
                        let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                        let tool = name.rsplit("__").next().unwrap_or(name).to_string();
                        let text = describe(&tool, block.get("input").unwrap_or(&Value::Null));
                        out.push(Event::Step { id: block.get("id").and_then(Value::as_str).unwrap_or("").to_string(), tool, text });
                    }
                    _ => {}
                }
            }
        }
        "user" => {
            for block in content() {
                if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                    let failed = block.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                    let error = failed.then(|| first_line(&result_text(block.get("content").unwrap_or(&Value::Null))));
                    out.push(Event::StepDone { id: block.get("tool_use_id").and_then(Value::as_str).unwrap_or("").to_string(), error });
                }
            }
        }
        "result" => {
            let text = v.get("result").and_then(Value::as_str).unwrap_or("").trim().to_string();
            let failed = v.get("is_error").and_then(Value::as_bool).unwrap_or(false) || v.get("subtype").and_then(Value::as_str).is_some_and(|s| s != "success");
            if failed && signed_out(&text) {
                out.push(Event::Failed(Failure::NotSignedIn));
            } else {
                let text = if text.is_empty() && failed { format!("Stopped: {}", v.get("subtype").and_then(Value::as_str).unwrap_or("an error").replace('_', " ")) } else { text };
                out.push(Event::Done { text, failed, cost: v.get("total_cost_usd").and_then(Value::as_f64) });
            }
        }
        _ => {}
    }
    out
}

fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join(" "),
        _ => String::new(),
    }
}

fn first_line(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if line.chars().count() > 160 {
        format!("{}…", line.chars().take(160).collect::<String>())
    } else {
        line.to_string()
    }
}

/// A tool call in the editor's words: what was added, what was checked,
/// what is being drawn.
pub(crate) fn describe(tool: &str, input: &Value) -> String {
    // (A list of times reads as "0.5, 2": not as JSON.)
    let text = |key: &str| {
        input.get(key).map(|v| match v {
            Value::String(s) => s.clone(),
            Value::Array(list) => list.iter().map(|v| v.as_str().map(String::from).unwrap_or_else(|| v.to_string())).collect::<Vec<_>>().join(", "),
            other => other.to_string(),
        })
    };
    match tool {
        "edit_apply" => {
            let ops = input.get("ops").and_then(Value::as_array).cloned().unwrap_or_default();
            let mut parts: Vec<String> = ops
                .iter()
                .take(3)
                .map(|op| {
                    let name = op.get("op").and_then(Value::as_str).unwrap_or("edit");
                    let what = ["target", "id"].iter().find_map(|k| op.get(*k).and_then(Value::as_str)).unwrap_or("");
                    match name {
                        "add" => format!("add {what}{}", op.get("kind").and_then(Value::as_str).map(|k| format!(" ({k})")).unwrap_or_default()),
                        "add_scene" => format!("add scene {what}"),
                        "add_sound" => format!("add sound {what}"),
                        "key" => format!("key {what}"),
                        "anchor" => format!("retime {what}"),
                        "insert_time" => "insert time".into(),
                        other => format!("{other} {what}").trim().to_string(),
                    }
                })
                .collect();
            if ops.len() > 3 {
                parts.push(format!("and {} more", ops.len() - 3));
            }
            if parts.is_empty() { "Editing".into() } else { format!("Editing: {}", parts.join(", ")) }
        }
        "snapshot" => format!("Looking at the frame{}", text("at").map(|t| format!(" at {t}")).unwrap_or_default()),
        "review" => "Checking the motion".into(),
        "check" => "Checking for problems".into(),
        "compare" => "Comparing with the reference".into(),
        "render" => "Drawing the video".into(),
        "render_status" => "Waiting for the drawing".into(),
        "timeline_get" | "project_info" | "project_open" | "recording_info" => "Reading the project".into(),
        "history" if input.get("undo").is_some() || input.get("undo_to").is_some() => "Taking back an edit".into(),
        "history" => "Reading the history".into(),
        "project_create" => "Making a motion piece".into(),
        "recording_place" => "Placing it on the recording".into(),
        "sfx_add" | "recording_sound_add" => format!("Adding a sound{}", text("sound").map(|s| format!(" ({s})")).unwrap_or_default()),
        "media_import" | "model_import" => format!("Bringing in {}", text("path").unwrap_or_else(|| "a file".into())),
        "shader_write" | "shader_check" => "Writing a shader".into(),
        other => {
            let mut words = other.replace('_', " ");
            if let Some(first) = words.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            words
        }
    }
}

// ---------------------------------------------------------------- context

/// What the words are about.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct About {
    /// The motion project's folder; None on a recording with no piece open.
    pub project: Option<PathBuf>,
    /// The recording the piece belongs to, or is to be made in.
    pub recording: Option<PathBuf>,
    /// Layers, with their kinds: "this one".
    pub layers: Vec<(String, String)>,
    /// A span of time in the project's (or the recording's) seconds.
    pub span: Option<(f64, f64)>,
    /// Playhead: seconds and frame.
    pub playhead: Option<(f64, i64)>,
    pub scene: Option<String>,
    pub has_picture: bool,
}

/// The message for the agent: what is pointed at, then the words.
pub(crate) fn compose(about: &About, words: &str) -> String {
    let mut lines = Vec::new();
    let show = |p: &Path| p.display().to_string();
    match (&about.project, &about.recording) {
        (Some(p), Some(r)) => lines.push(format!("Motion project: {} (a motion piece of the recording {}).", show(p), show(r))),
        (Some(p), None) => lines.push(format!("Motion project: {}.", show(p))),
        (None, Some(r)) => lines.push(format!(
            "Recording: {}. No motion piece is open: make a new one for this (project_create with \"recording\"), build it, and place it on the video with recording_place{}.",
            show(r),
            about.span.map(|(a, _)| format!(" at start {a:.2}")).unwrap_or_default()
        )),
        (None, None) => {}
    }
    // (The server starts with no project open: a real request spent its
    // first two calls finding that out.)
    if about.project.is_some() {
        lines.push("Open it first (project_open with that path); the tools then work on it.".into());
    }
    if let Some((t, frame)) = about.playhead {
        lines.push(format!("Playhead: {t:.2} s (frame {frame}){}.", about.scene.as_ref().map(|s| format!(", in scene \"{s}\"")).unwrap_or_default()));
    }
    match about.layers.as_slice() {
        [] => {}
        [(id, kind)] => lines.push(format!("Selected layer (\"this one\"): {id} ({kind}).")),
        many => lines.push(format!("Selected layers (\"these\"): {}.", many.iter().map(|(id, kind)| format!("{id} ({kind})")).collect::<Vec<_>>().join(", "))),
    }
    if let Some((a, b)) = about.span {
        lines.push(format!("Marked span (\"here\"): {a:.2} s to {b:.2} s ({:.2} s).", b - a));
    }
    if about.has_picture {
        lines.push("The picture is the frame at the playhead as it is now.".into());
    }
    lines.push(String::new());
    lines.push(words.trim().to_string());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    /// Lines as Claude Code 2.1 writes them (cut down to what is read),
    /// with kinds of line the reader has to skip.
    const STREAM: &str = r##"{"type":"active_goal","value":null,"session_id":"c0ffee"}
{"type":"system","subtype":"init","cwd":"/tmp","session_id":"c0ffee","tools":["mcp__lahza__edit_apply","mcp__lahza__snapshot"],"mcp_servers":[{"name":"lahza","status":"connected"}]}
{"type":"system","subtype":"status","status":"requesting","session_id":"c0ffee"}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"I'll look at the title first."},{"type":"tool_use","id":"t1","name":"mcp__lahza__snapshot","input":{"at":2.1}}]},"parent_tool_use_id":null,"session_id":"c0ffee"}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAA"}}]}]},"parent_tool_use_id":null,"session_id":"c0ffee"}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t2","name":"mcp__lahza__edit_apply","input":{"ops":[{"op":"set","target":"title.transform.position","value":[960,400]},{"op":"key","target":"title.transform.opacity","at":2.1,"v":1},{"op":"add","id":"underline","kind":"shape"},{"op":"remove","id":"old"}]}}]},"parent_tool_use_id":null,"session_id":"c0ffee"}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t2","is_error":true,"content":[{"type":"text","text":"op 4 (remove): no layer or scene with id 'old'\nmore"}]}]},"parent_tool_use_id":null,"session_id":"c0ffee"}
not json at all
{"type":"result","subtype":"success","is_error":false,"result":"Moved the title up and faded it in.","session_id":"c0ffee","total_cost_usd":0.0123,"num_turns":3}
"##;

    #[test]
    fn the_stream_becomes_steps() {
        let events: Vec<Event> = STREAM.lines().flat_map(parse_line).collect();
        assert_eq!(
            events,
            vec![
                Event::Started { conversation: "c0ffee".into() },
                Event::Said("I'll look at the title first.".into()),
                Event::Step { id: "t1".into(), tool: "snapshot".into(), text: "Looking at the frame at 2.1".into() },
                Event::StepDone { id: "t1".into(), error: None },
                Event::Step { id: "t2".into(), tool: "edit_apply".into(), text: "Editing: set title.transform.position, key title.transform.opacity, add underline (shape), and 1 more".into() },
                Event::StepDone { id: "t2".into(), error: Some("op 4 (remove): no layer or scene with id 'old'".into()) },
                Event::Done { text: "Moved the title up and faded it in.".into(), failed: false, cost: Some(0.0123) },
            ]
        );
        // An error at the end, and one that means "sign in".
        assert_eq!(
            parse_line(r#"{"type":"result","subtype":"error_max_turns","is_error":true,"session_id":"x"}"#),
            vec![Event::Done { text: "Stopped: error max turns".into(), failed: true, cost: None }]
        );
        assert_eq!(parse_line(r#"{"type":"result","subtype":"success","is_error":true,"result":"Invalid API key · Please run /login"}"#), vec![Event::Failed(Failure::NotSignedIn)]);
        assert_eq!(describe("fit_camera", &Value::Null), "Fit camera");
    }

    /// A program that stands in for `claude`: it keeps what it was given
    /// (arguments, input) beside itself and writes `body` as its stream.
    fn stub(name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("lahza-agent-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("claude");
        std::fs::write(&path, format!("#!/bin/sh\nif [ \"$1\" = auth ]; then cat \"$0.auth\" 2>/dev/null; exit 0; fi\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$0.args\"\ncat > \"$0.stdin\"\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// Starts a stub. Tests run side by side: another one forking while
    /// this one's script was still open for writing makes the kernel
    /// refuse to run it for a moment ("Text file busy").
    fn start(runner: &ClaudeCode, request: Request) -> (Box<dyn Running>, std::sync::mpsc::Receiver<Event>) {
        for _ in 0..200 {
            let (tx, rx) = channel();
            match runner.start(request.clone(), tx) {
                Ok(running) => return (running, rx),
                Err(Failure::Other(e)) if e.contains("Text file busy") => std::thread::sleep(Duration::from_millis(10)),
                Err(e) => panic!("{e:?}"),
            }
        }
        panic!("the stub never became runnable");
    }

    fn collect(rx: &std::sync::mpsc::Receiver<Event>) -> Vec<Event> {
        let mut out = Vec::new();
        while let Ok(e) = rx.recv_timeout(Duration::from_secs(20)) {
            let last = matches!(e, Event::Done { .. } | Event::Failed(_));
            out.push(e);
            if last {
                break;
            }
        }
        out
    }

    #[test]
    fn claude_code_is_started_on_our_server_and_read_as_it_comes() {
        let program = stub("run", &format!("cat <<'LAHZA_EOF'\n{STREAM}LAHZA_EOF"));
        let runner = ClaudeCode { program: program.clone(), server: "/opt/lahza".into() };
        let request = Request { id: "r7".into(), text: "Selected layer: title\n\nmove it up".into(), picture: Some(vec![1, 2, 3]), cwd: std::env::temp_dir(), conversation: Some("earlier".into()) };
        let (_running, rx) = start(&runner, request);
        let events = collect(&rx);
        assert_eq!(events.first(), Some(&Event::Started { conversation: "c0ffee".into() }));
        assert_eq!(events.last(), Some(&Event::Done { text: "Moved the title up and faded it in.".into(), failed: false, cost: Some(0.0123) }));
        assert_eq!(events.iter().filter(|e| matches!(e, Event::Step { .. })).count(), 2);
        // What it was started with: our server only, the request's id on
        // it, the earlier conversation, and nothing asked at a terminal.
        let args: Vec<String> = std::fs::read_to_string(program.with_extension("args")).unwrap().lines().map(String::from).collect();
        let after = |flag: &str| args.iter().position(|a| a == flag).map(|i| args[i + 1].clone()).unwrap_or_else(|| panic!("{flag} in {args:?}"));
        let servers: Value = serde_json::from_str(&after("--mcp-config")).unwrap();
        let server = &servers["mcpServers"]["lahza"];
        assert_eq!((&server["command"], &server["args"], &server["env"]["LAHZA_REQUEST"]), (&json!("/opt/lahza"), &json!(["mcp"]), &json!("r7")));
        assert!(args.contains(&"--strict-mcp-config".to_string()) && args.contains(&"-p".to_string()));
        assert_eq!((after("--output-format"), after("--input-format"), after("--resume")), ("stream-json".into(), "stream-json".into(), "earlier".into()));
        assert_eq!((after("--tools"), after("--allowedTools"), after("--permission-mode")), (String::new(), "mcp__lahza".into(), "dontAsk".into()));
        // What it was told: the words and the picture.
        let said: Value = serde_json::from_str(std::fs::read_to_string(program.with_extension("stdin")).unwrap().trim()).unwrap();
        assert_eq!(said["message"]["content"][0]["text"], "Selected layer: title\n\nmove it up");
        assert_eq!(said["message"]["content"][1]["source"], json!({"type": "base64", "media_type": "image/png", "data": "AQID"}));
        std::fs::remove_dir_all(program.parent().unwrap()).ok();
    }

    #[test]
    fn stop_ends_the_child() {
        let program = stub("stop", "echo $$ > \"$0.pid\"\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s\"}'\nsleep 600");
        let runner = ClaudeCode { program: program.clone(), server: "lahza".into() };
        let (mut running, rx) = start(&runner, Request { id: "r".into(), cwd: std::env::temp_dir(), ..Default::default() });
        assert_eq!(rx.recv_timeout(Duration::from_secs(20)).unwrap(), Event::Started { conversation: "s".into() });
        let pid: i32 = std::fs::read_to_string(program.with_extension("pid")).unwrap().trim().parse().unwrap();
        running.stop();
        // Nothing more arrives (no failure is reported for a stop), and
        // the process is gone.
        assert!(rx.recv_timeout(Duration::from_secs(20)).is_err());
        assert!(!Path::new(&format!("/proc/{pid}")).exists(), "the child ended");
        std::fs::remove_dir_all(program.parent().unwrap()).ok();
    }

    #[test]
    fn missing_or_signed_out_is_said() {
        let none = ClaudeCode { program: "/nonexistent/claude".into(), server: "lahza".into() };
        assert_eq!(none.available(), Availability::Missing(Failure::NotInstalled));
        let (tx, _rx) = channel();
        assert_eq!(none.start(Request::default(), tx).err(), Some(Failure::NotInstalled));
        assert!(Failure::NotInstalled.message("Claude Code").contains("not installed"));
        // `claude auth status` answers whether it is signed in.
        let program = stub("auth", "echo 'Invalid API key · Please run /login' >&2\nexit 1");
        let runner = ClaudeCode { program: program.clone(), server: "lahza".into() };
        // The stub answers `auth status` with the file beside it.
        let available = || loop {
            match runner.available() {
                Availability::Missing(Failure::Other(e)) if e.contains("Text file busy") => std::thread::sleep(Duration::from_millis(10)),
                answer => break answer,
            }
        };
        std::fs::write(program.with_extension("auth"), r#"{"loggedIn": false, "authMethod": "none"}"#).unwrap();
        assert_eq!(available(), Availability::Missing(Failure::NotSignedIn));
        std::fs::write(program.with_extension("auth"), r#"{"loggedIn": true, "authMethod": "oauth_token"}"#).unwrap();
        assert_eq!(available(), Availability::Ready);
        // With no answer (an older Claude Code) it is taken as ready, and
        // the request itself says what is wrong: here, that it is not
        // signed in.
        std::fs::remove_file(program.with_extension("auth")).unwrap();
        assert_eq!(available(), Availability::Ready);
        let (_running, rx) = start(&runner, Request { id: "r".into(), cwd: std::env::temp_dir(), ..Default::default() });
        assert_eq!(collect(&rx), vec![Event::Failed(Failure::NotSignedIn)]);
        std::fs::remove_dir_all(program.parent().unwrap()).ok();
    }

    /// A runner that can or cannot run, and keeps what it was started with.
    struct Fake(&'static str, Availability, Mutex<Vec<Request>>);

    struct Nothing;

    impl Running for Nothing {
        fn stop(&mut self) {}
    }

    impl Runner for Fake {
        fn name(&self) -> &'static str {
            self.0
        }
        fn available(&self) -> Availability {
            self.1.clone()
        }
        fn start(&self, request: Request, _: Sender<Event>) -> Result<Box<dyn Running>, Failure> {
            self.2.lock().unwrap().push(request);
            Ok(Box::new(Nothing))
        }
    }

    #[test]
    fn the_runner_is_claude_code_where_it_can_run_else_the_api_where_a_key_is_set() {
        let fake = |name, availability| Arc::new(Fake(name, availability, Mutex::default()));
        let chosen = |claude: &Arc<Fake>, api: &Arc<Fake>, choice: Choice| Chosen::new(claude.clone(), api.clone(), Box::new(move || choice));
        let (ready, no_claude, no_key) = (Availability::Ready, Availability::Missing(Failure::NotInstalled), Availability::Missing(Failure::NoKey));
        let start = |c: &Chosen, conversation: Option<&str>| {
            let (tx, _rx) = channel();
            c.start(Request { conversation: conversation.map(String::from), ..Default::default() }, tx).ok().map(|_| ())
        };
        // Both can: Claude Code.
        let (claude, api) = (fake("Claude Code", ready.clone()), fake("Claude API", ready.clone()));
        let c = chosen(&claude, &api, Choice::Auto);
        assert_eq!(c.name(), "The agent", "not settled yet");
        assert_eq!((c.available(), c.name()), (ready.clone(), "Claude Code"));
        start(&c, Some("api-1")).unwrap();
        assert_eq!((claude.2.lock().unwrap().len(), api.2.lock().unwrap().len()), (1, 0));
        assert!(claude.2.lock().unwrap()[0].conversation.is_none(), "the API loop's conversation is not Claude Code's");
        // Asked for by name.
        let c = chosen(&claude, &api, Choice::Api);
        assert_eq!((c.available(), c.name()), (ready.clone(), "Claude API"));
        start(&c, Some("c0ffee")).unwrap();
        start(&c, Some("api-1")).unwrap();
        let asked: Vec<Option<String>> = api.2.lock().unwrap().iter().map(|r| r.conversation.clone()).collect();
        assert_eq!(asked, [None, Some("api-1".to_string())]);
        // No Claude Code, a key: the API loop, without being asked first.
        let (claude, api) = (fake("Claude Code", no_claude.clone()), fake("Claude API", ready.clone()));
        let c = chosen(&claude, &api, Choice::Auto);
        start(&c, None).unwrap();
        assert_eq!((c.name(), api.2.lock().unwrap().len()), ("Claude API", 1));
        // Neither: both are said, so either can be put right.
        let (claude, api) = (fake("Claude Code", no_claude.clone()), fake("Claude API", no_key.clone()));
        let c = chosen(&claude, &api, Choice::Auto);
        let Availability::Missing(why) = c.available() else { panic!() };
        let why = why.message(c.name());
        assert!(why.contains("Claude Code is not installed") && why.contains("Or: Claude API has no API key"), "{why}");
        // Chosen by name and missing: only that one's reason.
        assert_eq!(chosen(&claude, &api, Choice::Api).available(), no_key);
        assert_eq!(chosen(&claude, &api, Choice::ClaudeCode).available(), no_claude);
        // The choice is kept.
        let file = std::env::temp_dir().join(format!("lahza-agent-choice-{}", std::process::id())).join("motion-agent");
        assert_eq!(read_choice(&file), Choice::Auto);
        for choice in Choice::ALL {
            write_choice(&file, choice);
            assert_eq!(read_choice(&file), choice);
        }
        std::fs::remove_dir_all(file.parent().unwrap()).ok();
    }

    #[test]
    fn the_words_go_with_what_they_are_about() {
        let about = About {
            project: Some("/work/promo.lahza".into()),
            recording: None,
            layers: vec![("title".into(), "text".into())],
            span: Some((2.1, 7.1)),
            playhead: Some((2.1, 63)),
            scene: Some("s1".into()),
            has_picture: true,
        };
        assert_eq!(
            compose(&about, "  make this one bolder "),
            "Motion project: /work/promo.lahza.\nOpen it first (project_open with that path); the tools then work on it.\nPlayhead: 2.10 s (frame 63), in scene \"s1\".\nSelected layer (\"this one\"): title (text).\nMarked span (\"here\"): 2.10 s to 7.10 s (5.00 s).\nThe picture is the frame at the playhead as it is now.\n\nmake this one bolder"
        );
        let recording = About { recording: Some("/work/talk.lahzarec".into()), span: Some((4.0, 9.0)), ..Default::default() };
        let text = compose(&recording, "five seconds of a map here");
        assert!(text.contains("project_create") && text.contains("recording_place at start 4.00"), "{text}");
        let several = About { layers: vec![("a".into(), "shape".into()), ("b".into(), "text".into())], ..Default::default() };
        assert!(compose(&several, "align").starts_with("Selected layers (\"these\"): a (shape), b (text)."));
    }
}
