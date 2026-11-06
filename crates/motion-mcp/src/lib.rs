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
  lahza motion sync-check <project> [--frames N] [--tracks a,b] [--all] [--json]
  lahza motion snapshot <project> [--at REF,REF,...] [--sheet] [--width PX] [--pt [--spp N] [--no-denoise]] [-o out.png]
  lahza motion compare <project> --against REF.mp4 [--at REF,...] [--from S] [--to S] [--offset S] [--width PX] [-o compare.png]
                                          (no --at: every frame, first to last, plus a side-by-side video)
  lahza motion track <video> (--color HEX [--tolerance N] [--region x,y,w,h] | --point x,y | --box x,y,w,h)
                    [--from S] [--to S] [--keys [--scale S] [--at S] [--layer ID]]
  lahza motion fit <samples.json> [--fps N]   samples [[t, value], …], or a track output (fits its x/y)
  lahza motion measure <video> cuts|events|ink|captions|curve [--from S] [--to S] [--at S] [--region x,y,w,h]
                    [--color HEX | --point x,y | --box x,y,w,h]   (curve: track it, fit an ease)
  lahza motion fit-camera <project> <marks.json> [--scene ID] [--fix fov,shift] [--apply]
                    marks.json: {\"frames\": [{\"at\": REF, \"marks\": [{\"world\": [x, y, z], \"image\": [x, y]}]}], \"start\": {camera}}
  lahza motion render <project> [-o out.mp4] [--draft] [--from S] [--to S] [--no-audio] [--cpu|--gpu]
                    [--pt|--raster] [--spp N] [--no-denoise]   path-trace meshes (default: the project's settings.renderer)

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

/// `--pt` / `--raster` and `--spp N` as the tools' `pt` and `samples`.
fn renderer_flags(args: &[String], a: &mut Value) -> Result<(), String> {
    if has(args, "--pt") {
        a["pt"] = json!(true);
    } else if has(args, "--raster") {
        a["pt"] = json!(false);
    }
    if let Some(n) = flag(args, "--spp") {
        a["samples"] = json!(n.parse::<u32>().map_err(|e| format!("--spp: {e}"))?);
    }
    if has(args, "--no-denoise") {
        a["denoise"] = json!(false);
    }
    Ok(())
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
                // One line per cause; one issue per cause shown in full.
                for c in r["causes"].as_array().into_iter().flatten() {
                    let elements: Vec<&str> = c["elements"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                    let what = r["issues"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|i| i["severity"] == c["severity"] && i["kind"] == c["kind"] && elements.first().is_none_or(|e| i["element"] == *e))
                        .and_then(|i| i["message"].as_str())
                        .unwrap_or_else(|| c["what"].as_str().unwrap_or(""));
                    let count = c["count"].as_u64().unwrap_or(1);
                    println!(
                        "  {:<7} {:<9} {}{}{}",
                        c["severity"].as_str().unwrap_or(""),
                        c["kind"].as_str().unwrap_or(""),
                        elements.first().map(|e| format!("[{e}] ")).unwrap_or_default(),
                        what,
                        if count > 1 { format!("  (×{count}: {})", elements.join(", ")) } else { String::new() }
                    );
                }
            }
            if out.json["ok"] != true {
                return Err("check found errors".into());
            }
        }
        "sync-check" => {
            let mut a = json!({"project": need()?, "flagged": !has(args, "--all") && !has(args, "--json"), "limit": 100000});
            if let Some(n) = flag(args, "--frames") {
                a["frames"] = json!(n.parse::<f64>().map_err(|e| e.to_string())?);
            }
            if let Some(t) = flag(args, "--tracks") {
                a["tracks"] = json!(t.split(',').map(str::trim).collect::<Vec<_>>());
            }
            let r = tb.call("sync_check", a)?.json;
            if has(args, "--json") {
                print(&r);
            } else {
                println!("{} drift, {} unsynced (within {} f of {} markers)", r["drift"], r["unsynced"], r["frames"], r["markers"]);
                if let Some(note) = r["note"].as_str() {
                    println!("  {note}");
                }
                for e in r["events"].as_array().into_iter().flatten() {
                    let near = &e["nearest"];
                    println!(
                        "  {:>8.3} s  f{:<5} {:<7} {:<8} {}.{}{}  nearest {}",
                        e["t"].as_f64().unwrap_or(0.0),
                        e["frame"],
                        e["kind"].as_str().unwrap_or(""),
                        e["flag"].as_str().unwrap_or("ok"),
                        e["element"].as_str().unwrap_or(""),
                        e["what"].as_str().unwrap_or(""),
                        e["reference"].as_str().map(|r| format!(" on {r} ({:+} f)", e["drift_frames"])).unwrap_or_default(),
                        if near.is_null() { "none".to_string() } else { format!("{} {:+} f", near["marker"].as_str().unwrap_or(""), near["frames"]) }
                    );
                }
            }
            if r["drift"].as_u64().unwrap_or(0) > 0 {
                return Err("events drift from their markers".into());
            }
        }
        "measure" => {
            let video = args.get(1).ok_or("measure needs <video> <kind>")?;
            let kind = args.get(2).ok_or("measure needs <video> <kind>")?;
            let mut a = json!({"video": video, "kind": kind});
            for f in ["from", "to", "at", "tolerance"] {
                if let Some(v) = flag(args, &format!("--{f}")) {
                    a[f] = json!(v.parse::<f64>().map_err(|e| format!("--{f}: {e}"))?);
                }
            }
            for f in ["region", "point", "box"] {
                if let Some(v) = flag(args, &format!("--{f}")) {
                    a[f] = json!(v.split(',').map(|x| x.trim().parse::<f64>().unwrap_or(0.0)).collect::<Vec<_>>());
                }
            }
            if let Some(c) = flag(args, "--color") {
                a["color"] = json!(c);
            }
            print(&tb.call("measure", a)?.json);
        }
        "fit-camera" => {
            let file = args.get(2).ok_or("fit-camera needs <project> <marks.json>")?;
            let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
            let marks: Value = serde_json::from_str(&text).map_err(|e| format!("{file}: {e}"))?;
            let mut a = json!({"project": need()?, "frames": marks["frames"].clone()});
            if !marks["start"].is_null() {
                a["start"] = marks["start"].clone();
            }
            if let Some(scene) = flag(args, "--scene") {
                a["scene"] = json!(scene);
            }
            if let Some(fix) = flag(args, "--fix") {
                a["fix"] = json!(fix.split(',').map(str::trim).collect::<Vec<_>>());
            }
            if has(args, "--apply") {
                a["apply"] = json!(true);
            }
            let out = tb.call("fit_camera", a)?.json;
            for f in out["frames"].as_array().into_iter().flatten() {
                println!("{:>10}  rms {:>6} px  {}", f["at"].as_str().unwrap_or(""), f["rms_px"], f["camera"]);
            }
            if out.get("ops").is_some() {
                print(&out["ops"]);
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
            renderer_flags(args, &mut a)?;
            let out = tb.call("snapshot", a)?;
            let base = flag(args, "-o").unwrap_or_else(|| "snapshot.png".into());
            for (i, png) in out.images.iter().enumerate() {
                let path = if out.images.len() == 1 { base.clone() } else { base.replace(".png", &format!("-{i:02}.png")) };
                std::fs::write(&path, png).map_err(|e| format!("{path}: {e}"))?;
                println!("{path}");
            }
            eprintln!("{}", serde_json::to_string(&out.json).unwrap_or_default());
            if out.json["ok"] == false {
                return Err(format!("not drawn as asked: {}", out.json["errors"]));
            }
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
            // Shading ratio (ours ÷ reference) as a percentage; "–" when the reference is flat.
            let pct = |v: &Value| v.as_f64().map(|r| format!("{:.0}%", r * 100.0)).unwrap_or_else(|| "–".into());
            for row in r["rows"].as_array().into_iter().flatten() {
                println!("  {:<14} ours {:>6}s  reference {:>6}s  difference {:>5}  missing detail {:>5}  extra {:>5}  shading {:>5}{}", row["at"].as_str().unwrap_or(""), row["t"], row["reference_t"], row["mean_difference"], row["missing_detail"], row["extra_detail"],
                         pct(&row["shading_ratio"]), row["shading_verdict"].as_str().map(|v| format!("  {v}")).unwrap_or_default());
            }
            if r.get("scenes").is_some() {
                println!("{} frames {}–{}s, mean difference {}{}", r["frames"], r["from"], r["to"], r["mean_difference"],
                         r["video"].as_str().map(|v| format!(", video {v}")).unwrap_or_default());
                println!("  {:<16} {:>7} {:>7} {:>6} {:>7} {:>7} {:>7}  verdict", "scene", "from", "to", "diff", "motion", "ref", "shading");
                for sc in r["scenes"].as_array().into_iter().flatten() {
                    println!("  {:<16} {:>7} {:>7} {:>6} {:>7} {:>7} {:>7}  {}", sc["scene"].as_str().unwrap_or(""), sc["from"], sc["to"], sc["mean_difference"],
                             sc["motion_ours"], sc["motion_reference"], pct(&sc["shading_ratio"]), sc["verdict"].as_str().unwrap_or(""));
                }
                println!("  worst stretches:");
                for w in r["worst"].as_array().into_iter().flatten() {
                    println!("    {:>6}–{:<6}s  {:<36} diff {:>5}  motion ours {:>5} ref {:>5}  missing detail {:>5}  shading {:>5}", w["from"], w["to"], w["verdict"].as_str().unwrap_or(""),
                             w["mean_difference"], w["motion_ours"], w["motion_reference"], w["missing_detail"], pct(&w["shading_ratio"]));
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
            renderer_flags(args, &mut a)?;
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
                        if s["ok"] == false {
                            return Err(format!("rendered, but not as asked: {}", s["errors"]));
                        }
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
