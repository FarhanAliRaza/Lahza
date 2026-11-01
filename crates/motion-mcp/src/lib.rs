//! Agent interface of the motion engine: the MCP server (`lahza mcp`) and
//! the `lahza motion` command line. Both run the same [`tools::Toolbox`].

pub mod server;
pub mod tools;

use lahza_motion_render::Backend;
use serde_json::{json, Value};
use tools::Toolbox;

const USAGE: &str = "\
Usage:
  lahza mcp [--cpu|--gpu]                 MCP server on stdio (for agents)
  lahza motion create <dir> [--size 1920x1080] [--fps 30] [--duration 20] [--style NAME] [--music FILE]
  lahza motion info <project>
  lahza motion analyze <project> [--audio FILE] [--offset SECONDS]
  lahza motion apply <project> <ops.json>
  lahza motion check <project> [--json]
  lahza motion snapshot <project> [--at REF,REF,...] [--sheet] [--width PX] [-o out.png]
  lahza motion render <project> [-o out.mp4] [--draft] [--from S] [--to S] [--no-audio] [--cpu|--gpu]

Times and references: 2.5, \"48f\", beat:17, bar:5, word:canal, hit:2, \"#title.end + 0.25\".";

/// Handles `mcp` and `motion` sub-commands; `None` when `args` are not ours.
pub fn run_cli(args: &[String]) -> Option<i32> {
    let first = args.first()?.as_str();
    if first != "mcp" && first != "motion" {
        return None;
    }
    let mut toolbox = Toolbox::new();
    if args.iter().any(|a| a == "--cpu") {
        toolbox.backend = Backend::Cpu;
    } else if args.iter().any(|a| a == "--gpu") {
        toolbox.backend = Backend::Gpu;
    }
    if first == "mcp" {
        return Some(match server::serve(toolbox) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("lahza mcp: {e}");
                1
            }
        });
    }
    Some(match motion(&mut toolbox, &args[1..]) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("lahza motion: {e}");
            2
        }
    })
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn print(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

fn motion(tb: &mut Toolbox, args: &[String]) -> Result<(), String> {
    let command = args.first().map(String::as_str).unwrap_or("help");
    let target = args.get(1).cloned();
    let need = || target.clone().ok_or_else(|| format!("{command} needs a project folder\n\n{USAGE}"));
    match command {
        "create" => {
            let mut a = json!({"path": need()?});
            if let Some(size) = flag(args, "--size") {
                let (w, h) = size.split_once('x').ok_or("--size is WIDTHxHEIGHT")?;
                a["width"] = json!(w.parse::<u32>().map_err(|e| e.to_string())?);
                a["height"] = json!(h.parse::<u32>().map_err(|e| e.to_string())?);
            }
            for (f, k) in [("--fps", "fps"), ("--duration", "duration")] {
                if let Some(v) = flag(args, f) {
                    a[k] = json!(v.parse::<f64>().map_err(|e| e.to_string())?);
                }
            }
            for (f, k) in [("--style", "style"), ("--music", "music"), ("--background", "background")] {
                if let Some(v) = flag(args, f) {
                    a[k] = json!(v);
                }
            }
            print(&tb.call("project_create", a)?.json);
        }
        "info" => print(&tb.call("project_info", json!({"project": need()?}))?.json),
        "analyze" => {
            let mut a = json!({"project": need()?});
            if let Some(p) = flag(args, "--audio") {
                a["path"] = json!(p);
            }
            if let Some(o) = flag(args, "--offset") {
                a["offset"] = json!(o.parse::<f64>().map_err(|e| e.to_string())?);
            }
            let out = tb.call("audio_analyze", a)?;
            print(&out.json);
        }
        "apply" => {
            let file = args.get(2).ok_or("apply needs an ops JSON file")?;
            let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
            let v: Value = serde_json::from_str(&text).map_err(|e| format!("{file}: {e}"))?;
            let ops = v.get("ops").cloned().unwrap_or(v);
            print(&tb.call("edit_apply", json!({"project": need()?, "ops": ops, "author": "person"}))?.json);
        }
        "check" => {
            let out = tb.call("check", json!({"project": need()?, "verbose": has(args, "--json")}))?;
            if has(args, "--json") {
                print(&out.json);
            } else {
                let r = &out.json;
                println!(
                    "{}: {} errors, {} warnings, {} sync points, max drift {} f",
                    if r["ok"] == true { "OK" } else { "FAILED" },
                    r["errors"],
                    r["warnings"],
                    r["sync_count"],
                    r["max_drift_frames"]
                );
                for i in r["issues"].as_array().into_iter().flatten() {
                    println!(
                        "  {:<7} {:<9} {}{}",
                        i["severity"].as_str().unwrap_or(""),
                        i["kind"].as_str().unwrap_or(""),
                        i["element"].as_str().map(|e| format!("[{e}] ")).unwrap_or_default(),
                        i["message"].as_str().unwrap_or("")
                    );
                }
            }
            if out.json["ok"] != true {
                return Err("check found errors".into());
            }
        }
        "snapshot" => {
            let mut a = json!({"project": need()?});
            if let Some(at) = flag(args, "--at") {
                a["at"] = Value::Array(at.split(',').map(|s| s.trim().parse::<f64>().map(|v| json!(v)).unwrap_or_else(|_| json!(s.trim()))).collect());
            }
            if has(args, "--sheet") {
                a["sheet"] = json!(true);
            }
            if let Some(w) = flag(args, "--width") {
                a["width"] = json!(w.parse::<u32>().map_err(|e| e.to_string())?);
            }
            let out = tb.call("snapshot", a)?;
            let base = flag(args, "-o").unwrap_or_else(|| "snapshot.png".into());
            for (i, png) in out.images.iter().enumerate() {
                let path = if out.images.len() == 1 { base.clone() } else { base.replace(".png", &format!("-{i:02}.png")) };
                std::fs::write(&path, png).map_err(|e| format!("{path}: {e}"))?;
                println!("{path}");
            }
            eprintln!("{}", serde_json::to_string(&out.json).unwrap_or_default());
        }
        "render" => {
            let mut a = json!({"project": need()?, "wait": false});
            if let Some(o) = flag(args, "-o") {
                a["output"] = json!(o);
            }
            if has(args, "--draft") {
                a["quality"] = json!("draft");
            }
            if has(args, "--no-audio") {
                a["audio"] = json!(false);
            }
            for f in ["from", "to"] {
                if let Some(v) = flag(args, &format!("--{f}")) {
                    a[f] = json!(v.parse::<f64>().map_err(|e| e.to_string())?);
                }
            }
            let job = tb.call("render", a)?.json["job"].as_str().unwrap_or("render-1").to_string();
            loop {
                let s = tb.call("render_status", json!({"job": job}))?.json;
                match s["state"].as_str() {
                    Some("running") => {
                        eprint!("\r{:>5.1}%  {}/{} frames  {}   ", s["progress"].as_f64().unwrap_or(0.0) * 100.0, s["frames_done"], s["frames"], s["status"].as_str().unwrap_or(""));
                        std::thread::sleep(std::time::Duration::from_millis(250));
                    }
                    Some("done") => {
                        eprintln!();
                        print(&s["report"]);
                        break;
                    }
                    _ => {
                        eprintln!();
                        return Err(s["error"].as_str().unwrap_or("render failed").to_string());
                    }
                }
            }
        }
        "help" | "--help" | "-h" => println!("{USAGE}"),
        other => return Err(format!("unknown command '{other}'\n\n{USAGE}")),
    }
    Ok(())
}

#[cfg(test)]
mod tests;
