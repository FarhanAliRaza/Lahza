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
    // The app's settings for this test are a folder of its own (the key
    // typed below must not reach the real one).
    let config = dir.join("config");
    std::env::set_var("LAHZA_CONFIG_DIR", &config);
    let project_dir = project.clone();
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
            // The API key: typed masked in the AI tab, kept for the owner
            // only, and not shown again.
            handle
                .update(cx, |studio, window, cx| {
                    crate::motion_agent::set_choice(crate::motion_agent::Choice::Api);
                    studio.motion_agent.ai_tab = true;
                    studio.text_fields.api_key.update(cx, |field, cx| {
                        assert!(field.masked);
                        field.type_text("sk-ant-window-test-9876", window, cx);
                    });
                    cx.notify();
                })
                .unwrap();
            wait(400).await;
            draw(cx);
            wait(100).await;
            shot("prompt-8-api-key-typed.png");
            handle.update(cx, |studio, _, cx| studio.text_fields.api_key.update(cx, |_, cx| cx.emit(FieldEvent { target: Target::ApiKey, kind: EventKind::Submit, text: "sk-ant-window-test-9876".into() }))).unwrap();
            wait(400).await;
            draw(cx);
            wait(100).await;
            shot("prompt-9-api-key-kept.png");
            {
                use std::os::unix::fs::PermissionsExt;
                let file = crate::motion_agent_api::key_file();
                assert!(file.starts_with(&config), "the test's own settings folder: {}", file.display());
                assert_eq!(std::fs::read_to_string(&file).unwrap().trim(), "sk-ant-window-test-9876");
                assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
                assert_eq!(crate::motion_agent_api::key_hint(None, &file).as_deref(), Some("A key from the app's settings is set (ends …9876)."));
                // Not in the project, and not in what the app keeps of requests.
                assert!(!std::fs::read_to_string(project_dir.join("requests.json")).unwrap_or_default().contains("window-test"));
            }
            crate::motion_agent::set_choice(crate::motion_agent::Choice::Auto);
            cx.update(|cx| cx.quit()).unwrap();
        })
        .detach();
    });
    std::fs::remove_dir_all(&dir).ok();
}

/// One small request to the real Claude Code from the bar itself. It
/// costs money (about $0.15), so it runs only when asked for by
/// `LAHZA_REAL_AGENT=1` as well as by name (its name shares no word with
/// the other window tests' filters): `LAHZA_REAL_AGENT=1
/// LAHZA_MOTION_SOFTWARE_GPU=1 xvfb-run -a cargo test --release -p lahza
/// -- --ignored a_real_request_is_sent_from_the_bar`. `LAHZA_AGENT_LOG`
/// keeps Claude Code's stream.
#[test]
#[ignore = "sends a real request to Claude Code (costs money: needs LAHZA_REAL_AGENT=1); requires a display"]
fn a_real_request_is_sent_from_the_bar() {
    if std::env::var_os("LAHZA_REAL_AGENT").is_none_or(|v| v != "1") {
        println!("not run: a real request costs money; set LAHZA_REAL_AGENT=1 to send it");
        return;
    }
    let dir = std::env::temp_dir().join(format!("lahza-motion-prompt-real-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let shots = std::env::var_os("LAHZA_WORKFLOW_ARTIFACTS").map(PathBuf::from).unwrap_or_else(|| dir.clone());
    std::fs::create_dir_all(&shots).unwrap();
    let project = sample(&dir);
    std::env::set_var("LAHZA_CONFIG_DIR", dir.join("config"));
    // The server is the app itself, built beside this test (`LAHZA_BIN`
    // names another).
    let server = std::env::var_os("LAHZA_BIN").map(PathBuf::from).unwrap_or_else(|| std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join("lahza"));
    assert!(server.is_file(), "the app is built: {}", server.display());
    let runner = Arc::new(crate::motion_agent::ClaudeCode { program: "claude".into(), server });
    assert_eq!(runner.available(), Availability::Ready, "Claude Code is installed and signed in");
    let open = project.clone();
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
            wait(300).await;
            any.update(cx, |_, window, _| {
                window.activate_window();
                window.refresh();
            })
            .unwrap();
            wait(1200).await;
            let editor = |cx: &mut gpui::AsyncApp| handle.update(cx, |s, _, _| s.motion_doc.as_ref().map(|d| d.editor.clone())).unwrap();
            let mut framed = false;
            for _ in 0..3600 {
                wait(50).await;
                draw(cx);
                framed = editor(cx).is_some_and(|e| e.update(cx, |e, _| e.frame_png().is_some()).unwrap()) && handle.update(cx, |s, _, _| s.motion_agent.missing().is_none() && s.motion_agent.runner_name() == "Claude Code").unwrap();
                if framed {
                    break;
                }
            }
            assert!(framed, "the preview rendered a frame and the runner is ready");
            let editor = editor(cx).unwrap();
            let words = "make this one red";
            let before = handle
                .update(cx, |studio, _, cx| {
                    editor.update(cx, |e, _| {
                        e.seek(0.5);
                        e.select(Some("title".into()));
                    });
                    studio.text_fields.prompt.update(cx, |_, cx| cx.emit(FieldEvent { target: Target::Prompt, kind: EventKind::Change, text: words.into() }));
                    cx.notify();
                    editor.read(cx).color_state("title", "color")
                })
                .unwrap();
            wait(300).await;
            draw(cx);
            wait(100).await;
            shot("real-1-typed.png");
            // Enter: the one request.
            handle.update(cx, |studio, _, cx| studio.text_fields.prompt.update(cx, |_, cx| cx.emit(FieldEvent { target: Target::Prompt, kind: EventKind::Submit, text: words.into() }))).unwrap();
            let (mut seen, mut ended) = (0, false);
            // Ten minutes at most.
            for _ in 0..6000 {
                wait(100).await;
                draw(cx);
                let (steps, busy) = handle.update(cx, |s, _, _| (s.motion_agent.requests.last().map_or(0, |r| r.steps.len()), s.motion_agent.busy())).unwrap();
                // The bar as each of the first steps shows.
                if steps > seen && steps <= 4 {
                    wait(200).await;
                    draw(cx);
                    wait(100).await;
                    shot(&format!("real-2-at-work-step-{steps}.png"));
                }
                seen = steps;
                if !busy && handle.update(cx, |s, _, _| s.motion_agent.requests.len()).unwrap() == 1 {
                    ended = true;
                    break;
                }
            }
            assert!(ended, "the request ended");
            wait(1500).await;
            draw(cx);
            wait(100).await;
            shot("real-3-proposal.png");
            let id = handle
                .update(cx, |studio, _, cx| {
                    let request = &studio.motion_agent.requests[0];
                    // What it took, for whoever runs this.
                    println!("real request: {:.1} s, {} steps, {} edits{}, outcome {:?}", request.seconds(), request.steps.len(), request.edits, request.spent(), request.outcome);
                    for step in &request.steps {
                        println!("  step: {}{}", step.text, step.error.as_ref().map(|e| format!("  FAILED: {e}")).unwrap_or_default());
                    }
                    println!("  said: {}", request.said);
                    assert_eq!(request.outcome, Outcome::Proposal, "what came back is a proposal");
                    assert!(studio.motion_agent.proposal().is_some_and(|p| p.id == request.id));
                    assert!(!request.steps.is_empty() && request.steps.iter().all(|s| s.done), "its steps showed and ended");
                    assert!(request.steps.iter().any(|s| s.text.starts_with("Editing")), "an edit among them");
                    assert!(request.edits >= 1 && request.cost.is_some() && !request.said.is_empty());
                    // The history's entries for it carry its id, and the
                    // title is red.
                    let history = lahza_motion_core::io::read_history(editor.read(cx).dir()).unwrap();
                    let ours: Vec<_> = history.iter().filter(|e| e.request.as_deref() == Some(request.id.as_str())).collect();
                    assert_eq!(ours.len(), request.edits, "{history:?}");
                    assert!(history.iter().skip(1).all(|e| e.request.as_deref() == Some(request.id.as_str())), "nothing else was written");
                    let color = editor.read(cx).color_state("title", "color");
                    let part = |i: usize| u8::from_str_radix(&color[i..i + 2], 16).unwrap();
                    assert!(color != before && part(1) > 150 && part(3) < 110 && part(5) < 110, "the title is red: {color} (was {before})");
                    studio.motion_agent.ai_tab = true;
                    cx.notify();
                    request.id.clone()
                })
                .unwrap();
            wait(400).await;
            draw(cx);
            wait(100).await;
            shot("real-4-ai-tab.png");
            // One Undo takes the whole request back.
            handle
                .update(cx, |studio, _, cx| {
                    assert!(studio.undo_motion_request(&id, cx));
                    assert_eq!(editor.read(cx).color_state("title", "color"), before);
                    assert_eq!(studio.motion_agent.requests[0].outcome, Outcome::Undone);
                    assert!(studio.motion_agent.proposal().is_none());
                    cx.notify();
                })
                .unwrap();
            // The preview draws the title as it was.
            wait(3000).await;
            draw(cx);
            wait(100).await;
            shot("real-5-undone.png");
            cx.update(|cx| cx.quit()).unwrap();
        })
        .detach();
    });
    std::fs::remove_dir_all(&dir).ok();
}

/// By hand: a text's rim light, a `lut` with its table, a gradient's
/// stops added, moved and taken out, and a colour dragged in the picker.
#[test]
#[ignore = "requires a display (can run under Xvfb)"]
fn by_hand_a_rim_light_a_lut_gradient_stops_and_the_picker() {
    use crate::motion_timeline::LayerDrag;
    let dir = std::env::temp_dir().join(format!("lahza-motion-by-hand-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let shots = std::env::var_os("LAHZA_WORKFLOW_ARTIFACTS").map(PathBuf::from).unwrap_or_else(|| dir.clone());
    std::fs::create_dir_all(&shots).unwrap();
    let project = sample(&dir);
    std::env::set_var("LAHZA_CONFIG_DIR", dir.join("config"));
    // A table that swaps red and blue, outside the project.
    let table = dir.join("swap.cube");
    let mut cube = String::from("LUT_3D_SIZE 2\n");
    for b in 0..2 {
        for g in 0..2 {
            for r in 0..2 {
                cube += &format!("{b} {g} {r}\n");
            }
        }
    }
    std::fs::write(&table, cube).unwrap();
    let open = project.clone();
    Application::new().with_assets(Assets { base: asset_directory() }).run(move |cx| {
        let handle = open_studio_window(cx, true, |handle, cx| {
            cx.new(|cx| {
                let mut studio = Studio::new(handle, None, None, cx);
                studio.open_motion_project(open.clone());
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
            wait(300).await;
            any.update(cx, |_, window, _| {
                window.activate_window();
                window.refresh();
            })
            .unwrap();
            wait(1200).await;
            let editor = |cx: &mut gpui::AsyncApp| handle.update(cx, |s, _, _| s.motion_doc.as_ref().map(|d| d.editor.clone())).unwrap();
            let mut framed = false;
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
            let entries = |cx: &mut gpui::AsyncApp| editor.update(cx, |e, _| lahza_motion_core::io::read_history(e.dir()).unwrap().len()).unwrap();
            handle
                .update(cx, |studio, _, cx| {
                    editor.update(cx, |e, _| {
                        e.seek(0.5);
                        e.select(Some("title".into()));
                        // The rim light: on, then its angle dragged.
                        e.commit_ops(vec![json!({"op": "set", "target": "title.edge", "value": {"color": "#ffffff", "width": 3, "angle": 45}})]);
                        let (own, _) = crate::motion_editor::extra_values(e.layer("title").unwrap());
                        let angle = own.fields.iter().find(|f| f.label == "Rim angle").expect("the rim's angle").clone();
                        assert_eq!(e.field_state("title", &angle).0, 45.0);
                        e.drop_value("title", &angle, 120.0);
                        assert_eq!(e.field_state("title", &angle).0, 120.0);
                        e.set_color("title", "edge.color", "#ffc928");
                        assert_eq!(e.color_state("title", "edge.color"), "#ffc928");
                        // A gradient map on the box: a third stop lands in
                        // the middle, between its neighbours' colours.
                        let ops = e.add_effect_ops("box", serde_json::from_str(crate::motion_editor::EFFECTS[7].1).unwrap());
                        e.commit_ops(ops);
                        let ops = e.add_stop_ops("box", "effects.0.stops");
                        e.commit_ops(ops);
                        let stops = |e: &crate::motion_editor::MotionEditor| serde_json::to_value(e.layer("box").unwrap()).unwrap()["effects"][0]["stops"].clone();
                        assert_eq!(stops(e), json!([[0.0, "#101030"], [0.5, "#887155"], [1.0, "#ffd27a"]]));
                        // Moved: it stays between its neighbours.
                        let (_, effects) = crate::motion_editor::extra_values(e.layer("box").unwrap());
                        let place = effects[0].fields.iter().find(|f| f.label == "stop 2 at").expect("the stop's place").clone();
                        e.drag_value("box", &place, 0.25);
                        assert_eq!(e.field_state("box", &place).0, 0.25);
                        e.drop_value("box", &place, 0.25);
                        e.drop_value("box", &place, 7.0);
                        assert_eq!(stops(e)[1][0], json!(1.0), "not past the next stop");
                        e.drop_value("box", &place, 0.3);
                        // A fourth, then the first taken out; two always stay.
                        let ops = e.add_stop_ops("box", "effects.0.stops");
                        e.commit_ops(ops);
                        assert_eq!(stops(e).as_array().unwrap().len(), 4);
                        assert_eq!(stops(e)[2][0], json!(0.65), "in the widest gap");
                        let ops = e.remove_stop_ops("box", "effects.0.stops", 0);
                        e.commit_ops(ops);
                        assert_eq!(stops(e).as_array().unwrap().iter().map(|s| s[0].as_f64().unwrap()).collect::<Vec<_>>(), [0.3, 0.65, 1.0]);
                        let ops = e.remove_stop_ops("box", "effects.0.stops", 0);
                        e.commit_ops(ops);
                        assert!(e.remove_stop_ops("box", "effects.0.stops", 0).is_empty(), "two stops stay");
                    });
                    // A lut: the table is copied into the project and named.
                    studio.add_motion_lut("title", &table, cx);
                    let e = editor.read(cx);
                    assert!(e.dir().join("assets/swap.cube").is_file());
                    assert_eq!(e.luts(), ["assets/swap.cube"]);
                    let title = serde_json::to_value(e.layer("title").unwrap()).unwrap();
                    assert_eq!(title["effects"][1], json!({"type": "lut", "path": "assets/swap.cube", "amount": 1.0}), "{title}");
                    assert!(e.compiled().errors.is_empty(), "{:?}", e.compiled().errors);
                    studio.add_motion_lut("title", std::path::Path::new("/nowhere/x.png"), cx);
                    assert!(editor.read(cx).status().contains("not a .cube table"));
                    // The picker opens under the colour that is clicked.
                    studio.motion_color_edit = Some(("title".into(), "color".into()));
                    cx.notify();
                })
                .unwrap();
            wait(400).await;
            draw(cx);
            wait(100).await;
            draw(cx);
            let before = entries(cx);
            // A drag on the square to its top right corner (the full
            // tone of the hue), then along the strip to a third (green).
            handle
                .update(cx, |studio, _, cx| {
                    let [square, strip] = *studio.motion_picker_bounds.lock().unwrap();
                    let (square, strip) = (square.expect("the square was laid out"), strip.expect("the strip was laid out"));
                    let (hsv, alpha) = crate::motion_editor::hsv_of(&editor.read(cx).color_state("title", "color"));
                    studio.motion_layer_drag = Some(LayerDrag::Colour { layer: "title".into(), path: "color".into(), strip: false, hsv, alpha });
                    let middle = gpui::point(square.origin.x + square.size.width / 2.0, square.origin.y + square.size.height / 2.0);
                    assert!(studio.update_motion_layer_drag(middle, cx));
                    assert_eq!(editor.read(cx).color_state("title", "color"), "#804040", "shown as it is dragged");
                    assert!(editor.read(cx).dragging(), "unsaved: the proxy draws");
                    let corner = gpui::point(square.origin.x + square.size.width + px(40.0), square.origin.y - px(40.0));
                    assert!(studio.finish_motion_layer_drag(corner, false, cx));
                    assert_eq!(editor.read(cx).color_state("title", "color"), "#ff0000");
                    let (hsv, alpha) = crate::motion_editor::hsv_of("#ff0000");
                    studio.motion_layer_drag = Some(LayerDrag::Colour { layer: "title".into(), path: "color".into(), strip: true, hsv, alpha });
                    let third = gpui::point(strip.origin.x + strip.size.width / 3.0, strip.origin.y + px(2.0));
                    assert!(studio.finish_motion_layer_drag(third, false, cx));
                    assert_eq!(editor.read(cx).color_state("title", "color"), "#00ff00");
                    // A shadow keeps its alpha when its colour is picked.
                    assert_eq!(crate::motion_timeline::picked([10.0, 0.2, 0.3], true, strip, gpui::point(strip.origin.x - px(9.0), strip.origin.y)), [0.0, 0.2, 0.3]);
                    cx.notify();
                })
                .unwrap();
            assert_eq!(entries(cx), before + 2, "a drag is one edit");
            wait(2500).await;
            draw(cx);
            wait(100).await;
            shot("by-hand-1-picker.png");
            // The box's effects, further down the inspector.
            handle
                .update(cx, |studio, _, cx| {
                    studio.motion_color_edit = None;
                    editor.update(cx, |e, _| e.select(Some("box".into())));
                    cx.notify();
                })
                .unwrap();
            wait(2500).await;
            draw(cx);
            wait(100).await;
            shot("by-hand-2-box.png");
            cx.update(|cx| cx.quit()).unwrap();
        })
        .detach();
    });
    std::fs::remove_dir_all(&dir).ok();
}

/// Never answers; says when it is let go of.
struct Lingering(Arc<std::sync::atomic::AtomicBool>);

struct Held(Arc<std::sync::atomic::AtomicBool>);

impl Running for Held {
    fn stop(&mut self) {}
}

impl Drop for Held {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Runner for Lingering {
    fn name(&self) -> &'static str {
        "Lingering"
    }
    fn available(&self) -> Availability {
        Availability::Ready
    }
    fn start(&self, _: Request, events: Sender<Event>) -> Result<Box<dyn Running>, Failure> {
        events.send(Event::Started { conversation: "c4".into() }).unwrap();
        // (The sender is kept so the request stays open.)
        std::mem::forget(events);
        Ok(Box::new(Held(self.0.clone())))
    }
}

/// With the pointer's own events: the picker is pressed, dragged and let
/// go. Then a long bake shows its share and is stopped, and closing the
/// window lets go of a request that is still running.
#[test]
#[ignore = "requires an X display and xdotool (can run under Xvfb)"]
fn a_pointer_on_the_picker_a_bake_stopped_and_the_window_closed() {
    let dir = std::env::temp_dir().join(format!("lahza-motion-pointer-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let shots = std::env::var_os("LAHZA_WORKFLOW_ARTIFACTS").map(PathBuf::from).unwrap_or_else(|| dir.clone());
    std::fs::create_dir_all(&shots).unwrap();
    std::env::set_var("LAHZA_CONFIG_DIR", dir.join("config"));
    // A title, and enough boxes falling for long enough that the bake
    // takes seconds.
    let mut tb = Toolbox::new();
    tb.cwd = dir.clone();
    tb.call("project_create", json!({"path": "heap", "duration": 20, "width": 640, "height": 360})).unwrap();
    let mut ops = vec![json!({"op": "add_scene", "id": "s1", "simulation": {"ground": {"y": 340}}}), json!({"op": "add", "id": "title", "kind": "text", "text": "Heap", "size": 40, "transform": {"position": [320, 40]}})];
    for i in 0..150 {
        ops.push(json!({"op": "add", "id": format!("b{i}"), "kind": "mesh", "mesh": {"type": "box", "size": [16, 16], "depth": 16}, "transform": {"position": [200 + (i % 15) * 17, 300 - (i / 15) * 30], "rotation": i * 7}, "body": {}}));
    }
    tb.call("edit_apply", json!({"ops": ops})).unwrap();
    let project = dir.join("heap.lahza");
    let open = project.clone();
    let let_go: Arc<std::sync::atomic::AtomicBool> = Arc::default();
    let runner = Arc::new(Lingering(let_go.clone()));
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
            wait(300).await;
            any.update(cx, |_, window, _| {
                window.activate_window();
                window.refresh();
            })
            .unwrap();
            wait(1200).await;
            let editor = |cx: &mut gpui::AsyncApp| handle.update(cx, |s, _, _| s.motion_doc.as_ref().map(|d| d.editor.clone())).unwrap();
            let mut framed = false;
            for _ in 0..3600 {
                wait(50).await;
                draw(cx);
                framed = editor(cx).is_some_and(|e| e.update(cx, |e, _| e.frame_png().is_some() && !e.sim_needs().is_empty()).unwrap());
                if framed {
                    break;
                }
            }
            assert!(framed, "the preview rendered a frame and the missing bake was found");
            let editor = editor(cx).unwrap();
            // The picker, opened under the title's colour.
            handle
                .update(cx, |studio, _, cx| {
                    editor.update(cx, |e, _| e.select(Some("title".into())));
                    studio.motion_color_edit = Some(("title".into(), "color".into()));
                    cx.notify();
                })
                .unwrap();
            wait(400).await;
            draw(cx);
            wait(100).await;
            draw(cx);
            let square = handle.update(cx, |s, _, _| s.motion_picker_bounds.lock().unwrap()[0]).unwrap().expect("the square was laid out");
            let entries = |cx: &mut gpui::AsyncApp| editor.update(cx, |e, _| lahza_motion_core::io::read_history(e.dir()).unwrap().len()).unwrap();
            let before = entries(cx);
            let colour = |cx: &mut gpui::AsyncApp| editor.update(cx, |e, _| e.color_state("title", "color")).unwrap();
            // The X server's own pointer (xdotool), at the window's place
            // on the screen.
            let origin = any.update(cx, |_, window, _| window.bounds().origin).unwrap();
            let pointer = |args: &[&str], at: Option<gpui::Point<gpui::Pixels>>| {
                let mut command = std::process::Command::new("xdotool");
                if let Some(at) = at {
                    command.args(["mousemove", &format!("{:.0}", f32::from(origin.x + at.x)), &format!("{:.0}", f32::from(origin.y + at.y))]);
                }
                assert!(command.args(args).status().expect("xdotool is installed").success());
            };
            let middle = gpui::point(square.origin.x + square.size.width / 2.0, square.origin.y + square.size.height / 2.0);
            let corner = gpui::point(square.origin.x + square.size.width - px(2.0), square.origin.y + px(2.0));
            pointer(&["mousedown", "1"], Some(middle));
            wait(300).await;
            draw(cx);
            let pressed = colour(cx);
            let part = |hex: &str, i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap();
            assert!((part(&pressed, 1) as i32 - 0x80).abs() < 6 && (part(&pressed, 3) as i32 - 0x40).abs() < 6, "the press shows the colour under it: {pressed}");
            assert_eq!(entries(cx), before, "unsaved while held");
            pointer(&[], Some(gpui::point(middle.x + px(30.0), middle.y - px(20.0))));
            wait(100).await;
            pointer(&[], Some(corner));
            wait(300).await;
            draw(cx);
            let dragged = colour(cx);
            assert!(part(&dragged, 1) > 0xf0 && part(&dragged, 3) < 0x10, "the drag follows the pointer: {dragged}");
            pointer(&["mouseup", "1"], None);
            wait(300).await;
            draw(cx);
            assert_eq!((colour(cx), entries(cx)), (dragged, before + 1), "let go: saved as one edit");
            // The strip over the ruler, dragged from 4 s to 10 s.
            let (ruler, spp, scroll) = handle
                .update(cx, |s, _, _| {
                    // (Its own statement: the next call locks the bounds too.)
                    let ruler = s.video_timeline_bounds.lock().unwrap().expect("the ruler was laid out");
                    (ruler, s.video_timeline_seconds_per_pixel(), s.video_timeline_scroll)
                })
                .unwrap();
            let on_strip = |t: f64| gpui::point(ruler.origin.x + px((t / spp - scroll) as f32), ruler.origin.y - px(7.0));
            pointer(&["mousedown", "1"], Some(on_strip(4.0)));
            wait(200).await;
            pointer(&[], Some(on_strip(7.0)));
            wait(100).await;
            pointer(&[], Some(on_strip(10.0)));
            wait(200).await;
            pointer(&["mouseup", "1"], None);
            wait(300).await;
            draw(cx);
            let (a, b) = handle.update(cx, |s, _, _| s.motion_agent.span).unwrap().expect("the pointer marked a span");
            assert!((a - 4.0).abs() < 0.2 && (b - 10.0).abs() < 0.2, "{a} to {b}");
            wait(200).await;
            shot("pointer-0-picker-and-span.png");
            // A bake that takes a while: its share shows, and Stop ends it.
            handle
                .update(cx, |studio, _, cx| {
                    studio.motion_color_edit = None;
                    studio.bake_motion_simulation(cx);
                })
                .unwrap();
            let began = std::time::Instant::now();
            let mut share = 0.0;
            for _ in 0..12000 {
                wait(5).await;
                share = editor.update(cx, |e, _| if e.baking() { e.bake_share().1 } else { -1.0 }).unwrap();
                if share > 0.03 || share < 0.0 {
                    break;
                }
            }
            assert!(share > 0.03, "the bake said how far it was (it ended first: make the scene heavier)");
            draw(cx);
            wait(50).await;
            shot("pointer-1-baking.png");
            handle.update(cx, |studio, _, cx| studio.stop_motion_bake(cx)).unwrap();
            let mut ended = false;
            for _ in 0..2000 {
                wait(10).await;
                draw(cx);
                ended = editor.update(cx, |e, _| !e.baking()).unwrap();
                if ended {
                    break;
                }
            }
            println!("bake: {:.0}% after {:.2} s, stopped", share * 100.0, began.elapsed().as_secs_f64());
            assert!(ended, "the bake stopped");
            let said = editor.update(cx, |e, _| e.status().to_string()).unwrap();
            assert!(said.contains("stopped"), "{said}");
            assert!(!lahza_motion_core::sim::bake_path(&project, "s1").is_file(), "nothing was written");
            assert!(began.elapsed() < Duration::from_secs(60));
            draw(cx);
            wait(100).await;
            shot("pointer-2-stopped.png");
            // A request still running when the window closes is let go of
            // (a real runner's child ends when it is).
            handle.update(cx, |studio, _, cx| studio.send_motion_prompt("anything", cx)).unwrap();
            wait(200).await;
            assert!(handle.update(cx, |s, _, _| s.motion_agent.busy()).unwrap());
            assert!(!let_go.load(std::sync::atomic::Ordering::SeqCst));
            any.update(cx, |_, window, _| window.remove_window()).unwrap();
            let mut gone = false;
            for _ in 0..100 {
                wait(50).await;
                gone = let_go.load(std::sync::atomic::Ordering::SeqCst);
                if gone {
                    break;
                }
            }
            assert!(gone, "closing the window ends the request");
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

/// Makes a piece on the recording and places it, as an agent asked for
/// one would: real tool calls carrying the request's id.
struct Making {
    recording: PathBuf,
    gate: Mutex<Option<Receiver<()>>>,
}

impl Runner for Making {
    fn name(&self) -> &'static str {
        "Making"
    }
    fn available(&self) -> Availability {
        Availability::Ready
    }
    fn start(&self, request: Request, events: Sender<Event>) -> Result<Box<dyn Running>, Failure> {
        let (gate, recording) = (self.gate.lock().unwrap().take(), self.recording.clone());
        std::thread::spawn(move || {
            let mut tb = Toolbox::new();
            tb.request = Some(request.id.clone());
            tb.cwd = request.cwd.clone();
            tb.backend = lahza_motion_render::Backend::Cpu;
            events.send(Event::Started { conversation: "c3".into() }).unwrap();
            events.send(Event::Step { id: "t1".into(), tool: "project_create".into(), text: "Making a motion piece".into() }).unwrap();
            if let Some(gate) = gate {
                let _ = gate.recv_timeout(Duration::from_secs(60));
            }
            tb.call("project_create", json!({"path": "card", "recording": recording, "duration": 3})).unwrap();
            tb.call("edit_apply", json!({"ops": [{"op": "add_scene", "id": "s1", "background": "#d03734"}, {"op": "add", "id": "word", "kind": "text", "text": "Card", "size": 60}]})).unwrap();
            tb.call("recording_place", json!({"start": 2.0, "placement": "full"})).unwrap();
            events.send(Event::StepDone { id: "t1".into(), error: None }).unwrap();
            events.send(Event::Done { text: "Made a card and placed it at 2 s.".into(), failed: false, cost: None }).unwrap();
        });
        Ok(Box::new(Nothing))
    }
}

/// On a recording with no piece open: the agent's ghost clip shows on the
/// Motion lane while it works, the piece it placed comes back on the
/// timeline, Hold to compare shows the video without it, and Undo takes
/// it off.
#[test]
#[ignore = "requires a display and FFmpeg (can run under Xvfb)"]
fn a_piece_made_on_a_recording_is_seen_compared_and_undone() {
    let dir = std::env::temp_dir().join(format!("lahza-motion-prompt-made-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let shots = std::env::var_os("LAHZA_WORKFLOW_ARTIFACTS").map(PathBuf::from).unwrap_or_else(|| dir.clone());
    std::fs::create_dir_all(&shots).unwrap();
    let recording = studio_workflow_tests::fixture(&dir).directory.clone();
    let (release, gate) = std::sync::mpsc::channel();
    let runner = Arc::new(Making { recording: recording.clone(), gate: Mutex::new(Some(gate)) });
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
            let shot = |name: &str| studio_workflow_tests::save_display(&shots.join(name), false);
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
                    assert!(studio.motion_piece_lane_height().is_none(), "no piece, no lane");
                    studio.video_position = 2.5;
                    studio.motion_agent.span = Some((2.0, 5.0));
                    studio.send_motion_prompt("a red card here", cx);
                })
                .unwrap();
            let mut at_work = false;
            for _ in 0..100 {
                wait(50).await;
                draw(cx);
                at_work = handle.update(cx, |s, _, _| s.motion_agent.requests.last().is_some_and(|r| r.steps.len() == 1)).unwrap();
                if at_work {
                    break;
                }
            }
            assert!(at_work, "its step arrived");
            handle
                .update(cx, |studio, _, _| {
                    let (request, a, b) = studio.motion_ghost_on_recording().expect("a ghost clip while it works");
                    assert_eq!((request.now().as_str(), a, b), ("Making a motion piece", 2.0, 5.0));
                    assert!(studio.motion_piece_lane_height().is_some(), "a lane for the ghost");
                })
                .unwrap();
            wait(400).await;
            draw(cx);
            wait(100).await;
            shot("made-1-ghost.png");
            release.send(()).unwrap();
            // The request ends and the piece it placed is on the timeline.
            let mut back = false;
            for _ in 0..400 {
                wait(50).await;
                draw(cx);
                back = handle.update(cx, |s, _, _| !s.motion_agent.busy() && s.motion_pieces.as_ref().is_some_and(|m| m.items.len() == 1)).unwrap();
                if back {
                    break;
                }
            }
            assert!(back, "the piece came back on the timeline");
            let id = handle
                .update(cx, |studio, _, _| {
                    assert!(studio.motion_ghost_on_recording().is_none());
                    let request = studio.motion_agent.proposal().expect("a proposal");
                    assert!(request.made_piece && request.edits == 1, "{} edits", request.edits);
                    let pieces = studio.motion_pieces.as_ref().unwrap();
                    assert_eq!((pieces.items[0].piece.start, pieces.items[0].piece.placement), (2.0, lahza_motion_core::recording::Placement::Full));
                    assert!(pieces.any_active(2.5, lahza_motion_core::recording::Placement::Full));
                    request.id.clone()
                })
                .unwrap();
            // Its frame over the video (a software GPU's first takes a
            // while).
            let mut drawn = false;
            for _ in 0..3600 {
                wait(50).await;
                draw(cx);
                drawn = handle.update(cx, |s, _, _| s.motion_pieces.as_ref().is_some_and(|m| !m.frames.is_empty())).unwrap();
                if drawn {
                    break;
                }
            }
            assert!(drawn, "the card is drawn over the video");
            wait(500).await;
            draw(cx);
            wait(100).await;
            shot("made-2-placed.png");
            // Held: the video as it was, with nothing written.
            handle
                .update(cx, |studio, _, cx| {
                    studio.compare_motion_request(Some(&id), cx);
                    let pieces = studio.motion_pieces.as_ref().unwrap();
                    assert!(!pieces.any_active(2.5, lahza_motion_core::recording::Placement::Full), "before: no card ({:?} against {:?}, a piece open: {})", studio.motion_agent.requests[0].about.project, pieces.items[0].dir, studio.motion_doc.is_some());
                    assert_eq!(lahza_motion_core::recording::read_pieces(&recording).len(), 1, "nothing was written");
                })
                .unwrap();
            wait(1500).await;
            draw(cx);
            wait(100).await;
            draw(cx);
            wait(100).await;
            shot("made-3-compared.png");
            handle
                .update(cx, |studio, _, cx| {
                    studio.compare_motion_request(None, cx);
                    assert!(studio.motion_pieces.as_ref().unwrap().any_active(2.5, lahza_motion_core::recording::Placement::Full), "after: the card");
                    // Undo takes the piece off the video.
                    assert!(studio.undo_motion_request(&id, cx));
                    assert!(lahza_motion_core::recording::read_pieces(&recording).is_empty());
                })
                .unwrap();
            cx.update(|cx| cx.quit()).unwrap();
        })
        .detach();
    });
    std::fs::remove_dir_all(&dir).ok();
}

/// A scene with bodies: the bar says the simulation is not baked, Bake
/// bakes it without holding the window, and moving a body says it needs
/// baking again.
#[test]
#[ignore = "requires a display (can run under Xvfb)"]
fn a_simulation_is_baked_from_the_bar_and_goes_stale_when_a_body_moves() {
    let dir = std::env::temp_dir().join(format!("lahza-motion-sim-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let shots = std::env::var_os("LAHZA_WORKFLOW_ARTIFACTS").map(PathBuf::from).unwrap_or_else(|| dir.clone());
    std::fs::create_dir_all(&shots).unwrap();
    let mut tb = Toolbox::new();
    tb.cwd = dir.clone();
    tb.call("project_create", json!({"path": "drop", "duration": 2, "width": 640, "height": 360})).unwrap();
    tb.call("edit_apply", json!({"ops": [
        {"op": "add_scene", "id": "s1", "simulation": {"ground": {"y": 330}}},
        {"op": "add", "id": "crate", "kind": "mesh", "mesh": {"type": "box", "size": [60, 60], "depth": 60}, "material": {"preset": "plastic", "color": "#d03734"},
         "transform": {"position": [320, 60], "rotation": 12}, "body": {"bounce": 0.3}}
    ]}))
    .unwrap();
    let project = dir.join("drop.lahza");
    let open = project.clone();
    Application::new().with_assets(Assets { base: asset_directory() }).run(move |cx| {
        let handle = open_studio_window(cx, true, |handle, cx| {
            cx.new(|cx| {
                let mut studio = Studio::new(handle, None, None, cx);
                studio.open_motion_project(open.clone());
                studio
            })
        })
        .unwrap();
        cx.activate(true);
        cx.spawn(async move |cx| {
            use crate::motion_editor::SimNeed;
            let wait = |ms| Timer::after(Duration::from_millis(ms));
            let any: AnyWindowHandle = handle.into();
            let draw = |cx: &mut gpui::AsyncApp| any.update(cx, |_, window, cx| window.draw(cx).clear()).unwrap();
            wait(300).await;
            any.update(cx, |_, window, _| {
                window.activate_window();
                window.refresh();
            })
            .unwrap();
            let editor = |cx: &mut gpui::AsyncApp| handle.update(cx, |s, _, _| s.motion_doc.as_ref().map(|d| d.editor.clone())).unwrap();
            // Waits (up to three minutes: a software GPU's first frame) for
            // the editor to say `want` of its simulation.
            let until = async |cx: &mut gpui::AsyncApp, what: &str, want: &dyn Fn(&crate::motion_editor::MotionEditor) -> bool| {
                for _ in 0..3600 {
                    wait(50).await;
                    draw(cx);
                    if editor(cx).is_some_and(|e| e.update(cx, |e, _| want(e)).unwrap()) {
                        return;
                    }
                }
                panic!("never: {what}");
            };
            until(cx, "the missing bake is found", &|e| e.sim_needs() == [SimNeed::Unbaked { scene: "s1".into(), bodies: 1 }] && e.frame_png().is_some()).await;
            wait(600).await;
            draw(cx);
            wait(100).await;
            studio_workflow_tests::save_display(&shots.join("sim-1-not-baked.png"), false);
            // One click.
            handle
                .update(cx, |studio, _, cx| {
                    studio.bake_motion_simulation(cx);
                    assert!(studio.motion_doc.as_ref().unwrap().editor.read(cx).baking(), "the bake runs beside the window");
                })
                .unwrap();
            until(cx, "the bake ends and is loaded", &|e| !e.baking() && e.sim_needs().is_empty() && e.project().bakes.contains_key("s1")).await;
            assert!(lahza_motion_core::sim::bake_path(&project, "s1").is_file());
            let said = editor(cx).unwrap().update(cx, |e, _| e.status().to_string()).unwrap();
            assert!(said.starts_with("Baked s1 (1 bodies"), "{said}");
            // The body dragged: the simulation needs baking again.
            editor(cx).unwrap().update(cx, |e, _| e.move_layer("crate", [200.0, 40.0], true)).unwrap();
            until(cx, "the stale bake is found", &|e| e.sim_needs() == [SimNeed::Stale { scene: "s1".into(), bodies: 1 }]).await;
            wait(600).await;
            draw(cx);
            wait(100).await;
            studio_workflow_tests::save_display(&shots.join("sim-2-stale.png"), false);
            // A bake made elsewhere (an agent's `sim_bake`) shows too.
            crate::motion_editor::bake_simulations(&project).unwrap();
            until(cx, "a bake made elsewhere is picked up", &|e| e.sim_needs().is_empty()).await;
            cx.update(|cx| cx.quit()).unwrap();
        })
        .detach();
    });
    std::fs::remove_dir_all(&dir).ok();
}
