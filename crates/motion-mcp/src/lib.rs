//! Agent interface of the motion engine: the MCP server (`lahza mcp`) and
//! the `lahza motion` command line. Both run the same [`tools::Toolbox`].

pub mod measure;
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
  lahza motion compare <project> --against REF.mp4 [--at REF,...] [--from S] [--to S] [--offset S] [--width PX] [-o compare.png]
                                          (no --at: every frame, first to last, plus a side-by-side video)
  lahza motion track <video> (--color HEX [--tolerance N] [--region x,y,w,h] | --point x,y | --box x,y,w,h)
                    [--from S] [--to S] [--keys [--scale S] [--at S] [--layer ID]]
  lahza motion fit <samples.json> [--fps N]   samples [[t, value], …], or a track output (fits its x/y)
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
        "compare" => {
            let against = flag(args, "--against").ok_or("compare needs --against REF.mp4")?;
            let mut a = json!({"project": need()?, "against": against});
            if let Some(at) = flag(args, "--at") {
                a["at"] = Value::Array(at.split(',').map(|s| s.trim().parse::<f64>().map(|v| json!(v)).unwrap_or_else(|_| json!(s.trim()))).collect());
            }
            for f in ["offset", "from", "to"] {
                if let Some(v) = flag(args, &format!("--{f}")) {
                    a[f] = json!(v.parse::<f64>().map_err(|e| e.to_string())?);
                }
            }
            if let Some(w) = flag(args, "--width") {
                a["width"] = json!(w.parse::<u32>().map_err(|e| e.to_string())?);
            }
            let out = tb.call("compare", a)?;
            let path = flag(args, "-o").unwrap_or_else(|| "compare.png".into());
            for (i, png) in out.images.iter().enumerate() {
                let p = if i == 0 { path.clone() } else { path.replace(".png", "-worst.png") };
                std::fs::write(&p, png).map_err(|e| format!("{p}: {e}"))?;
                println!("{p}");
            }
            let r = &out.json;
            for row in r["rows"].as_array().into_iter().flatten() {
                println!("  {:<14} ours {:>6}s  reference {:>6}s  difference {:>5}  missing detail {:>5}  extra {:>5}", row["at"].as_str().unwrap_or(""), row["t"], row["reference_t"], row["mean_difference"], row["missing_detail"], row["extra_detail"]);
            }
            if r.get("scenes").is_some() {
                println!("{} frames {}–{}s, mean difference {}{}", r["frames"], r["from"], r["to"], r["mean_difference"],
                         r["video"].as_str().map(|v| format!(", video {v}")).unwrap_or_default());
                println!("  {:<16} {:>7} {:>7} {:>6} {:>7} {:>7}  verdict", "scene", "from", "to", "diff", "motion", "ref");
                for sc in r["scenes"].as_array().into_iter().flatten() {
                    println!("  {:<16} {:>7} {:>7} {:>6} {:>7} {:>7}  {}", sc["scene"].as_str().unwrap_or(""), sc["from"], sc["to"], sc["mean_difference"],
                             sc["motion_ours"], sc["motion_reference"], sc["verdict"].as_str().unwrap_or(""));
                }
                println!("  worst stretches:");
                for w in r["worst"].as_array().into_iter().flatten() {
                    println!("    {:>6}–{:<6}s  {:<36} diff {:>5}  motion ours {:>5} ref {:>5}  missing detail {:>5}", w["from"], w["to"], w["verdict"].as_str().unwrap_or(""),
                             w["mean_difference"], w["motion_ours"], w["motion_reference"], w["missing_detail"]);
                }
                if let Some(t) = r["reference_ended_at"].as_f64() {
                    println!("  the reference ends at {t}s");
                }
            }
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
        "track" => {
            let mut a = json!({"video": target.clone().ok_or("track needs a video")?});
            let nums = |s: String| -> Result<Value, String> { s.split(',').map(|v| v.trim().parse::<f64>().map(|x| json!(x)).map_err(|e| e.to_string())).collect::<Result<Vec<_>, _>>().map(Value::Array) };
            for k in ["color"] {
                if let Some(v) = flag(args, &format!("--{k}")) {
                    a[k] = json!(v);
                }
            }
            for k in ["point", "box", "region"] {
                if let Some(v) = flag(args, &format!("--{k}")) {
                    a[k] = nums(v)?;
                }
            }
            for k in ["tolerance", "search", "from", "to", "width"] {
                if let Some(v) = flag(args, &format!("--{k}")) {
                    a[k] = json!(v.parse::<f64>().map_err(|e| e.to_string())?);
                }
            }
            if let Some(w) = a.get("width").and_then(Value::as_f64) {
                a["width"] = json!(w as u64);
            }
            if has(args, "--keys") || flag(args, "--layer").is_some() {
                let mut k = json!({});
                if let Some(s) = flag(args, "--scale") {
                    k["scale"] = json!(s.parse::<f64>().map_err(|e| e.to_string())?);
                }
                if let Some(at) = flag(args, "--at") {
                    k["at"] = json!(at.parse::<f64>().map_err(|e| e.to_string())?);
                }
                if let Some(l) = flag(args, "--layer") {
                    k["layer"] = json!(l);
                }
                a["keys"] = k;
            }
            print(&tb.call("track", a)?.json);
        }
        "fit" => {
            let file = target.clone().ok_or("fit needs a samples JSON file ([[t, value], …], or a track output)")?;
            let text = std::fs::read_to_string(&file).map_err(|e| format!("{file}: {e}"))?;
            let v: Value = serde_json::from_str(&text).map_err(|e| format!("{file}: {e}"))?;
            // A track output: its found frames' centroids.
            let samples = match v.get("track").and_then(Value::as_array) {
                Some(rows) => rows.iter().filter(|r| r["found"] == true).map(|r| json!([r["t"], [r["x"], r["y"]]])).collect(),
                None => v.get("samples").cloned().unwrap_or(v),
            };
            let mut a = json!({"samples": samples});
            if let Some(f) = flag(args, "--fps") {
                a["fps"] = json!(f.parse::<f64>().map_err(|e| e.to_string())?);
            }
            print(&tb.call("fit_curve", a)?.json);
        }
        "help" | "--help" | "-h" => println!("{USAGE}"),
        other => return Err(format!("unknown command '{other}'\n\n{USAGE}")),
    }
    Ok(())
}

#[cfg(test)]
mod tests;
