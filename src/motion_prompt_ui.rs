//! Directing an agent from the editor: one prompt bar under the preview
//! (the same on a motion project and on a recording's Motion tab), the
//! requests made with it, and what the studio shows of an agent at work.
//!
//! The rule: select, type, Enter. The bar shows what the words will act on
//! as chips (layers, a span of time, the conversation being continued);
//! removing a chip leaves it out. What comes back is a proposal: keep it,
//! undo all of it, ask again, or hold Compare to see before and after.
//! The editor stays usable while a request runs: the agent's server and
//! the editor take the project's lock to write (`io::lock`).

use crate::controls::{button, ButtonKind, ButtonSize};
use crate::motion_agent::{compose, About, Availability, ClaudeCode, Event, Failure, Request, Runner, Running};
use crate::motion_timeline::LayerDrag;
use crate::text_field::{EventKind, FieldEvent, Target};
use crate::theme;
use crate::Studio;
use gpui::{div, prelude::*, px, AnyElement, Context, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent, SharedString, Timer, Window};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Height of the prompt bar: chips, the field, and a line of status.
pub(crate) const PROMPT_BAR_HEIGHT: f32 = 112.0;

/// Where a project's (or a recording's) requests are kept between runs
/// of the app.
const REQUESTS_FILE: &str = "requests.json";

/// One thing the agent did.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Step {
    id: String,
    pub text: String,
    pub done: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) enum Outcome {
    Running,
    /// Done, with edits to keep or undo.
    Proposal,
    Kept,
    Undone,
    Stopped,
    Failed(String),
}

/// A request as it is kept on disk.
#[derive(serde::Serialize, serde::Deserialize)]
struct Kept {
    id: String,
    words: String,
    about: About,
    steps: Vec<Step>,
    said: String,
    outcome: Outcome,
    edits: usize,
    seconds: f64,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct KeptRequests {
    /// The conversation a follow-up continues.
    conversation: Option<String>,
    requests: Vec<Kept>,
}

/// One prompt and what came of it.
pub(crate) struct AgentRequest {
    pub id: String,
    pub words: String,
    pub about: About,
    /// Where the conversation is kept: the project, or the recording.
    key: PathBuf,
    pub steps: Vec<Step>,
    /// What it said last.
    pub said: String,
    pub outcome: Outcome,
    /// Edits of this request in effect (counted when it ended).
    pub edits: usize,
    started: Instant,
    seconds: Option<f64>,
}

impl AgentRequest {
    pub(crate) fn seconds(&self) -> f64 {
        self.seconds.unwrap_or_else(|| self.started.elapsed().as_secs_f64())
    }

    /// The step it is at, in a few words.
    pub(crate) fn now(&self) -> String {
        match self.steps.iter().rev().find(|s| !s.done).or(self.steps.last()) {
            Some(step) if !step.done => step.text.clone(),
            _ if self.steps.is_empty() => "Starting".into(),
            _ => "Thinking".into(),
        }
    }

    /// Whether there is something to keep or undo.
    pub(crate) fn decidable(&self) -> bool {
        self.edits > 0 && matches!(self.outcome, Outcome::Proposal | Outcome::Stopped | Outcome::Failed(_))
    }

    /// Whether it was about `layer` (None: about no layer in particular).
    pub(crate) fn concerns(&self, layer: Option<&str>) -> bool {
        match layer {
            Some(id) => self.about.layers.iter().any(|(l, _)| l == id),
            None => true,
        }
    }

    fn apply(&mut self, event: &Event) {
        match event {
            Event::Started { .. } => {}
            Event::Step { id, text, .. } => self.steps.push(Step { id: id.clone(), text: text.clone(), done: false, error: None }),
            Event::StepDone { id, error } => {
                if let Some(step) = self.steps.iter_mut().find(|s| &s.id == id) {
                    step.done = true;
                    step.error = error.clone();
                }
            }
            Event::Said(text) => self.said = text.clone(),
            Event::Done { text, failed, .. } => {
                if !text.is_empty() {
                    self.said = text.clone();
                }
                self.end(if *failed { Outcome::Failed(self.said.clone()) } else { Outcome::Proposal });
            }
            Event::Failed(_) => {}
        }
    }

    fn end(&mut self, outcome: Outcome) {
        for step in &mut self.steps {
            step.done = true;
        }
        self.seconds.get_or_insert(self.started.elapsed().as_secs_f64());
        self.outcome = outcome;
    }
}

/// The studio's agent: the runner, the requests made here, and what the
/// next prompt will go with.
pub(crate) struct MotionAgent {
    runner: Arc<dyn Runner>,
    availability: Arc<Mutex<Option<Availability>>>,
    asked: bool,
    pub requests: Vec<AgentRequest>,
    running: Option<(Box<dyn Running>, Receiver<Event>)>,
    /// The conversation each project (or recording) is in.
    conversations: HashMap<PathBuf, String>,
    /// The words being typed.
    pub draft: String,
    /// A span marked on the ruler, in timeline seconds.
    pub span: Option<(f64, f64)>,
    /// Layers selected besides the editor's one.
    pub also: Vec<String>,
    /// The next prompt starts a new conversation.
    pub fresh: bool,
    /// Before/after is showing "before" for this request.
    comparing: Option<String>,
    /// The inspector shows the AI tab.
    pub ai_tab: bool,
    /// Folders whose kept requests were read.
    loaded: Vec<PathBuf>,
}

impl Default for MotionAgent {
    fn default() -> Self {
        Self::new(Arc::new(ClaudeCode::default()))
    }
}

impl MotionAgent {
    pub(crate) fn new(runner: Arc<dyn Runner>) -> Self {
        Self {
            runner,
            availability: Arc::default(),
            asked: false,
            requests: Vec::new(),
            running: None,
            conversations: HashMap::new(),
            draft: String::new(),
            span: None,
            also: Vec::new(),
            fresh: false,
            comparing: None,
            ai_tab: false,
            loaded: Vec::new(),
        }
    }

    /// Reads the requests kept in `key` (a project's or a recording's
    /// folder), once: the AI tab and the conversation survive the app.
    pub(crate) fn load(&mut self, key: &Path) {
        if self.loaded.iter().any(|k| k == key) {
            return;
        }
        self.loaded.push(key.to_path_buf());
        let Some(kept) = std::fs::read(key.join(REQUESTS_FILE)).ok().and_then(|b| serde_json::from_slice::<KeptRequests>(&b).ok()) else { return };
        if let Some(conversation) = kept.conversation {
            self.conversations.entry(key.to_path_buf()).or_insert(conversation);
        }
        let at = self.requests.len().saturating_sub(usize::from(self.busy()));
        let old = kept.requests.into_iter().filter(|k| !self.requests.iter().any(|r| r.id == k.id)).map(|k| AgentRequest {
            id: k.id,
            words: k.words,
            about: k.about,
            key: key.to_path_buf(),
            steps: k.steps,
            said: k.said,
            // The app closed while it ran.
            outcome: if k.outcome == Outcome::Running { Outcome::Stopped } else { k.outcome },
            edits: k.edits,
            started: Instant::now(),
            seconds: Some(k.seconds),
        });
        // Before a request that is running now, which stays the last.
        self.requests.splice(at..at, old.collect::<Vec<_>>());
    }

    /// Writes the requests of `key` beside it. A failure to write only
    /// loses the record.
    pub(crate) fn save(&self, key: &Path) {
        let kept = KeptRequests {
            conversation: self.conversations.get(key).cloned(),
            requests: self
                .requests
                .iter()
                .filter(|r| r.key == key)
                .map(|r| Kept { id: r.id.clone(), words: r.words.clone(), about: r.about.clone(), steps: r.steps.clone(), said: r.said.clone(), outcome: r.outcome.clone(), edits: r.edits, seconds: r.seconds() })
                .collect(),
        };
        if let Ok(text) = serde_json::to_string_pretty(&kept) {
            let _ = lahza_motion_core::io::write_atomic(&key.join(REQUESTS_FILE), text.as_bytes());
        }
    }

    /// Saves the record of the request with `id`.
    fn save_request(&self, id: &str) {
        if let Some(r) = self.requests.iter().find(|r| r.id == id) {
            self.save(&r.key);
        }
    }

    /// Asks once, off the UI thread, whether the runner can run here.
    fn ask_available(&mut self) {
        if std::mem::replace(&mut self.asked, true) {
            return;
        }
        let (runner, slot) = (self.runner.clone(), self.availability.clone());
        std::thread::spawn(move || {
            let answer = runner.available();
            *slot.lock().unwrap() = Some(answer);
        });
    }

    /// Why prompts cannot be sent, when they cannot.
    pub(crate) fn missing(&self) -> Option<String> {
        match &*self.availability.lock().unwrap() {
            Some(Availability::Missing(failure)) => Some(failure.message(self.runner.name())),
            _ => None,
        }
    }

    pub(crate) fn busy(&self) -> bool {
        self.running.is_some()
    }

    /// Whether a prompt about `key` continues a conversation.
    fn continues(&self, key: &Path) -> bool {
        !self.fresh && self.conversations.contains_key(key)
    }

    /// Starts a request: `words` about `about`, with the frame as `picture`.
    pub(crate) fn send(&mut self, about: About, words: &str, picture: Option<Vec<u8>>, key: PathBuf, cwd: PathBuf) {
        if self.busy() || words.trim().is_empty() {
            return;
        }
        let id = format!("r{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
        let about = About { has_picture: picture.is_some(), ..about };
        let request = Request { id: id.clone(), text: compose(&about, words), picture, cwd, conversation: (!self.fresh).then(|| self.conversations.get(&key).cloned()).flatten() };
        let mut entry = AgentRequest { id, words: words.trim().to_string(), about, key, steps: Vec::new(), said: String::new(), outcome: Outcome::Running, edits: 0, started: Instant::now(), seconds: None };
        let (tx, rx) = channel();
        match self.runner.start(request, tx) {
            Ok(running) => self.running = Some((running, rx)),
            Err(failure) => {
                entry.end(Outcome::Failed(failure.message(self.runner.name())));
                self.note(failure);
            }
        }
        self.fresh = false;
        self.draft.clear();
        self.requests.push(entry);
    }

    fn note(&mut self, failure: Failure) {
        if matches!(failure, Failure::NotInstalled | Failure::NotSignedIn) {
            *self.availability.lock().unwrap() = Some(Availability::Missing(failure));
        }
    }

    /// Takes what the runner reported since the last call. Returns whether
    /// anything changed, and the request that just ended.
    pub(crate) fn poll(&mut self) -> (bool, Option<usize>) {
        let Some((_, rx)) = &self.running else { return (false, None) };
        let events: Vec<Event> = rx.try_iter().collect();
        let index = self.requests.len().saturating_sub(1);
        let mut ended = false;
        for event in &events {
            let Some(request) = self.requests.last_mut() else { break };
            request.apply(event);
            match event {
                Event::Started { conversation } => {
                    self.conversations.insert(request.key.clone(), conversation.clone());
                }
                Event::Done { .. } => ended = true,
                Event::Failed(failure) => {
                    request.end(Outcome::Failed(failure.message(self.runner.name())));
                    self.note(failure.clone());
                    ended = true;
                }
                _ => {}
            }
        }
        if ended {
            self.running = None;
        }
        (!events.is_empty(), ended.then_some(index))
    }

    /// Ends the agent now; what it changed so far stays, to keep or undo.
    pub(crate) fn stop(&mut self) -> Option<usize> {
        let (mut running, _) = self.running.take()?;
        running.stop();
        let request = self.requests.last_mut()?;
        request.end(Outcome::Stopped);
        Some(self.requests.len() - 1)
    }

    /// The request waiting for keep or undo.
    pub(crate) fn proposal(&self) -> Option<&AgentRequest> {
        self.requests.last().filter(|r| r.decidable())
    }
}

/// Edits of `request` in effect in the project at `dir`.
fn request_edits(dir: &Path, request: &str) -> usize {
    lahza_motion_core::io::read_history(dir).map(|h| h.iter().filter(|e| e.request.as_deref() == Some(request) && !e.undone).count()).unwrap_or(0)
}

impl Studio {
    fn motion_editor(&self) -> Option<gpui::Entity<crate::motion_editor::MotionEditor>> {
        self.motion_doc.as_ref().map(|d| d.editor.clone())
    }

    /// Whether the prompt bar shows: wherever motion is edited.
    pub(crate) fn motion_prompt_visible(&self) -> bool {
        self.motion_editing() && !self.crop_active
    }

    pub(crate) fn motion_prompt_height(&self) -> f32 {
        if self.motion_prompt_visible() { PROMPT_BAR_HEIGHT } else { 0.0 }
    }

    /// What a prompt sent now would be about, where its conversation is
    /// kept, and the folder the agent works in.
    fn motion_about(&self, cx: &gpui::App) -> Option<(About, PathBuf, PathBuf)> {
        let recording = self.motion_project.is_none().then(|| self.video_project.as_ref().map(|s| s.directory.clone())).flatten();
        let mut about = About { recording: recording.clone(), ..Default::default() };
        let offset = self.motion_doc_start().unwrap_or(0.0);
        if let Some(editor) = self.motion_editor() {
            let editor = editor.read(cx);
            about.project = Some(editor.dir().to_path_buf());
            about.playhead = Some((editor.time(), editor.frame_at_playhead()));
            about.scene = editor.scene_at_playhead();
            let ids = editor.selected().into_iter().map(str::to_string).chain(self.motion_agent.also.iter().cloned());
            for id in ids {
                if let Some(layer) = editor.layer(&id) {
                    about.layers.push((id, layer.kind.name().to_string()));
                } else if editor.sound(&id).is_some() {
                    about.layers.push((id, "sound".into()));
                }
            }
            let length = editor.project().settings.duration;
            about.span = self.motion_agent.span.map(|(a, b)| ((a - offset).clamp(0.0, length), (b - offset).clamp(0.0, length))).filter(|(a, b)| b > a);
        } else {
            about.playhead = Some((self.video_position, 0));
            about.span = self.motion_agent.span;
        }
        let key = about.project.clone().or(recording)?;
        let cwd = key.parent().map(Path::to_path_buf).unwrap_or_default();
        Some((about, key, cwd))
    }

    /// Enter in the prompt bar: the words go to the agent with what is
    /// selected, the span, the playhead and the frame as it is.
    pub(crate) fn send_motion_prompt(&mut self, words: &str, cx: &mut Context<Self>) {
        if self.motion_agent.busy() || words.trim().is_empty() {
            return;
        }
        if let Some(why) = self.motion_agent.missing() {
            self.toast = Some(crate::Notification::error(why));
            return;
        }
        let Some((about, key, cwd)) = self.motion_about(cx) else { return };
        let picture = self.motion_editor().and_then(|e| e.read(cx).frame_png());
        self.motion_agent.send(about, words, picture, key, cwd);
        // The field is emptied even while it has the keys (Send clicked).
        self.text_fields.prompt.update(cx, |field, cx| field.set_text("", cx));
        if let Some(index) = self.motion_agent.requests.len().checked_sub(1).filter(|_| !self.motion_agent.busy()) {
            // It could not start: say why.
            if let Outcome::Failed(why) = &self.motion_agent.requests[index].outcome {
                self.toast = Some(crate::Notification::error(why.clone()));
            }
        }
        self.watch_motion_agent(cx);
        cx.notify();
    }

    /// Follows the running request: steps as they come, the clock, the end.
    fn watch_motion_agent(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |weak, cx| loop {
            Timer::after(Duration::from_millis(120)).await;
            let alive = weak.update(cx, |this, cx| {
                let (_, ended) = this.motion_agent.poll();
                if let Some(index) = ended {
                    this.end_motion_request(index);
                }
                cx.notify();
                this.motion_agent.busy()
            });
            if !matches!(alive, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    /// A request ended: count what it changed. With nothing changed there
    /// is nothing to decide.
    fn end_motion_request(&mut self, index: usize) {
        let recording = self.video_project.as_ref().map(|s| s.directory.clone());
        let Some(request) = self.motion_agent.requests.get_mut(index) else { return };
        if request.about.project.is_none() {
            // Made on a recording with no piece open: find the piece the
            // agent made, by the request its history carries.
            let made = recording.iter().flat_map(|r| lahza_motion_core::recording::read_pieces(r).into_iter().map(move |p| p.dir(r))).find(|dir| request_edits(dir, &request.id) > 0);
            request.about.project = made;
        }
        request.edits = request.about.project.as_deref().map_or(0, |dir| request_edits(dir, &request.id));
        if request.outcome == Outcome::Proposal && request.edits == 0 {
            request.outcome = Outcome::Kept;
        }
        let key = request.key.clone();
        self.motion_agent.save(&key);
    }

    pub(crate) fn stop_motion_agent(&mut self, cx: &mut Context<Self>) {
        if let Some(index) = self.motion_agent.stop() {
            self.end_motion_request(index);
        }
        cx.notify();
    }

    pub(crate) fn keep_motion_request(&mut self, id: &str, cx: &mut Context<Self>) {
        self.compare_motion_request(None, cx);
        if let Some(r) = self.motion_agent.requests.iter_mut().find(|r| r.id == id) {
            r.outcome = Outcome::Kept;
        }
        self.motion_agent.save_request(id);
        cx.notify();
    }

    /// Takes back everything a request changed, as one undo.
    pub(crate) fn undo_motion_request(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        self.compare_motion_request(None, cx);
        let Some(dir) = self.motion_agent.requests.iter().find(|r| r.id == id).and_then(|r| r.about.project.clone()) else { return false };
        let result = match self.motion_editor().filter(|e| e.read(cx).dir() == dir) {
            Some(editor) => {
                let result = editor.update(cx, |e, _| e.undo_request(id));
                self.show_motion_doc(cx);
                result
            }
            // The piece is not the one open: through a toolbox of its own.
            None => lahza_motion_mcp::tools::Toolbox::new().call("history", json!({"project": dir, "undo_request": id})).map(|_| ()),
        };
        match result {
            Ok(()) => {
                if let Some(r) = self.motion_agent.requests.iter_mut().find(|r| r.id == id) {
                    r.outcome = Outcome::Undone;
                    r.edits = 0;
                }
                self.motion_agent.save_request(id);
            }
            Err(ref e) => self.toast = Some(crate::Notification::error(format!("Could not undo the request: {e}"))),
        }
        cx.notify();
        result.is_ok()
    }

    /// Undoes a request and asks for it again, in the same conversation.
    pub(crate) fn retry_motion_request(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(words) = self.motion_agent.requests.iter().find(|r| r.id == id).map(|r| r.words.clone()) else { return };
        if self.motion_agent.requests.iter().any(|r| r.id == id && r.edits > 0) && !self.undo_motion_request(id, cx) {
            return;
        }
        self.send_motion_prompt(&format!("{words}\n\n(Your last attempt at this was undone: do it again, differently.)"), cx);
    }

    /// Shows the project without a request's edits while held (`Some`),
    /// and as it is again on release (`None`). Nothing is saved.
    pub(crate) fn compare_motion_request(&mut self, id: Option<&str>, cx: &mut Context<Self>) {
        if self.motion_agent.comparing.as_deref() == id {
            return;
        }
        self.motion_agent.comparing = id.map(str::to_string);
        if let Some(editor) = self.motion_editor() {
            editor.update(cx, |e, _| e.compare(id));
            self.show_motion_doc(cx);
        }
        cx.notify();
    }

    /// The `\` key held compares the proposal: before while down.
    pub(crate) fn motion_compare_key(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.motion_agent.proposal().map(|r| r.id.clone()) else { return false };
        self.compare_motion_request(down.then_some(id.as_str()), cx);
        true
    }

    // ------------------------------------------------------- selection

    /// Ctrl/Shift-click on a row: one more layer for the prompt (or one
    /// fewer). The editor's own selection stays the layer the inspector
    /// shows.
    pub(crate) fn toggle_motion_layer_also(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(editor) = self.motion_editor() else { return };
        let primary = editor.read(cx).selected().map(str::to_string);
        if primary.is_none() {
            editor.update(cx, |e, _| e.select(Some(id)));
        } else if primary.as_deref() == Some(id.as_str()) {
            // The first of the others takes its place.
            let next = (!self.motion_agent.also.is_empty()).then(|| self.motion_agent.also.remove(0));
            editor.update(cx, |e, _| e.select(next));
        } else if let Some(i) = self.motion_agent.also.iter().position(|l| l == &id) {
            self.motion_agent.also.remove(i);
        } else {
            self.motion_agent.also.push(id);
        }
        cx.notify();
    }

    /// Whether `id` is one of the layers a prompt would be about.
    pub(crate) fn motion_layer_also(&self, id: &str) -> bool {
        self.motion_agent.also.iter().any(|l| l == id)
    }

    /// A press on the ruler's span band (or Shift on the ruler) starts
    /// marking a span.
    pub(crate) fn begin_motion_span(&mut self, x: gpui::Pixels) {
        if let Some(t) = self.motion_timeline_time_at(x) {
            self.motion_layer_drag = Some(LayerDrag::Span { anchor: t });
            self.motion_agent.span = None;
        }
    }

    pub(crate) fn update_motion_span(&mut self, anchor: f64, x: gpui::Pixels) -> bool {
        let Some(t) = self.motion_timeline_time_at(x) else { return false };
        let (a, b) = (anchor.min(t), anchor.max(t));
        // A click marks nothing (and clears the span).
        self.motion_agent.span = (b - a >= 0.05).then_some((a, b));
        true
    }

    // ---------------------------------------------------------- typing

    /// Binds the prompt bar's field and the inspector's text field.
    pub(crate) fn sync_motion_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let visible = self.motion_prompt_visible();
        if visible {
            self.motion_agent.ask_available();
            if let Some((_, key, _)) = self.motion_about(cx) {
                self.motion_agent.load(&key);
            }
        }
        let draft = self.motion_agent.draft.clone();
        self.text_fields.prompt.update(cx, |field, cx| field.sync(if visible { Target::Prompt } else { Target::None }, &draft, window, cx));
        let text = self.motion_text_layer(cx).filter(|_| visible && self.inspector_visible && !self.motion_agent.ai_tab);
        let (target, value) = text.map(|(_, text)| (Target::LayerText, text)).unwrap_or((Target::None, String::new()));
        self.text_fields.layer_text.update(cx, |field, cx| field.sync(target, &value, window, cx));
        // The colour being typed: of the selected layer only.
        let selected = self.motion_editor().and_then(|e| e.read(cx).selected().map(str::to_string));
        if self.motion_color_edit.as_ref().is_some_and(|(layer, _)| selected.as_deref() != Some(layer.as_str())) {
            self.motion_color_edit = None;
        }
        let color = self.motion_color_edit.clone().filter(|_| visible && self.inspector_visible && !self.motion_agent.ai_tab);
        let now = color.as_ref().and_then(|(layer, path)| self.motion_editor().map(|e| e.read(cx).color_state(layer, path)));
        let (target, value) = now.map(|hex| (Target::LayerColor, hex)).unwrap_or((Target::None, String::new()));
        self.text_fields.layer_color.update(cx, |field, cx| field.sync(target, &value, window, cx));
    }

    /// The selected layer's id and words, when it is a text.
    fn motion_text_layer(&self, cx: &gpui::App) -> Option<(String, String)> {
        let editor = self.motion_editor()?;
        let editor = editor.read(cx);
        let id = editor.selected()?;
        match &editor.layer(id)?.kind {
            lahza_motion_core::document::LayerKind::Text(t) => Some((id.to_string(), t.text.clone())),
            _ => None,
        }
    }

    pub(crate) fn on_motion_field_event(&mut self, event: &FieldEvent, cx: &mut Context<Self>) {
        match (event.target, event.kind) {
            (Target::Prompt, EventKind::Change | EventKind::Cancel) => self.motion_agent.draft = event.text.clone(),
            (Target::Prompt, EventKind::Submit) => {
                self.motion_agent.draft = event.text.clone();
                self.send_motion_prompt(&event.text, cx);
            }
            // A colour typed as #rrggbb (the # may be left out): saved on
            // Enter or when the field is left, if it changed.
            (Target::LayerColor, EventKind::Submit | EventKind::Commit) => {
                let (Some((layer, path)), Some(editor)) = (self.motion_color_edit.clone(), self.motion_editor()) else { return };
                let typed = event.text.trim().to_lowercase();
                let hex = if typed.starts_with('#') { typed } else { format!("#{typed}") };
                if hex != editor.read(cx).color_state(&layer, &path) {
                    editor.update(cx, |e, _| e.set_color(&layer, &path, &hex));
                    self.show_motion_doc(cx);
                }
            }
            // The words show as they are typed and are saved when the
            // field is left.
            (Target::LayerText, kind) => {
                let Some((id, saved)) = self.motion_text_layer(cx) else { return };
                let Some(editor) = self.motion_editor() else { return };
                let ops = vec![json!({"op": "set", "target": format!("{id}.text"), "value": event.text})];
                match kind {
                    EventKind::Change => editor.update(cx, |e, _| e.preview_ops(&ops)),
                    EventKind::Commit if event.text != saved => editor.update(cx, |e, _| e.commit_ops(ops)),
                    EventKind::Commit | EventKind::Cancel => editor.update(cx, |e, _| e.cancel_preview()),
                    EventKind::Focus | EventKind::Submit => {}
                }
                self.show_motion_doc(cx);
            }
            _ => {}
        }
    }

    /// A printable key pressed with a layer selected (on a motion project:
    /// at any time) begins a prompt with that key. False when the key is
    /// not text, or belongs to a shortcut.
    pub(crate) fn begin_prompt_by_typing(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let k = &event.keystroke;
        if !self.motion_prompt_visible() || k.modifiers.control || k.modifiers.platform || k.modifiers.alt || k.modifiers.function {
            return false;
        }
        let Some(text) = k.key_char.as_deref().filter(|t| !t.is_empty() && !t.chars().any(char::is_control) && *t != " " && *t != "\\") else { return false };
        let selected = self.motion_editor().is_some_and(|e| e.read(cx).selected().is_some());
        if !selected && self.motion_project.is_none() {
            return false;
        }
        self.pause_video_playback();
        let text = text.to_string();
        self.text_fields.prompt.update(cx, |field, cx| field.type_text(&text, window, cx));
        true
    }

    // ----------------------------------------------------------- views

    fn chip(&self, key: impl Into<SharedString>, label: String, remove: impl Fn(&mut Self, &mut Context<Self>) + 'static, cx: &mut Context<Self>) -> AnyElement {
        let key: SharedString = key.into();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .h(px(22.0))
            .pl_2()
            .pr_1()
            .rounded_full()
            .bg(theme::control_bg())
            .border_1()
            .border_color(theme::line())
            .text_xs()
            .child(label)
            .child(
                div()
                    .id(key)
                    .size(px(16.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .text_color(theme::muted())
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::control_hover()).text_color(theme::ink()))
                    .child("×")
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        remove(this, cx);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    /// What the next prompt will act on, as chips that can be removed.
    fn motion_prompt_chips(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some((about, key, _)) = self.motion_about(cx) else { return Vec::new() };
        let mut chips = Vec::new();
        for (i, (id, _)) in about.layers.iter().enumerate() {
            let layer = id.clone();
            chips.push(self.chip(
                SharedString::from(format!("prompt-chip-layer-{id}")),
                format!("Layer: {id}"),
                move |this, cx| {
                    if i == 0 {
                        this.toggle_motion_layer_also(layer.clone(), cx);
                    } else {
                        this.motion_agent.also.retain(|l| l != &layer);
                    }
                },
                cx,
            ));
        }
        if let Some((a, b)) = about.span {
            chips.push(self.chip("prompt-chip-span", format!("{a:.1} s to {b:.1} s"), |this, _| this.motion_agent.span = None, cx));
        }
        if self.motion_agent.continues(&key) {
            chips.push(self.chip("prompt-chip-conversation", "Continues the conversation".into(), |this, _| this.motion_agent.fresh = true, cx));
        }
        if chips.is_empty() {
            let what = match (&about.project, about.playhead) {
                (Some(_), Some((t, _))) => format!("The whole design, at {t:.1} s"),
                (None, Some((t, _))) => format!("A new motion piece at {t:.1} s"),
                _ => "The whole design".into(),
            };
            chips.push(div().h(px(22.0)).flex().items_center().text_xs().text_color(theme::muted()).child(what).into_any_element());
        }
        chips
    }

    /// How the preview is drawn: Raster, Raster + sky or Traced, one
    /// choice for the app; with a note when this GPU cannot do it.
    fn motion_preview_quality(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::motion_preview::{quality, set_quality, Quality, CANNOT_TRACE};
        let now = quality();
        let cannot = now.traces() && (self.motion_editor().is_some_and(|e| e.read(cx).cannot_trace()) || self.motion_pieces.as_ref().is_some_and(|p| p.cannot_trace));
        let mut row = div().flex().flex_none().items_center().gap_1().text_xs();
        if cannot {
            row = row.child(div().text_color(theme::danger()).child(CANNOT_TRACE));
        }
        row = row.child(div().text_color(theme::muted()).child("Preview"));
        for (i, q) in Quality::ALL.into_iter().enumerate() {
            row = row.child(
                div()
                    .id(("motion-preview-quality", i))
                    .h(px(22.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded_full()
                    .border_1()
                    .border_color(if q == now { theme::accent() } else { theme::line() })
                    .bg(if q == now { theme::control_selected() } else { theme::control_bg() })
                    .hover(|s| s.bg(theme::control_hover()))
                    .cursor_pointer()
                    .tooltip(crate::timeline_ui::tooltip(q.hint()))
                    .child(q.label())
                    .on_mouse_down(MouseButton::Left, cx.listener(move |_, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        set_quality(q);
                        cx.notify();
                    })),
            );
        }
        row.into_any_element()
    }

    /// The prompt bar, under the preview.
    pub(crate) fn motion_prompt_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let agent = &self.motion_agent;
        let busy = agent.busy();
        let missing = agent.missing();
        let small = |text: String| div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_xs().text_color(theme::muted()).child(text);
        let action = if busy {
            button("motion-prompt-stop", "Stop", ButtonKind::Danger, ButtonSize::Sm).icon("icons/stop.svg").tooltip("Stop the agent; what it changed so far stays, to keep or undo").on_click(cx.listener(|this, _, _, cx| this.stop_motion_agent(cx)))
        } else {
            button("motion-prompt-send", "Send", ButtonKind::Primary, ButtonSize::Sm)
                .disabled(missing.is_some() || agent.draft.trim().is_empty())
                .tooltip("Send (Enter)")
                .on_click(cx.listener(|this, _, _, cx| {
                    let words = this.motion_agent.draft.clone();
                    this.send_motion_prompt(&words, cx);
                }))
        };
        let mut status = div().h(px(28.0)).flex().items_center().gap_2();
        if let Some(why) = missing {
            status = status.child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_xs().text_color(theme::danger()).child(why));
        } else if let Some(request) = agent.requests.last().filter(|_| busy) {
            let edits = request.steps.iter().filter(|s| s.done && s.error.is_none() && s.text.starts_with("Editing")).count();
            status = status
                .child(div().size(px(8.0)).flex_none().rounded_full().bg(theme::accent()))
                .child(small(format!("{} · {:.0} s · {} steps, {edits} edits so far. You can keep editing.", request.now(), request.seconds(), request.steps.len())));
        } else if let Some(request) = agent.proposal() {
            let comparing = agent.comparing.is_some();
            let (keep, undo, again, held) = (request.id.clone(), request.id.clone(), request.id.clone(), request.id.clone());
            let head = match &request.outcome {
                Outcome::Stopped => "Stopped".to_string(),
                Outcome::Failed(_) => "Failed".to_string(),
                _ => format!("Done in {:.0} s", request.seconds()),
            };
            status = status
                .child(small(format!("{head} · {} edits · {}", request.edits, if comparing { "showing it before".to_string() } else { request.said.replace('\n', " ") })))
                .child(button("motion-prompt-keep", "Keep", ButtonKind::Primary, ButtonSize::Sm).on_click(cx.listener(move |this, _, _, cx| this.keep_motion_request(&keep, cx))))
                .child(button("motion-prompt-undo", "Undo", ButtonKind::Secondary, ButtonSize::Sm).tooltip("Take back everything this request changed").on_click(cx.listener(move |this, _, _, cx| {
                    this.undo_motion_request(&undo, cx);
                })))
                .child(button("motion-prompt-again", "Again", ButtonKind::Secondary, ButtonSize::Sm).tooltip("Undo it and ask again").on_click(cx.listener(move |this, _, _, cx| this.retry_motion_request(&again, cx))))
                .child(
                    div()
                        .id("motion-prompt-compare")
                        .flex_none()
                        .h(px(28.0))
                        .px_2()
                        .flex()
                        .items_center()
                        .rounded_md()
                        .border_1()
                        .border_color(theme::line())
                        .bg(if comparing { theme::control_selected() } else { theme::control_bg() })
                        .hover(|s| s.bg(theme::control_hover()))
                        .text_xs()
                        .cursor_pointer()
                        .child("Hold to compare")
                        .on_mouse_down(MouseButton::Left, cx.listener(move |this, _: &MouseDownEvent, _, cx| this.compare_motion_request(Some(&held), cx)))
                        .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &gpui::MouseUpEvent, _, cx| this.compare_motion_request(None, cx)))
                        .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _: &gpui::MouseUpEvent, _, cx| this.compare_motion_request(None, cx))),
                );
        } else if let Some(request) = agent.requests.last() {
            let text = match &request.outcome {
                Outcome::Failed(why) => format!("Failed: {}", why.replace('\n', " ")),
                Outcome::Undone => "Undone. Ctrl+Shift+Z puts it back.".into(),
                Outcome::Stopped => "Stopped before it changed anything.".into(),
                _ => request.said.replace('\n', " "),
            };
            status = status.child(small(text));
        } else {
            status = status.child(small("Select, type, Enter. Ctrl-click rows for more layers; drag the strip along the top of the ruler to mark a span.".into()));
        }
        div()
            .id("motion-prompt-bar")
            .flex_none()
            .h(px(PROMPT_BAR_HEIGHT))
            .px_3()
            .py_2()
            .flex()
            .flex_col()
            .gap_1()
            .border_t_1()
            .border_color(theme::line())
            .bg(theme::panel())
            .child(
                div()
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().min_w_0().flex().items_center().gap_1().overflow_hidden().children(self.motion_prompt_chips(cx)))
                    .child(self.motion_preview_quality(cx)),
            )
            .child(div().flex().items_center().gap_2().child(div().flex_1().min_w_0().child(self.text_fields.prompt.clone())).child(action))
            .child(status)
            .into_any_element()
    }

    /// The inspector's Properties / AI tabs.
    pub(crate) fn motion_inspector_tabs(&self, cx: &Context<Self>) -> AnyElement {
        let ai = self.motion_agent.ai_tab;
        let tab = |id: &'static str, label: &'static str, on: bool, selected: bool, cx: &Context<Self>| {
            button(id, label, if selected { ButtonKind::Secondary } else { ButtonKind::Ghost }, ButtonSize::Sm).selected(selected).on_click(cx.listener(move |this, _, _, cx| {
                this.motion_agent.ai_tab = on;
                cx.notify();
            }))
        };
        div().flex().gap_1().child(tab("motion-tab-properties", "Properties", false, !ai, cx)).child(tab("motion-tab-ai", "AI", true, ai, cx)).into_any_element()
    }

    /// The AI tab: what was asked (about `layer`, or about anything) and
    /// what came of it, newest first, each step as it happened.
    pub(crate) fn motion_agent_log(&self, layer: Option<&str>, cx: &Context<Self>) -> AnyElement {
        let note = |text: String| div().text_xs().text_color(theme::muted()).child(text);
        let mut list = div().flex().flex_col().gap_3();
        let shown: Vec<&AgentRequest> = self.motion_agent.requests.iter().rev().filter(|r| r.concerns(layer)).collect();
        if let Some(why) = self.motion_agent.missing() {
            list = list.child(div().text_xs().text_color(theme::danger()).child(why));
        }
        if shown.is_empty() {
            let what = layer.map(|l| format!("Nothing has been asked about {l} yet.")).unwrap_or_else(|| "Nothing has been asked yet.".into());
            return list.child(note(format!("{what} Type what you want in the bar under the preview and press Enter."))).into_any_element();
        }
        for request in shown {
            let state = match &request.outcome {
                Outcome::Running => format!("Working · {:.0} s", request.seconds()),
                Outcome::Proposal => format!("Proposed · {} edits", request.edits),
                Outcome::Kept if request.edits > 0 => format!("Kept · {} edits", request.edits),
                Outcome::Kept => "Answered".into(),
                Outcome::Undone => "Undone".into(),
                Outcome::Stopped => "Stopped".into(),
                Outcome::Failed(_) => "Failed".into(),
            };
            let mut card = div()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded_md()
                .bg(theme::control_bg())
                .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).child(format!("“{}”", request.words.lines().next().unwrap_or(""))))
                .child(note(state));
            for step in &request.steps {
                let mark = match (&step.error, step.done) {
                    (Some(_), _) => "✕",
                    (None, true) => "✓",
                    (None, false) => "…",
                };
                card = card.child(div().flex().gap_1().text_xs().child(div().w(px(12.0)).flex_none().text_color(if step.error.is_some() { theme::danger() } else { theme::muted() }).child(mark)).child(div().flex_1().min_w_0().child(step.text.clone())));
                if let Some(error) = &step.error {
                    card = card.child(div().pl(px(16.0)).text_xs().text_color(theme::danger()).child(error.clone()));
                }
            }
            if !request.said.is_empty() && request.outcome != Outcome::Running {
                card = card.child(div().text_xs().child(request.said.clone()));
            }
            if let Outcome::Failed(why) = &request.outcome {
                if why != &request.said {
                    card = card.child(div().text_xs().text_color(theme::danger()).child(why.clone()));
                }
            }
            let (undo, again, keep) = (request.id.clone(), request.id.clone(), request.id.clone());
            let id = |name: &str| SharedString::from(format!("motion-request-{name}-{}", request.id));
            let mut actions = div().flex().gap_1().pt_1();
            if request.outcome == Outcome::Running {
                actions = actions.child(button(id("stop"), "Stop", ButtonKind::Danger, ButtonSize::Sm).on_click(cx.listener(|this, _, _, cx| this.stop_motion_agent(cx))));
            } else if !self.motion_agent.busy() {
                if request.decidable() {
                    actions = actions.child(button(id("keep"), "Keep", ButtonKind::Secondary, ButtonSize::Sm).on_click(cx.listener(move |this, _, _, cx| this.keep_motion_request(&keep, cx))));
                }
                if request.edits > 0 {
                    actions = actions.child(button(id("undo"), "Undo", ButtonKind::Secondary, ButtonSize::Sm).on_click(cx.listener(move |this, _, _, cx| {
                        this.undo_motion_request(&undo, cx);
                    })));
                }
                actions = actions.child(button(id("again"), "Again", ButtonKind::Ghost, ButtonSize::Sm).on_click(cx.listener(move |this, _, _, cx| this.retry_motion_request(&again, cx))));
            }
            list = list.child(card.child(actions));
        }
        list.into_any_element()
    }

    /// Over the rows: the marked span, and while a request runs a ghost
    /// clip over the span being made (the whole width without one) with
    /// the step it is at. `x_of` maps timeline seconds to lane pixels.
    pub(crate) fn motion_lane_overlays(&self, x_of: &dyn Fn(f64) -> f32, cx: &gpui::App) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let offset = self.motion_doc_start().unwrap_or(0.0);
        if let Some((a, b)) = self.motion_agent.span {
            out.push(div().absolute().top_0().bottom_0().left(px(x_of(a))).w(px((x_of(b) - x_of(a)).max(2.0))).bg(theme::accent().opacity(0.10)).border_l_1().border_r_1().border_color(theme::accent().opacity(0.6)).into_any_element());
        }
        if let Some(request) = self.motion_agent.requests.last().filter(|_| self.motion_agent.busy()) {
            let length = self.motion_editor().map_or(self.video_duration, |e| e.read(cx).project().settings.duration);
            let (a, b) = request.about.span.unwrap_or((0.0, length));
            out.push(
                div()
                    .absolute()
                    .top(px(1.0))
                    .bottom(px(1.0))
                    .left(px(x_of(offset + a)))
                    .w(px((x_of(offset + b) - x_of(offset + a)).max(24.0)))
                    .rounded(px(4.0))
                    .border_1()
                    .border_dashed()
                    .border_color(theme::accent())
                    .bg(theme::accent().opacity(0.14))
                    .overflow_hidden()
                    .child(div().px_2().pt(px(2.0)).whitespace_nowrap().text_xs().text_color(theme::ink()).child(format!("✦ {} · {:.0} s", request.now(), request.seconds())))
                    .into_any_element(),
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Sender;

    /// A runner of our own in Claude Code's place: it plays a script and
    /// keeps what it was asked.
    struct Scripted {
        events: Vec<Event>,
        asked: Mutex<Vec<Request>>,
        stopped: Arc<Mutex<bool>>,
    }

    struct Held(#[allow(dead_code)] Sender<Event>, Arc<Mutex<bool>>);

    impl Running for Held {
        fn stop(&mut self) {
            *self.1.lock().unwrap() = true;
        }
    }

    impl Runner for Scripted {
        fn name(&self) -> &'static str {
            "Scripted"
        }
        fn available(&self) -> Availability {
            Availability::Ready
        }
        fn start(&self, request: Request, events: Sender<Event>) -> Result<Box<dyn Running>, Failure> {
            self.asked.lock().unwrap().push(request);
            for e in &self.events {
                events.send(e.clone()).unwrap();
            }
            Ok(Box::new(Held(events, self.stopped.clone())))
        }
    }

    fn step(id: &str, text: &str) -> Event {
        Event::Step { id: id.into(), tool: "edit_apply".into(), text: text.into() }
    }

    #[test]
    fn a_request_is_followed_to_its_end_and_the_next_continues_it() {
        let runner = Arc::new(Scripted {
            events: vec![
                Event::Started { conversation: "c1".into() },
                step("t1", "Looking at the frame"),
                Event::StepDone { id: "t1".into(), error: None },
                step("t2", "Editing: set title.size"),
            ],
            asked: Mutex::default(),
            stopped: Arc::default(),
        });
        let mut agent = MotionAgent::new(runner.clone());
        let about = About { project: Some("/work/p.lahza".into()), layers: vec![("title".into(), "text".into())], ..Default::default() };
        agent.send(about.clone(), "  make it bigger ", Some(vec![1]), "/work/p.lahza".into(), "/work".into());
        assert!(agent.busy() && agent.draft.is_empty());
        // A second prompt while one runs is not sent.
        agent.send(about.clone(), "and red", None, "/work/p.lahza".into(), "/work".into());
        assert_eq!(agent.requests.len(), 1);
        assert_eq!(agent.poll(), (true, None));
        let r = &agent.requests[0];
        assert_eq!((r.words.as_str(), r.outcome.clone(), r.steps.len()), ("make it bigger", Outcome::Running, 2));
        assert_eq!(r.now(), "Editing: set title.size", "the step it is at");
        assert!(r.concerns(Some("title")) && !r.concerns(Some("other")) && r.concerns(None));
        assert!(runner.asked.lock().unwrap()[0].text.ends_with("make it bigger"));
        assert!(runner.asked.lock().unwrap()[0].conversation.is_none());
        // Stop ends the runner; nothing changed, so nothing to decide.
        assert_eq!(agent.stop(), Some(0));
        assert!(*runner.stopped.lock().unwrap() && !agent.busy());
        assert_eq!(agent.requests[0].outcome, Outcome::Stopped);
        assert!(agent.proposal().is_none());
        // With edits in effect it is a proposal, even stopped.
        agent.requests[0].edits = 2;
        assert_eq!(agent.proposal().map(|r| r.id.clone()), Some(agent.requests[0].id.clone()));
        // The follow-up continues the conversation, unless a new one was
        // asked for.
        assert!(agent.continues(Path::new("/work/p.lahza")) && !agent.continues(Path::new("/work/q.lahza")));
        agent.send(about.clone(), "now red", None, "/work/p.lahza".into(), "/work".into());
        assert_eq!(runner.asked.lock().unwrap()[1].conversation.as_deref(), Some("c1"));
        agent.stop();
        agent.fresh = true;
        agent.send(about, "start over", None, "/work/p.lahza".into(), "/work".into());
        assert!(runner.asked.lock().unwrap()[2].conversation.is_none() && !agent.fresh);
        let ids: Vec<&str> = agent.requests.iter().map(|r| r.id.as_str()).collect();
        assert!(ids[0] != ids[1] && ids[1] != ids[2], "each request has its own id");
    }

    #[test]
    fn requests_are_kept_with_the_project() {
        let dir = std::env::temp_dir().join(format!("lahza-agent-kept-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = || Arc::new(Scripted { events: vec![Event::Started { conversation: "c7".into() }, step("t", "Editing: add dot (shape)"), Event::Done { text: "Added a dot.".into(), failed: false, cost: None }], asked: Mutex::default(), stopped: Arc::default() });
        let mut agent = MotionAgent::new(script());
        let about = About { project: Some(dir.clone()), layers: vec![("title".into(), "text".into())], span: Some((1.0, 2.0)), ..Default::default() };
        agent.send(about.clone(), "a dot", None, dir.clone(), dir.clone());
        agent.poll();
        agent.requests[0].edits = 1;
        agent.save(&dir);
        // The app again: the AI tab has it, and a follow-up continues.
        let runner = script();
        let mut again = MotionAgent::new(runner.clone());
        again.load(&dir);
        again.load(&dir);
        assert_eq!(again.requests.len(), 1, "read once");
        let r = &again.requests[0];
        assert_eq!((r.words.as_str(), r.said.as_str(), r.outcome.clone(), r.edits), ("a dot", "Added a dot.", Outcome::Proposal, 1));
        assert_eq!((r.about.clone(), r.steps.len()), (about.clone(), 1));
        assert!(again.proposal().is_some(), "still to keep or undo");
        again.send(about, "bigger", None, dir.clone(), dir.clone());
        assert_eq!(runner.asked.lock().unwrap()[0].conversation.as_deref(), Some("c7"));
        // Nothing kept, or something unreadable: nothing happens.
        let mut none = MotionAgent::new(script());
        none.load(Path::new("/nonexistent/project.lahza"));
        std::fs::write(dir.join(REQUESTS_FILE), "not json").unwrap();
        none.load(&dir);
        assert!(none.requests.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_end_and_failures_are_said() {
        let done = Arc::new(Scripted {
            events: vec![Event::Started { conversation: "c".into() }, step("t", "Editing: add dot (shape)"), Event::Done { text: "Added a dot.".into(), failed: false, cost: None }],
            asked: Mutex::default(),
            stopped: Arc::default(),
        });
        let mut agent = MotionAgent::new(done);
        agent.send(About::default(), "a dot", None, "/p".into(), "/".into());
        assert_eq!(agent.poll(), (true, Some(0)));
        let r = &agent.requests[0];
        assert_eq!((r.outcome.clone(), r.said.as_str()), (Outcome::Proposal, "Added a dot."));
        assert!(r.steps.iter().all(|s| s.done) && !agent.busy());
        assert_eq!(agent.poll(), (false, None));

        let signed_out = Arc::new(Scripted { events: vec![Event::Failed(Failure::NotSignedIn)], asked: Mutex::default(), stopped: Arc::default() });
        let mut agent = MotionAgent::new(signed_out);
        assert!(agent.missing().is_none());
        agent.send(About::default(), "a dot", None, "/p".into(), "/".into());
        assert_eq!(agent.poll().1, Some(0));
        assert!(matches!(&agent.requests[0].outcome, Outcome::Failed(why) if why.contains("not signed in")));
        assert!(agent.missing().is_some_and(|m| m.contains("Scripted is not signed in")), "and the bar says so from then on");

        // Not installed: it cannot start at all.
        let mut agent = MotionAgent::new(Arc::new(ClaudeCode { program: "/nonexistent/claude".into(), server: "lahza".into() }));
        agent.send(About::default(), "a dot", None, "/p".into(), "/".into());
        assert!(!agent.busy());
        assert!(matches!(&agent.requests[0].outcome, Outcome::Failed(why) if why.contains("not installed")));
        assert!(agent.missing().is_some_and(|m| m.contains("Claude Code is not installed")));
    }
}
