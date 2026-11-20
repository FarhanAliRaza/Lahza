//! The prompt bar in the real window, with a runner of our own in Claude
//! Code's place: it makes real edits through a `Toolbox` carrying the
//! request's id, as `lahza mcp` does for an agent.
use super::*;
use crate::motion_agent::{Availability, Event, Failure, Request, Runner, Running};
use crate::motion_prompt_ui::{MotionAgent, Outcome};
use crate::text_field::{EventKind, FieldEvent, Target};
use lahza_motion_mcp::tools::Toolbox;
use serde_json::json;
use std::sync::mpsc::{Receiver, Sender};
use std::path::Path;
use std::sync::Mutex;

struct Acting {
    project: PathBuf,
    /// The agent waits here before its last edit.
    gate: Mutex<Option<Receiver<()>>>,
    asked: Arc<Mutex<Vec<Request>>>,
}

struct Nothing;

impl Running for Nothing {
    fn stop(&mut self) {}
}

impl Runner for Acting {
    fn name(&self) -> &'static str {
        "Acting"
    }
    fn available(&self) -> Availability {
        Availability::Ready
    }
    fn start(&self, request: Request, events: Sender<Event>) -> Result<Box<dyn Running>, Failure> {
        self.asked.lock().unwrap().push(request.clone());
        let (gate, project) = (self.gate.lock().unwrap().take(), self.project.clone());
        std::thread::spawn(move || {
            let step = |id: &str, text: &str| events.send(Event::Step { id: id.into(), tool: String::new(), text: text.into() }).unwrap();
            let done = |id: &str| events.send(Event::StepDone { id: id.into(), error: None }).unwrap();
            let mut tb = Toolbox::new();
            tb.request = Some(request.id.clone());
            events.send(Event::Started { conversation: "c1".into() }).unwrap();
            step("t1", "Looking at the frame at 0.5");
            done("t1");
            step("t2", "Editing: set title.transform.position");
            tb.call("edit_apply", json!({"project": project, "ops": [{"op": "set", "target": "title.transform.position", "value": [320, 200]}]})).unwrap();
            done("t2");
            step("t3", "Editing: add dot (shape)");
            if let Some(gate) = gate {
                let _ = gate.recv_timeout(Duration::from_secs(30));
            }
            tb.call("edit_apply", json!({"project": project, "ops": [{"op": "add", "scene": "s1", "id": "dot", "kind": "shape", "shape": "circle", "size": [24, 24], "fill": "#ffffff", "transform": {"position": [320, 250]}}]})).unwrap();
            done("t3");
            events.send(Event::Done { text: "Moved the title down and put a dot under it.".into(), failed: false, cost: None }).unwrap();
        });
        Ok(Box::new(Nothing))
    }
}

fn sample(dir: &Path) -> PathBuf {
    let mut tb = Toolbox::new();
    tb.cwd = dir.to_path_buf();
    tb.backend = lahza_motion_render::Backend::Cpu;
    tb.call("project_create", json!({"path": "promo", "width": 640, "height": 360, "duration": 3})).unwrap();
    tb.call("edit_apply", json!({"ops": [
        {"op": "add_scene", "id": "s1"},
        {"op": "add", "id": "box", "kind": "shape", "shape": "rect", "size": [120, 80], "fill": "#ff3355",
         "transform": {"position": {"keys": [{"at": 0, "v": [200, 180]}, {"at": 1.5, "v": [440, 180], "ease": "glide"}]}}},
        {"op": "add", "id": "title", "kind": "text", "text": "Hello", "size": 48, "transform": {"position": [320, 80]},
         "effects": [{"type": "blur", "radius": 0}]}
    ]})).unwrap();
    dir.join("promo.lahza")
}

/// Select, type, Enter; the agent at work; a hand edit meanwhile; the
/// proposal compared, undone as one and put back. Screenshots go to
/// `$LAHZA_WORKFLOW_ARTIFACTS` (or the temp folder).
#[test]
#[ignore = "requires a display (can run under Xvfb)"]
fn prompt_bar_directs_an_agent_and_takes_its_request_back() {
    let dir = std::env::temp_dir().join(format!("lahza-motion-prompt-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let shots = std::env::var_os("LAHZA_WORKFLOW_ARTIFACTS").map(PathBuf::from).unwrap_or_else(|| dir.clone());
    std::fs::create_dir_all(&shots).unwrap();
    let project = sample(&dir);
    let (open, asked) = (project.clone(), Arc::new(Mutex::new(Vec::new())));
    let (release, gate) = std::sync::mpsc::channel();
    let runner = Arc::new(Acting { project: project.clone(), gate: Mutex::new(Some(gate)), asked: asked.clone() });
    Application::new().with_assets(Assets { base: asset_directory() }).run(move |cx| {
        let handle = open_studio_window(cx, true, |handle, cx| {
            cx.new(|cx| {
                let mut studio = Studio::new(handle, None, None, cx);
                studio.open_motion_project(open.clone());
                studio.motion_agent = MotionAgent::new(runner.clone());
                studio
            })
        })
        .unwrap();
        cx.activate(true);
        cx.spawn(async move |cx| {
            let wait = |ms| Timer::after(Duration::from_millis(ms));
            let any: AnyWindowHandle = handle.into();
            let draw = |cx: &mut gpui::AsyncApp| any.update(cx, |_, window, cx| window.draw(cx).clear()).unwrap();
            let shot = |name: &str| studio_workflow_tests::save_display(&shots.join(name), false);
            // Let the window be shown before anything is read back from it.
            wait(300).await;
            any.update(cx, |_, window, _| {
                window.activate_window();
                window.refresh();
            })
            .unwrap();
            wait(1200).await;
            let editor = |cx: &mut gpui::AsyncApp| handle.update(cx, |s, _, _| s.motion_doc.as_ref().map(|d| d.editor.clone())).unwrap();
            // The first frame of the preview.
            let mut framed = false;
            // On a software GPU the first frame builds its pipelines:
            // tens of seconds, cold.
            for _ in 0..3600 {
                wait(50).await;
                draw(cx);
                framed = editor(cx).is_some_and(|e| e.update(cx, |e, _| e.frame_png().is_some()).unwrap());
                if framed {
                    break;
                }
            }
            assert!(framed, "the preview rendered a frame");
            let editor = editor(cx).unwrap();
            // Select a layer, mark a span, type.
            let words = "move this one down and put a dot under it";
            handle
                .update(cx, |studio, window, cx| {
                    editor.update(cx, |e, _| {
                        e.seek(0.5);
                        e.select(Some("title".into()));
                    });
                    // A drag on the span strip, from 0.5 s to 2 s.
                    let bounds = studio.video_timeline_bounds.lock().unwrap().expect("the ruler was laid out");
                    let (spp, scroll) = (studio.video_timeline_seconds_per_pixel(), studio.video_timeline_scroll);
                    let x_at = |t: f64| bounds.origin.x + px((t / spp - scroll) as f32);
                    studio.begin_motion_span(x_at(0.5));
                    assert!(studio.update_motion_layer_drag(gpui::point(x_at(1.2), bounds.origin.y), cx));
                    assert!(studio.finish_motion_layer_drag(gpui::point(x_at(2.0), bounds.origin.y), false, cx));
                    let (a, b) = studio.motion_agent.span.expect("a span is marked");
                    assert!((a - 0.5).abs() < 0.02 && (b - 2.0).abs() < 0.02, "{a} to {b}");
                    // Ctrl-click adds a layer to what the prompt is about,
                    // and again takes it off.
                    studio.toggle_motion_layer_also("box".into(), cx);
                    assert!(studio.motion_layer_also("box"));
                    studio.toggle_motion_layer_also("box".into(), cx);
                    assert!(!studio.motion_layer_also("box"));
                    // A key typed with the layer selected begins a prompt.
                    window.focus(&studio.focus_handle);
                    cx.notify();
                })
                .unwrap();
            wait(100).await;
            draw(cx);
            any.update(cx, |_, window, cx| {
                let key = gpui::Keystroke { modifiers: Default::default(), key: "m".into(), key_char: Some("m".into()) };
                assert!(window.dispatch_keystroke(key, cx), "the key was taken");
            })
            .unwrap();
            wait(100).await;
            draw(cx);
            handle
                .update(cx, |studio, window, cx| {
                    assert_eq!(studio.motion_agent.draft, "m", "typing began the prompt");
                    assert!(studio.text_fields.prompt.read(cx).focus.is_focused(window), "and the bar has the keys from here");
                    studio.text_fields.prompt.update(cx, |_, cx| cx.emit(FieldEvent { target: Target::Prompt, kind: EventKind::Change, text: words.into() }));
                    cx.notify();
                })
                .unwrap();
            wait(300).await;
            draw(cx);
            wait(100).await;
            shot("prompt-1-selected-and-typed.png");
            assert_eq!(handle.update(cx, |s, _, _| s.motion_agent.draft.clone()).unwrap(), words);
            // Enter.
            handle.update(cx, |studio, _, cx| studio.text_fields.prompt.update(cx, |_, cx| cx.emit(FieldEvent { target: Target::Prompt, kind: EventKind::Submit, text: words.into() }))).unwrap();
            let mut steps = 0;
            for _ in 0..100 {
                wait(50).await;
                draw(cx);
                steps = handle.update(cx, |s, _, _| s.motion_agent.requests.last().map_or(0, |r| r.steps.len())).unwrap();
                if steps == 3 {
                    break;
                }
            }
            assert_eq!(steps, 3, "its steps arrive as it works");
            wait(700).await;
            draw(cx);
            wait(100).await;
            shot("prompt-2-agent-at-work.png");
            {
                let asked = asked.lock().unwrap();
                let text = &asked[0].text;
                assert!(text.contains("Selected layer (\"this one\"): title (text).") && text.contains("Marked span (\"here\"): 0.50 s to 2.00 s") && text.ends_with(words), "{text}");
                assert!(text.contains("Playhead: 0.50 s") && text.contains("in scene \"s1\""), "{text}");
                assert!(asked[0].picture.as_ref().is_some_and(|p| p.starts_with(b"\x89PNG")), "the frame goes with the words");
                assert!(asked[0].conversation.is_none());
            }
            // The editor stays usable: a hand edit while the agent works.
            handle
                .update(cx, |studio, _, cx| {
                    assert!(studio.motion_agent.busy());
                    assert_eq!(studio.motion_agent.requests[0].now(), "Editing: add dot (shape)");
                    editor.update(cx, |e, _| e.commit_ops(vec![json!({"op": "set", "target": "box.transform.opacity", "value": 0.5})]));
                })
                .unwrap();
            release.send(()).unwrap();
            let mut ended = false;
            for _ in 0..100 {
                wait(50).await;
                draw(cx);
                ended = handle.update(cx, |s, _, cx| !s.motion_agent.busy() && editor.read(cx).layer("dot").is_some()).unwrap();
                if ended {
                    break;
                }
            }
            assert!(ended, "the request ended and its edits show");
            wait(300).await;
            draw(cx);
            wait(100).await;
            shot("prompt-3-proposal.png");
            let id = handle
                .update(cx, |studio, _, cx| {
                    let request = studio.motion_agent.proposal().expect("what came back is a proposal");
                    assert_eq!((request.outcome.clone(), request.edits), (Outcome::Proposal, 2));
                    assert_eq!(request.said, "Moved the title down and put a dot under it.");
                    // Nothing lost, no number taken twice, the agent's
                    // entries (and only they) carry the request.
                    let history = lahza_motion_core::io::read_history(editor.read(cx).dir()).unwrap();
                    assert_eq!(history.iter().map(|e| e.n).collect::<Vec<_>>(), [1, 2, 3, 4]);
                    assert_eq!(history.iter().map(|e| e.request.is_some()).collect::<Vec<_>>(), [false, true, false, true]);
                    assert_eq!(editor.read(cx).field_state("box", &crate::motion_editor::FIELDS[5]).0, 0.5, "the hand edit is there too");
                    request.id.clone()
                })
                .unwrap();
            // Before and after, with nothing saved.
            handle
                .update(cx, |studio, _, cx| {
                    studio.compare_motion_request(Some(&id), cx);
                    assert!(editor.read(cx).compiled().element_window("dot").is_none(), "before: no dot");
                    assert!(!editor.read(cx).dragging(), "a comparison is not a drag: it draws in full");
                    studio.compare_motion_request(None, cx);
                    assert!(editor.read(cx).compiled().element_window("dot").is_some(), "after: the dot");
                    studio.motion_agent.ai_tab = true;
                    cx.notify();
                })
                .unwrap();
            wait(300).await;
            draw(cx);
            wait(100).await;
            shot("prompt-4-ai-tab.png");
            // Undo takes the whole request back and leaves the hand edit.
            handle
                .update(cx, |studio, _, cx| {
                    assert!(studio.undo_motion_request(&id, cx));
                    let e = editor.read(cx);
                    assert!(e.layer("dot").is_none());
                    assert_eq!(e.position_of("title").0, [320.0, 80.0]);
                    assert_eq!(e.field_state("box", &crate::motion_editor::FIELDS[5]).0, 0.5);
                    assert_eq!(studio.motion_agent.requests[0].outcome, Outcome::Undone);
                    assert!(studio.motion_agent.proposal().is_none());
                    // Redo puts all of it back; Ctrl+Z then takes the
                    // request back as one step.
                    studio.undo_motion_doc(true, cx);
                    assert!(editor.read(cx).layer("dot").is_some() && editor.read(cx).position_of("title").0 == [320.0, 200.0]);
                    studio.undo_motion_doc(false, cx);
                    let e = editor.read(cx);
                    assert!(e.layer("dot").is_none() && e.position_of("title").0 == [320.0, 80.0], "one undo, the whole request");
                    assert_eq!(e.field_state("box", &crate::motion_editor::FIELDS[5]).0, 0.5);
                    studio.motion_agent.ai_tab = false;
                    cx.notify();
                })
                .unwrap();
            // The follow-up continues the conversation.
            handle.update(cx, |studio, _, cx| studio.send_motion_prompt("and make it red", cx)).unwrap();
            for _ in 0..100 {
                wait(50).await;
                if !handle.update(cx, |s, _, _| s.motion_agent.busy()).unwrap() {
                    break;
                }
            }
            assert_eq!(asked.lock().unwrap()[1].conversation.as_deref(), Some("c1"));
            wait(300).await;
            draw(cx);
            wait(100).await;
            shot("prompt-5-properties.png");
            // By hand: a text's font and colour, an effect's colour and
            // two-part value, a colour typed.
            handle
                .update(cx, |studio, _, cx| {
                    editor.update(cx, |e, _| {
                        assert!(e.fonts().iter().any(|f| f == "Inter Medium"));
                        e.commit_ops(vec![
                            json!({"op": "set", "target": "title.font", "value": "Inter Medium"}),
                            json!({"op": "set", "target": "title.effects.1", "value": serde_json::from_str::<serde_json::Value>(crate::motion_editor::EFFECTS[1].1).unwrap()}),
                        ]);
                        e.set_color("title", "color", "#ffc928");
                        assert_eq!(e.color_state("title", "color"), "#ffc928");
                        e.set_color("title", "effects.1.color", "#3678ef");
                        assert_eq!(e.color_state("title", "effects.1.color"), "#3678ef");
                        e.set_color("title", "color", "yellow");
                        assert!(e.status().contains("not a colour") && e.color_state("title", "color") == "#ffc928");
                        // The shadow's offset is two values to drag.
                        let (_, effects) = crate::motion_editor::extra_values(e.layer("title").unwrap());
                        let y = effects[1].fields.iter().find(|f| f.label == "offset Y").expect("offset Y").clone();
                        assert_eq!(e.field_state("title", &y).0, 8.0);
                        e.drag_value("title", &y, 20.0);
                        e.drop_value("title", &y, 20.0);
                        assert_eq!(e.field_state("title", &y).0, 20.0);
                        assert_eq!(e.field_state("title", &effects[1].fields[0]).0, 0.0, "X stayed");
                    });
                    studio.show_motion_doc(cx);
                    // Typed in the inspector, with or without the #.
                    studio.motion_color_edit = Some(("title".into(), "color".into()));
                    studio.text_fields.layer_color.update(cx, |_, cx| cx.emit(FieldEvent { target: Target::LayerColor, kind: EventKind::Submit, text: "22B45D".into() }));
                    cx.notify();
                })
                .unwrap();
            wait(400).await;
            draw(cx);
            handle
                .update(cx, |_, _, cx| {
                    let e = editor.read(cx);
                    assert_eq!(e.color_state("title", "color"), "#22b45d");
                    let layer = serde_json::to_value(e.layer("title").unwrap()).unwrap();
                    assert_eq!((layer["font"].as_str(), layer["effects"][1]["offset"][1].as_f64()), (Some("Inter Medium"), Some(20.0)));
                })
                .unwrap();
            wait(600).await;
            draw(cx);
            wait(100).await;
            shot("prompt-7-by-hand.png");
            cx.update(|cx| cx.quit()).unwrap();
        })
        .detach();
    });
    std::fs::remove_dir_all(&dir).ok();
}

/// Answers at once and changes nothing.
struct Answering(Arc<Mutex<Vec<Request>>>);

impl Runner for Answering {
    fn name(&self) -> &'static str {
        "Answering"
    }
    fn available(&self) -> Availability {
        Availability::Ready
    }
    fn start(&self, request: Request, events: Sender<Event>) -> Result<Box<dyn Running>, Failure> {
        self.0.lock().unwrap().push(request);
        events.send(Event::Started { conversation: "c2".into() }).unwrap();
        events.send(Event::Done { text: "Nothing to change.".into(), failed: false, cost: None }).unwrap();
        Ok(Box::new(Nothing))
    }
}

/// The same bar on a recording's Motion tab: with no piece open the words
/// ask for a new piece on the span marked with Shift on the ruler.
#[test]
#[ignore = "requires a display and FFmpeg (can run under Xvfb)"]
fn prompt_bar_on_a_recording_asks_for_a_piece_on_the_span() {
    let dir = std::env::temp_dir().join(format!("lahza-motion-prompt-rec-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let shots = std::env::var_os("LAHZA_WORKFLOW_ARTIFACTS").map(PathBuf::from).unwrap_or_else(|| dir.clone());
    std::fs::create_dir_all(&shots).unwrap();
    let recording = studio_workflow_tests::fixture(&dir).directory.clone();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let runner = Arc::new(Answering(asked.clone()));
    let open = recording.clone();
    Application::new().with_assets(Assets { base: asset_directory() }).run(move |cx| {
        let handle = open_studio_window(cx, true, |handle, cx| cx.new(|cx| Studio::new(handle, None, None, cx))).unwrap();
        handle
            .update(cx, |studio, _, _| {
                studio.launcher_window = false;
                studio.load_generation = studio.load_generation.wrapping_add(1);
                studio.loading = None;
                studio.open_video_project(open.clone()).unwrap();
                studio.launcher_active = false;
                studio.inspector_visible = true;
                studio.select_inspector_tab(shell_ui::InspectorTab::Motion);
                studio.motion_agent = MotionAgent::new(runner.clone());
            })
            .unwrap();
        cx.activate(true);
        cx.spawn(async move |cx| {
            let wait = |ms| Timer::after(Duration::from_millis(ms));
            let any: AnyWindowHandle = handle.into();
            let draw = |cx: &mut gpui::AsyncApp| any.update(cx, |_, window, cx| window.draw(cx).clear()).unwrap();
            wait(300).await;
            any.update(cx, |_, window, _| {
                window.activate_window();
                window.refresh();
            })
            .unwrap();
            wait(1500).await;
            draw(cx);
            handle
                .update(cx, |studio, _, cx| {
                    assert!(studio.motion_prompt_visible(), "the bar shows in the Motion tab");
                    // Shift on the ruler marks a span instead of seeking.
                    let bounds = studio.video_timeline_bounds.lock().unwrap().expect("the ruler was laid out");
                    let (spp, scroll) = (studio.video_timeline_seconds_per_pixel(), studio.video_timeline_scroll);
                    let at = |t: f64| gpui::point(bounds.origin.x + px((t / spp - scroll) as f32), bounds.origin.y);
                    let before = studio.video_position;
                    let press = gpui::MouseDownEvent { button: MouseButton::Left, position: at(2.0), modifiers: gpui::Modifiers { shift: true, ..Default::default() }, click_count: 1, first_mouse: false };
                    studio.timeline_seek_down(&press, cx);
                    assert!(studio.update_motion_layer_drag(at(5.0), cx));
                    assert!(studio.finish_motion_layer_drag(at(7.0), true, cx));
                    assert_eq!(studio.video_position, before, "the playhead stayed");
                    let (a, b) = studio.motion_agent.span.expect("a span is marked");
                    assert!((a - 2.0).abs() < 0.05 && (b - 7.0).abs() < 0.05, "{a} to {b}");
                    studio.text_fields.prompt.update(cx, |_, cx| cx.emit(FieldEvent { target: Target::Prompt, kind: EventKind::Change, text: "five seconds of a map here".into() }));
                    cx.notify();
                })
                .unwrap();
            wait(400).await;
            draw(cx);
            wait(100).await;
            studio_workflow_tests::save_display(&shots.join("prompt-6-recording.png"), false);
            handle.update(cx, |studio, _, cx| studio.send_motion_prompt("five seconds of a map here", cx)).unwrap();
            for _ in 0..100 {
                wait(50).await;
                if !handle.update(cx, |s, _, _| s.motion_agent.busy()).unwrap() {
                    break;
                }
            }
            handle
                .update(cx, |studio, _, _| {
                    let asked = asked.lock().unwrap();
                    let text = &asked[0].text;
                    assert!(text.starts_with(&format!("Recording: {}. No motion piece is open", recording.display())), "{text}");
                    assert!(text.contains("recording_place at start 2.0") && text.contains("Marked span"), "{text}");
                    assert_eq!(asked[0].cwd, recording.parent().unwrap());
                    // It changed nothing: there is nothing to keep or undo.
                    let request = &studio.motion_agent.requests[0];
                    assert_eq!((request.outcome.clone(), request.edits), (Outcome::Kept, 0));
                    assert!(studio.motion_agent.proposal().is_none());
                })
                .unwrap();
            cx.update(|cx| cx.quit()).unwrap();
        })
        .detach();
    });
    std::fs::remove_dir_all(&dir).ok();
}
