//! Edit operations (design section 8.5). A batch applies atomically: either
//! every op succeeds and the project is replaced, or nothing changes. Each
//! applied batch returns its inverse, which undoes it exactly.
//!
//! Ops work on the JSON form of the project and the result is re-validated
//! through the typed document, so every field the format has can be edited
//! without a dedicated op.
//!
//! ```json
//! {"op": "add", "scene": "s1", "id": "title", "kind": "text", "text": "Hello"}
//! {"op": "add_scene", "id": "s2", "start": "bar:5"}
//! {"op": "set", "target": "title.transform.position", "value": [960, 540]}
//! {"op": "set", "target": "project.settings.duration", "value": 20}
//! {"op": "remove", "id": "title"}
//! {"op": "move", "id": "title", "scene": "s2", "index": 0}
//! {"op": "key", "target": "title.transform.opacity", "at": "beat:3", "v": 1, "ease": "glide"}
//! {"op": "anchor", "target": "title.transform.opacity", "key": 1, "at": "beat:4", "anchor": "peak", "lead": -1}
//! {"op": "group", "ids": ["a", "b"], "id": "pair"}
//! {"op": "ungroup", "id": "pair"}
//! {"op": "add_sound", "id": "whoosh1", "path": "assets/sfx/whoosh.wav", "at": "#title.start - 4f", "gain_db": -6}
//! {"op": "set", "target": "whoosh1.at", "value": "beat:9"}
//! {"op": "remove", "id": "whoosh1"}
//! {"op": "insert_time", "at": 0, "duration": 5}
//! {"op": "shift", "ids": ["s07", "map"], "by": "12f"}
//! ```

use crate::document::{valid_id, Project};
use crate::{bail, Result};
use serde_json::{json, Map, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct Applied {
    pub project: Project,
    /// `insert_time` moved markers or the narration: save them too.
    pub markers_changed: bool,
    /// Ops that undo this batch, in order.
    pub inverse: Vec<Value>,
    /// Ids created by `add`, `add_scene` and `group`, in op order.
    pub created: Vec<String>,
    /// Ids whose content changed.
    pub changed: Vec<String>,
    /// One line per op.
    pub summary: Vec<String>,
}

pub fn apply(project: &Project, ops: &[Value]) -> Result<Applied> {
    let mut doc = serde_json::to_value(project).map_err(|e| e.to_string())?;
    let base = time_base(project);
    // Time inserted (at, duration), applied to markers and narration below.
    let mut ripples: Vec<(f64, f64)> = Vec::new();
    let mut inverse = Vec::new();
    let mut created = Vec::new();
    let mut changed = Vec::new();
    let mut summary = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        let name = op.get("op").and_then(Value::as_str).unwrap_or("");
        let result = match child_target(op) {
            Some((component, child)) => op_child(&mut doc, op, &component, &child, project, &base, &mut ripples, &mut created),
            None => dispatch(&mut doc, op, &base, &mut ripples, &mut created),
        };
        let (inv, ids, line) = result.map_err(|e| format!("op {} ({name}): {e}", i + 1))?;
        inverse.splice(0..0, inv);
        changed.extend(ids);
        summary.push(line);
    }
    let mut next: Project = serde_json::from_value(doc).map_err(|e| format!("result is not a valid project: {e}"))?;
    next.markers = project.markers.clone();
    next.narration = project.narration.clone();
    let markers_changed = !ripples.is_empty();
    for (at, d) in ripples {
        ripple_markers(&mut next, project, at, d);
    }
    changed.sort();
    changed.dedup();
    Ok(Applied { project: next, markers_changed, inverse, created, changed, summary })
}

type OpResult = Result<(Vec<Value>, Vec<String>, String)>;

fn dispatch(doc: &mut Value, op: &Value, base: &crate::time::TimeBase, ripples: &mut Vec<(f64, f64)>, created: &mut Vec<String>) -> OpResult {
    match op.get("op").and_then(Value::as_str).unwrap_or("") {
        "add" => op_add(doc, op, created),
        "add_scene" => op_add_scene(doc, op, created),
        "set" => op_set(doc, op),
        "remove" => op_remove(doc, op),
        "move" => op_move(doc, op),
        "key" => op_key(doc, op),
        "anchor" => op_anchor(doc, op),
        "group" => op_group(doc, op, created),
        "ungroup" => op_ungroup(doc, op),
        "add_sound" => op_add_sound(doc, op, created),
        "insert_time" => op_insert_time(doc, op, base, ripples),
        "shift" => op_shift(doc, op, base),
        "detach" => op_detach(doc, op),
        "set_override" => op_set_override(doc, op),
        "replace_layer" => op_replace_layer(doc, op),
        "" => Err("op needs an \"op\" field".into()),
        other => Err(format!(
            "unknown op '{other}': use add, add_scene, set, remove, move, key, anchor, group, ungroup, add_sound, insert_time, shift, detach"
        )
        .into()),
    }
}

// ------------------------------------------------------------ components

/// `map/IRN.fill` (or `"id": "map/IRN"`): the component and child an op
/// targets.
fn child_target(op: &Value) -> Option<(String, String)> {
    let target = op.get("target").and_then(Value::as_str).map(|t| t.split('.').next().unwrap_or(t)).or_else(|| op.get("id").and_then(Value::as_str))?;
    let (component, child) = target.split_once('/')?;
    Some((component.to_string(), child.to_string()))
}

fn component_ctx(project: &Project) -> crate::components::Ctx<'_> {
    crate::components::Ctx { width: project.settings.width as f64, height: project.settings.height as f64, fps: project.settings.fps, markers: &project.markers }
}

/// An op on a generated child: done on the child as generated (with its
/// current override), and the difference kept as the child's override,
/// so it survives regeneration.
#[allow(clippy::too_many_arguments)]
fn op_child(doc: &mut Value, op: &Value, component: &str, child: &str, project: &Project, base: &crate::time::TimeBase, ripples: &mut Vec<(f64, f64)>, created: &mut Vec<String>) -> OpResult {
    let name = op.get("op").and_then(Value::as_str).unwrap_or("");
    if !["set", "key", "anchor", "remove"].contains(&name) {
        bail!("'{name}' does not apply to a component's child ({component}/{child}): use set, key, anchor or remove, or detach the component first");
    }
    let (ptr, _, _) = find(doc, component).ok_or_else(|| format!("no component '{component}'"))?;
    let layer = doc.pointer(&ptr).cloned().unwrap_or_default();
    if layer.get("kind").and_then(Value::as_str) != Some("component") {
        bail!("'{component}' is not a component");
    }
    let kind = layer["component"].as_str().unwrap_or("").to_string();
    let params = layer.get("params").cloned().unwrap_or_else(|| json!({}));
    let generated = crate::components::generate(&kind, &params, &component_ctx(project))?;
    let base_child = generated.into_iter().find(|c| c["id"] == child).ok_or_else(|| format!("{component} has no child '{child}'"))?;
    let old = layer.pointer(&format!("/overrides/{child}")).cloned();
    let mut current = base_child.clone();
    match &old {
        Some(Value::Null) => bail!("{component}/{child} was removed; set {component}.overrides to bring it back"),
        Some(patch) => crate::components::merge_patch(&mut current, patch),
        None => {}
    }
    let new_patch = if name == "remove" {
        Some(Value::Null)
    } else {
        // The op, on a scratch document holding just this child.
        let mut scratch = json!({"settings": doc["settings"].clone(), "scenes": [{"id": "__component", "layers": [current]}]});
        let mut local = op.clone();
        if let Some(t) = op.get("target").and_then(Value::as_str) {
            local["target"] = json!(t.split_once('/').map(|(_, rest)| rest).unwrap_or(t));
        }
        dispatch(&mut scratch, &local, base, ripples, created)?;
        crate::components::diff_patch(&base_child, &scratch["scenes"][0]["layers"][0])
    };
    let overrides = doc.pointer_mut(&ptr).unwrap().as_object_mut().unwrap().entry("overrides").or_insert_with(|| json!({}));
    let map = overrides.as_object_mut().ok_or("overrides is not an object")?;
    match &new_patch {
        Some(p) => {
            map.insert(child.into(), p.clone());
        }
        None => {
            map.remove(child);
        }
    }
    let inverse = vec![json!({"op": "set_override", "id": component, "child": child, "value": old.clone().unwrap_or(json!("__none"))})];
    Ok((inverse, vec![format!("{component}/{child}")], format!("{name} on {component}/{child} (kept as an override)")))
}

fn op_set_override(doc: &mut Value, op: &Value) -> OpResult {
    let id = op.get("id").and_then(Value::as_str).ok_or("set_override needs \"id\"")?;
    let child = op.get("child").and_then(Value::as_str).ok_or("set_override needs \"child\"")?;
    let (ptr, _, _) = find(doc, id).ok_or_else(|| format!("no component '{id}'"))?;
    let layer = doc.pointer_mut(&ptr).unwrap().as_object_mut().ok_or("bad layer")?;
    let overrides = layer.entry("overrides").or_insert_with(|| json!({}));
    let old = overrides.get(child).cloned();
    let map = overrides.as_object_mut().ok_or("overrides is not an object")?;
    match op.get("value") {
        Some(Value::String(s)) if s == "__none" => {
            map.remove(child);
        }
        Some(v) => {
            map.insert(child.into(), v.clone());
        }
        None => bail!("set_override needs \"value\""),
    }
    if map.is_empty() {
        layer.remove("overrides");
    }
    Ok((vec![json!({"op": "set_override", "id": id, "child": child, "value": old.unwrap_or(json!("__none"))})], vec![id.to_string()], format!("override of {id}/{child}")))
}

/// Turns a component into a comp of its children as plain layers (ids
/// `<component>-<child>`), overrides applied.
fn op_detach(doc: &mut Value, op: &Value) -> OpResult {
    let id = op.get("id").and_then(Value::as_str).ok_or("detach needs \"id\"")?;
    let (ptr, _, _) = find(doc, id).ok_or_else(|| format!("no component '{id}'"))?;
    let layer = doc.pointer(&ptr).cloned().unwrap_or_default();
    if layer.get("kind").and_then(Value::as_str) != Some("component") {
        bail!("'{id}' is not a component");
    }
    let project: Project = serde_json::from_value(doc.clone()).map_err(|e| e.to_string())?;
    let kind = layer["component"].as_str().unwrap_or("");
    let overrides = layer.get("overrides").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut children = crate::components::children(kind, layer.get("params").unwrap_or(&json!({})), &overrides, &component_ctx(&project))?;
    crate::components::rename(&mut children, &format!("{id}-"), true);
    for c in &children {
        let cid = c["id"].as_str().unwrap_or("");
        if exists(doc, cid) {
            bail!("detaching {id} would make a second '{cid}'");
        }
    }
    let mut comp = layer.clone();
    let obj = comp.as_object_mut().unwrap();
    for k in ["component", "params", "overrides"] {
        obj.remove(k);
    }
    obj.insert("kind".into(), json!("comp"));
    obj.insert("layers".into(), Value::Array(children));
    *doc.pointer_mut(&ptr).unwrap() = comp;
    Ok((vec![json!({"op": "replace_layer", "id": id, "layer": layer})], vec![id.to_string()], format!("detached {id} into plain layers")))
}

fn op_replace_layer(doc: &mut Value, op: &Value) -> OpResult {
    let id = op.get("id").and_then(Value::as_str).ok_or("replace_layer needs \"id\"")?;
    let new = op.get("layer").cloned().ok_or("replace_layer needs \"layer\"")?;
    let (ptr, _, _) = find(doc, id).ok_or_else(|| format!("no layer '{id}'"))?;
    let old = std::mem::replace(doc.pointer_mut(&ptr).unwrap(), new);
    Ok((vec![json!({"op": "replace_layer", "id": id, "layer": old})], vec![id.to_string()], format!("replaced {id}")))
}

// ------------------------------------------------------------ locating

/// JSON pointer of the element with `id`, and its container pointer.
fn find(doc: &Value, id: &str) -> Option<(String, String, usize)> {
    if id == "project" {
        return Some((String::new(), String::new(), 0));
    }
    let scenes = doc.get("scenes")?.as_array()?;
    for (si, scene) in scenes.iter().enumerate() {
        if scene.get("id").and_then(Value::as_str) == Some(id) {
            return Some((format!("/scenes/{si}"), "/scenes".into(), si));
        }
        if let Some(found) = find_layer(scene, &format!("/scenes/{si}"), id) {
            return Some(found);
        }
    }
    find_sound(doc, id)
}

/// Sounds: effects by id, music and narration by id or as "music"/"vo".
fn find_sound(doc: &Value, id: &str) -> Option<(String, String, usize)> {
    let audio = doc.get("audio")?;
    for slot in ["music", "vo"] {
        if let Some(clip) = audio.get(slot).filter(|c| c.is_object()) {
            if id == slot || clip.get("id").and_then(Value::as_str) == Some(id) {
                return Some((format!("/audio/{slot}"), "/audio".into(), 0));
            }
        }
    }
    let sfx = audio.get("sfx")?.as_array()?;
    let i = sfx.iter().position(|c| c.get("id").and_then(Value::as_str) == Some(id))?;
    Some((format!("/audio/sfx/{i}"), "/audio/sfx".into(), i))
}

/// Adds a sound: an effect (default), the music or the narration.
fn op_add_sound(doc: &mut Value, op: &Value, created: &mut Vec<String>) -> OpResult {
    let kind = op.get("kind").and_then(Value::as_str).unwrap_or("sfx");
    let mut clip = match op.get("sound") {
        Some(s) => s.clone(),
        None => {
            let mut m = Map::new();
            for (k, v) in op.as_object().unwrap() {
                if !["op", "kind", "index", "sound"].contains(&k.as_str()) {
                    m.insert(k.clone(), v.clone());
                }
            }
            Value::Object(m)
        }
    };
    if clip.get("path").and_then(Value::as_str).is_none() {
        bail!("add_sound needs \"path\" (an audio file in the project)");
    }
    let audio = doc.as_object_mut().unwrap().entry("audio").or_insert_with(|| json!({}));
    if !audio.is_object() {
        *audio = json!({});
    }
    match kind {
        "music" | "vo" => {
            if audio.get(kind).is_some_and(|v| !v.is_null()) {
                bail!("the project already has {kind}: remove \"{kind}\" first, or set its fields");
            }
            let id = clip.get("id").and_then(Value::as_str).unwrap_or(kind).to_string();
            audio[kind] = clip;
            let audio_clip: Result<crate::document::AudioClip> = serde_json::from_value(audio[kind].clone()).map_err(|e| e.to_string().into());
            if let Err(e) = audio_clip {
                audio.as_object_mut().unwrap().remove(kind);
                bail!("add_sound: {e}");
            }
            created.push(id.clone());
            Ok((vec![json!({"op": "remove", "id": id})], vec![id], format!("added {kind}")))
        }
        "sfx" => {
            let id = match clip.get("id").and_then(Value::as_str) {
                Some(id) => {
                    if !valid_id(id) {
                        bail!("sound id '{id}': letters, digits, '-' and '_' only");
                    }
                    id.to_string()
                }
                None => {
                    let stem = clip["path"].as_str().unwrap().rsplit('/').next().unwrap_or("sfx").split('.').next().unwrap_or("sfx").to_string();
                    let stem: String = stem.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
                    let stem = if stem.is_empty() { "sfx".to_string() } else { stem };
                    let doc_ref: &Value = doc;
                    (1..).map(|n| format!("{stem}{n}")).find(|c| !exists(doc_ref, c)).unwrap()
                }
            };
            if exists(doc, &id) {
                bail!("id '{id}' is already used");
            }
            clip["id"] = json!(id);
            serde_json::from_value::<crate::document::AudioClip>(clip.clone()).map_err(|e| format!("add_sound: {e}"))?;
            let audio = doc.as_object_mut().unwrap().get_mut("audio").unwrap();
            let sfx = audio.as_object_mut().unwrap().entry("sfx").or_insert_with(|| json!([]));
            let list = sfx.as_array_mut().ok_or("audio.sfx is not a list")?;
            let index = op.get("index").and_then(Value::as_u64).map(|i| (i as usize).min(list.len())).unwrap_or(list.len());
            list.insert(index, clip);
            created.push(id.clone());
            Ok((vec![json!({"op": "remove", "id": id})], vec![id.clone()], format!("added sound {id}")))
        }
        other => bail!("add_sound kind '{other}': sfx, music or vo"),
    }
}

fn find_layer(node: &Value, path: &str, id: &str) -> Option<(String, String, usize)> {
    let layers = node.get("layers")?.as_array()?;
    let container = format!("{path}/layers");
    for (i, layer) in layers.iter().enumerate() {
        let here = format!("{container}/{i}");
        if layer.get("id").and_then(Value::as_str) == Some(id) {
            return Some((here, container, i));
        }
        if let Some(found) = find_layer(layer, &here, id) {
            return Some(found);
        }
    }
    None
}

fn exists(doc: &Value, id: &str) -> bool {
    id == "project" || find(doc, id).is_some()
}

/// Dotted target `id.path.to.field` → JSON pointer.
fn target_pointer(doc: &Value, target: &str) -> Result<(String, String)> {
    let (id, rest) = target.split_once('.').unwrap_or((target, ""));
    let Some((pointer, _, _)) = find(doc, id) else {
        bail!("no layer or scene with id '{id}'");
    };
    let mut p = pointer;
    if !rest.is_empty() {
        for seg in rest.split('.') {
            if seg.is_empty() {
                bail!("empty path segment in '{target}'");
            }
            p.push('/');
            p.push_str(&seg.replace('~', "~0").replace('/', "~1"));
        }
    }
    Ok((id.to_string(), p))
}

fn container_of(doc: &Value, scene: Option<&str>, parent: Option<&str>) -> Result<String> {
    if let Some(parent) = parent {
        let Some((p, _, _)) = find(doc, parent) else { bail!("no comp '{parent}'") };
        let node = doc.pointer(&p).unwrap();
        if node.get("kind").and_then(Value::as_str) != Some("comp") {
            bail!("'{parent}' is not a comp layer");
        }
        return Ok(format!("{p}/layers"));
    }
    let scenes = doc.get("scenes").and_then(Value::as_array).cloned().unwrap_or_default();
    let index = match scene {
        Some(id) => scenes
            .iter()
            .position(|s| s.get("id").and_then(Value::as_str) == Some(id))
            .ok_or_else(|| format!("no scene '{id}'"))?,
        None if scenes.is_empty() => bail!("the project has no scenes: add_scene first"),
        None => scenes.len() - 1,
    };
    Ok(format!("/scenes/{index}/layers"))
}

fn array_mut<'a>(doc: &'a mut Value, pointer: &str) -> Result<&'a mut Vec<Value>> {
    let parent_ptr = pointer.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
    let key = pointer.rsplit_once('/').map(|(_, k)| k).unwrap_or("");
    if doc.pointer(pointer).is_none() {
        let parent = doc
            .pointer_mut(parent_ptr)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| format!("no container at {pointer}"))?;
        parent.insert(key.to_string(), json!([]));
    }
    doc.pointer_mut(pointer)
        .and_then(Value::as_array_mut)
        .ok_or_else(|| format!("{pointer} is not a list").into())
}

/// The dotted target for a container pointer (inverse ops must survive
/// index shifts, so they address containers by id).
fn container_target(doc: &Value, container: &str) -> String {
    let owner = container.strip_suffix("/layers").unwrap_or(container);
    let id = doc.pointer(owner).and_then(|v| v.get("id")).and_then(Value::as_str).unwrap_or("project");
    format!("{id}.layers")
}

fn ids_in(value: &Value, out: &mut Vec<String>) {
    if let Some(id) = value.get("id").and_then(Value::as_str) {
        out.push(id.to_string());
    }
    if let Some(layers) = value.get("layers").and_then(Value::as_array) {
        for l in layers {
            ids_in(l, out);
        }
    }
}

fn fresh_id(doc: &Value, prefix: &str) -> String {
    (1..).map(|n| format!("{prefix}-{n}")).find(|id| !exists(doc, id)).unwrap()
}

// ----------------------------------------------------------------- ops

const ADD_KEYS: &[&str] = &["op", "scene", "parent", "index", "layer"];

fn op_add(doc: &mut Value, op: &Value, created: &mut Vec<String>) -> OpResult {
    let mut layer = match op.get("layer") {
        Some(l) => l.clone(),
        None => {
            let mut m = Map::new();
            for (k, v) in op.as_object().unwrap() {
                if !ADD_KEYS.contains(&k.as_str()) {
                    m.insert(k.clone(), v.clone());
                }
            }
            Value::Object(m)
        }
    };
    if !layer.is_object() {
        bail!("layer must be an object");
    }
    if layer.get("id").is_none() {
        let kind = layer.get("kind").and_then(Value::as_str).unwrap_or("layer").to_string();
        layer["id"] = fresh_id(doc, &kind).into();
    }
    let mut ids = Vec::new();
    ids_in(&layer, &mut ids);
    for id in &ids {
        if !valid_id(id) {
            bail!("bad id '{id}': use letters, digits, '_' and '-'");
        }
        if exists(doc, id) {
            bail!("id '{id}' already exists");
        }
    }
    // Validate early so the error names this op.
    crate::document::Layer::from_json(layer.clone())?;
    let container = container_of(doc, op.get("scene").and_then(Value::as_str), op.get("parent").and_then(Value::as_str))?;
    let list = array_mut(doc, &container)?;
    let index = op.get("index").and_then(Value::as_u64).map(|i| i as usize).unwrap_or(list.len()).min(list.len());
    let id = ids[0].clone();
    list.insert(index, layer);
    created.push(id.clone());
    Ok((vec![json!({"op": "remove", "id": id})], ids, format!("added {id}")))
}

fn op_add_scene(doc: &mut Value, op: &Value, created: &mut Vec<String>) -> OpResult {
    let mut scene = match op.get("scene") {
        Some(s) if s.is_object() => s.clone(),
        _ => {
            let mut m = Map::new();
            for (k, v) in op.as_object().unwrap() {
                if k != "op" && k != "index" {
                    m.insert(k.clone(), v.clone());
                }
            }
            Value::Object(m)
        }
    };
    if scene.get("id").is_none() {
        scene["id"] = fresh_id(doc, "scene").into();
    }
    let mut ids = Vec::new();
    ids_in(&scene, &mut ids);
    for id in &ids {
        if !valid_id(id) {
            bail!("bad id '{id}'");
        }
        if exists(doc, id) {
            bail!("id '{id}' already exists");
        }
    }
    serde_json::from_value::<crate::document::Scene>(scene.clone()).map_err(|e| e.to_string())?;
    let list = array_mut(doc, "/scenes")?;
    let index = op.get("index").and_then(Value::as_u64).map(|i| i as usize).unwrap_or(list.len()).min(list.len());
    let id = ids[0].clone();
    list.insert(index, scene);
    created.push(id.clone());
    Ok((vec![json!({"op": "remove", "id": id})], ids, format!("added scene {id}")))
}

fn op_set(doc: &mut Value, op: &Value) -> OpResult {
    let target = op.get("target").and_then(Value::as_str).ok_or("set needs \"target\": \"<id>.<path>\"")?;
    let (id, pointer) = target_pointer(doc, target)?;
    if !target.contains('.') {
        bail!("set needs a field path after the id, e.g. '{id}.transform.position'");
    }
    if target.ends_with(".id") {
        bail!("ids are stable and cannot be changed");
    }
    let old = doc.pointer(&pointer).cloned();
    let value = op.get("value").cloned();
    set_pointer(doc, &pointer, value.clone())?;
    let inverse = match old {
        Some(v) => json!({"op": "set", "target": target, "value": v}),
        None => json!({"op": "set", "target": target}),
    };
    Ok((vec![inverse], vec![id], format!("set {target}")))
}

/// Set (Some) or remove (None) the value at a pointer, creating objects.
fn set_pointer(doc: &mut Value, pointer: &str, value: Option<Value>) -> Result<()> {
    let (parent, key) = pointer.rsplit_once('/').ok_or("cannot replace the whole project")?;
    let key = key.replace("~1", "/").replace("~0", "~");
    // Create missing parents.
    let mut cur = String::new();
    for seg in parent.split('/').skip(1) {
        let next = format!("{cur}/{seg}");
        if doc.pointer(&next).is_none() {
            if value.is_none() {
                return Ok(());
            }
            let holder = doc.pointer_mut(&cur).ok_or_else(|| format!("no field at {cur}"))?;
            match holder {
                Value::Object(m) => {
                    m.insert(seg.replace("~1", "/").replace("~0", "~"), json!({}));
                }
                _ => bail!("cannot create '{seg}' inside a non-object"),
            }
        }
        cur = next;
    }
    let holder = doc.pointer_mut(parent).ok_or_else(|| format!("no field at {parent}"))?;
    match (holder, value) {
        (Value::Object(m), Some(v)) => {
            m.insert(key, v);
        }
        (Value::Object(m), None) => {
            m.remove(&key);
        }
        (Value::Array(a), Some(v)) => {
            let i: usize = key.parse().map_err(|_| format!("'{key}' is not a list index"))?;
            if i < a.len() {
                a[i] = v;
            } else if i == a.len() {
                a.push(v);
            } else {
                bail!("index {i} is past the end of a list of {}", a.len());
            }
        }
        (Value::Array(a), None) => {
            let i: usize = key.parse().map_err(|_| format!("'{key}' is not a list index"))?;
            if i < a.len() {
                a.remove(i);
            }
        }
        _ => bail!("cannot set a field inside a non-object"),
    }
    Ok(())
}

fn op_remove(doc: &mut Value, op: &Value) -> OpResult {
    let id = op.get("id").and_then(Value::as_str).ok_or("remove needs \"id\"")?;
    if id == "project" {
        bail!("cannot remove the project");
    }
    let (pointer, container, index) = find(doc, id).ok_or_else(|| format!("no layer or scene with id '{id}'"))?;
    if container == "/audio" {
        // Music or narration.
        let slot = pointer.trim_start_matches("/audio/").to_string();
        let removed = doc["audio"].as_object_mut().unwrap().remove(&slot).unwrap_or(Value::Null);
        return Ok((vec![json!({"op": "add_sound", "kind": slot, "sound": removed})], vec![id.into()], format!("removed {slot}")));
    }
    if container == "/audio/sfx" {
        let removed = doc["audio"]["sfx"].as_array_mut().unwrap().remove(index);
        return Ok((vec![json!({"op": "add_sound", "kind": "sfx", "index": index, "sound": removed})], vec![id.into()], format!("removed sound {id}")));
    }
    let target = container_target(doc, &container);
    let list = doc.pointer_mut(&container).and_then(Value::as_array_mut).unwrap();
    let removed = list.remove(index);
    let mut ids = Vec::new();
    ids_in(&removed, &mut ids);
    let inverse = if container == "/scenes" {
        json!({"op": "add_scene", "scene": removed, "index": index})
    } else {
        let owner = target.trim_end_matches(".layers");
        let is_scene = doc
            .get("scenes")
            .and_then(Value::as_array)
            .is_some_and(|s| s.iter().any(|s| s.get("id").and_then(Value::as_str) == Some(owner)));
        if is_scene {
            json!({"op": "add", "scene": owner, "index": index, "layer": removed})
        } else {
            json!({"op": "add", "parent": owner, "index": index, "layer": removed})
        }
    };
    Ok((vec![inverse], ids, format!("removed {id}")))
}

fn op_move(doc: &mut Value, op: &Value) -> OpResult {
    let id = op.get("id").and_then(Value::as_str).ok_or("move needs \"id\"")?;
    let (_, container, index) = find(doc, id).ok_or_else(|| format!("no layer with id '{id}'"))?;
    if container == "/scenes" {
        let list = array_mut(doc, "/scenes")?;
        let scene = list.remove(index);
        let to = op.get("index").and_then(Value::as_u64).map(|i| i as usize).unwrap_or(list.len()).min(list.len());
        list.insert(to, scene);
        return Ok((vec![json!({"op": "move", "id": id, "index": index})], vec![id.into()], format!("moved scene {id}")));
    }
    let old_owner = container_target(doc, &container);
    let old_owner = old_owner.trim_end_matches(".layers").to_string();
    let layer = doc.pointer_mut(&container).and_then(Value::as_array_mut).unwrap().remove(index);
    let scene = op.get("scene").and_then(Value::as_str);
    let parent = op.get("parent").and_then(Value::as_str);
    if parent == Some(id) {
        bail!("cannot move a layer into itself");
    }
    let dest = if scene.is_none() && parent.is_none() {
        container.clone()
    } else {
        container_of(doc, scene, parent).map_err(|e| {
            if find(doc, parent.unwrap_or("")).is_none() && parent.is_some() {
                format!("{e} (moving a comp into its own child is not allowed)").into()
            } else {
                e
            }
        })?
    };
    let list = array_mut(doc, &dest)?;
    let to = op.get("index").and_then(Value::as_u64).map(|i| i as usize).unwrap_or(list.len()).min(list.len());
    list.insert(to, layer);
    let back_is_scene = doc
        .get("scenes")
        .and_then(Value::as_array)
        .is_some_and(|s| s.iter().any(|s| s.get("id").and_then(Value::as_str) == Some(old_owner.as_str())));
    let inverse = if back_is_scene {
        json!({"op": "move", "id": id, "scene": old_owner, "index": index})
    } else {
        json!({"op": "move", "id": id, "parent": old_owner, "index": index})
    };
    Ok((vec![inverse], vec![id.into()], format!("moved {id}")))
}

/// Defaults of animatable fields that may be absent from the JSON.
fn property_default(doc: &Value, id: &str, path: &str) -> Option<Value> {
    let node = find(doc, id).and_then(|(p, _, _)| doc.pointer(&p).cloned()).unwrap_or(Value::Null);
    let kind = node.get("kind").and_then(Value::as_str).unwrap_or("");
    Some(match path {
        "transform.position" | "transform.anchor" => json!([0, 0]),
        "transform.scale" => json!([1, 1]),
        "transform.rotation" | "camera.rotation" | "radius" | "tracking" | "weight" | "edge.angle" => json!(0),
        "edge.width" => json!(3),
        "transform.opacity" | "camera.zoom" => json!(1),
        "camera.position" => {
            let w = doc.pointer("/settings/width").and_then(Value::as_f64).unwrap_or(1920.0);
            let h = doc.pointer("/settings/height").and_then(Value::as_f64).unwrap_or(1080.0);
            json!([w / 2.0, h / 2.0])
        }
        "size" if kind == "text" => json!(96),
        "size" => json!([100, 100]),
        "thickness" => json!(10),
        "inner" => json!(0.5),
        "angles" => json!([0, 270]),
        "trim" => json!([0, 1]),
        "color" => json!("#ffffff"),
        "stroke.width" => json!(4),
        _ => return None,
    })
}

const KEY_FIELDS: &[&str] = &["at", "t", "v", "value", "to", "from", "ease", "anchor", "lead", "dur", "space"];

fn op_key(doc: &mut Value, op: &Value) -> OpResult {
    let target = op.get("target").and_then(Value::as_str).ok_or("key needs \"target\": \"<id>.<property>\"")?;
    let (id, pointer) = target_pointer(doc, target)?;
    let path = target.split_once('.').map(|(_, p)| p).unwrap_or("");
    let mut key = Map::new();
    for (k, v) in op.as_object().unwrap() {
        if KEY_FIELDS.contains(&k.as_str()) {
            let k = match k.as_str() {
                "t" => "at",
                "value" | "to" => "v",
                other => other,
            };
            key.insert(k.into(), v.clone());
        }
    }
    if !key.contains_key("at") || !key.contains_key("v") {
        bail!("key needs \"at\" and \"v\"");
    }
    let old = doc.pointer(&pointer).cloned();
    let current = old.clone().or_else(|| property_default(doc, &id, path));
    let next = match current {
        Some(Value::Object(mut prop)) if prop.contains_key("keys") || prop.contains_key("add") => {
            let keys = prop.entry("keys").or_insert(json!([]));
            let keys = keys.as_array_mut().ok_or("keys must be a list")?;
            if keys.is_empty() && !key.contains_key("from") {
                if let Some(base) = prop.get("base").cloned() {
                    key.insert("from".into(), base);
                }
            }
            let keys = prop.get_mut("keys").unwrap().as_array_mut().unwrap();
            keys.push(Value::Object(key));
            Value::Object(prop)
        }
        Some(constant) => {
            if !key.contains_key("from") {
                key.insert("from".into(), constant);
            }
            json!({"keys": [Value::Object(key)]})
        }
        None => bail!("'{target}' has no value to animate from; give \"from\""),
    };
    set_pointer(doc, &pointer, Some(next))?;
    let inverse = match old {
        Some(v) => json!({"op": "set", "target": target, "value": v}),
        None => json!({"op": "set", "target": target}),
    };
    Ok((vec![inverse], vec![id], format!("keyed {target}")))
}

fn op_anchor(doc: &mut Value, op: &Value) -> OpResult {
    let target = op.get("target").and_then(Value::as_str).ok_or("anchor needs \"target\"")?;
    let (id, pointer) = target_pointer(doc, target)?;
    let n = op.get("key").and_then(Value::as_u64).unwrap_or(1) as usize;
    let old = doc.pointer(&pointer).cloned().ok_or_else(|| format!("'{target}' is not set"))?;
    let mut next = old.clone();
    let keys = next
        .get_mut("keys")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| format!("'{target}' has no keys"))?;
    let len = keys.len();
    let key = keys
        .get_mut(n.max(1) - 1)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| format!("'{target}' has {len} keys, asked for key {n}"))?;
    for field in ["at", "anchor", "lead", "dur", "ease"] {
        if let Some(v) = op.get(field) {
            if v.is_null() {
                key.remove(field);
            } else {
                key.insert(field.into(), v.clone());
            }
        }
    }
    set_pointer(doc, &pointer, Some(next))?;
    Ok((vec![json!({"op": "set", "target": target, "value": old})], vec![id], format!("anchored {target} key {n}")))
}

fn op_group(doc: &mut Value, op: &Value, created: &mut Vec<String>) -> OpResult {
    let ids: Vec<String> = op
        .get("ids")
        .and_then(Value::as_array)
        .ok_or("group needs \"ids\": [...]")?
        .iter()
        .map(|v| v.as_str().map(String::from).ok_or("ids must be strings"))
        .collect::<std::result::Result<_, _>>()?;
    if ids.is_empty() {
        bail!("group needs at least one id");
    }
    let mut container = None;
    for id in &ids {
        let (_, c, _) = find(doc, id).ok_or_else(|| format!("no layer '{id}'"))?;
        if c == "/scenes" {
            bail!("'{id}' is a scene; only layers can be grouped");
        }
        if container.get_or_insert(c.clone()) != &c {
            bail!("grouped layers must share one parent");
        }
    }
    let container = container.unwrap();
    let group_id = match op.get("id").and_then(Value::as_str) {
        Some(id) if exists(doc, id) => bail!("id '{id}' already exists"),
        Some(id) if !valid_id(id) => bail!("bad id '{id}'"),
        Some(id) => id.to_string(),
        None => fresh_id(doc, "group"),
    };
    let target = container_target(doc, &container);
    let list = doc.pointer_mut(&container).and_then(Value::as_array_mut).unwrap();
    let old = Value::Array(list.clone());
    let first = list.iter().position(|l| ids.iter().any(|id| l.get("id").and_then(Value::as_str) == Some(id))).unwrap();
    let mut members = Vec::new();
    list.retain(|l| {
        let take = ids.iter().any(|id| l.get("id").and_then(Value::as_str) == Some(id));
        if take {
            members.push(l.clone());
        }
        !take
    });
    let mut group = json!({"id": group_id, "kind": "comp", "layers": members});
    if let Some(extra) = op.get("layer").and_then(Value::as_object) {
        for (k, v) in extra {
            group[k] = v.clone();
        }
    }
    list.insert(first.min(list.len()), group);
    created.push(group_id.clone());
    let mut changed = ids;
    changed.push(group_id.clone());
    Ok((vec![json!({"op": "set", "target": target, "value": old})], changed, format!("grouped into {group_id}")))
}

fn op_ungroup(doc: &mut Value, op: &Value) -> OpResult {
    let id = op.get("id").and_then(Value::as_str).ok_or("ungroup needs \"id\"")?;
    let (pointer, container, index) = find(doc, id).ok_or_else(|| format!("no layer '{id}'"))?;
    let node = doc.pointer(&pointer).cloned().unwrap();
    if node.get("kind").and_then(Value::as_str) != Some("comp") {
        bail!("'{id}' is not a comp");
    }
    let target = container_target(doc, &container);
    let list = doc.pointer_mut(&container).and_then(Value::as_array_mut).unwrap();
    let old = Value::Array(list.clone());
    list.remove(index);
    let children = node.get("layers").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut changed = vec![id.to_string()];
    for (k, child) in children.into_iter().enumerate() {
        ids_in(&child, &mut changed);
        list.insert(index + k, child);
    }
    Ok((vec![json!({"op": "set", "target": target, "value": old})], changed, format!("ungrouped {id} (its transform is dropped)")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::Fps;

    fn base() -> Project {
        let mut p = Project::new(1920, 1080, Fps::whole(30), 10.0);
        p.scenes.push(crate::document::Scene::new("s1"));
        p
    }

    #[test]
    fn sounds_are_added_retimed_and_removed_with_undo() {
        let p = base();
        let p = apply(&p, &[json!({"op": "add", "scene": "s1", "id": "card", "kind": "shape", "shape": "rect", "time": {"start": 2.0}})]).unwrap().project;
        let a = apply(&p, &[
            json!({"op": "add_sound", "path": "assets/sfx/whoosh.wav", "at": "#card.start - 4f", "gain_db": -6}),
            json!({"op": "add_sound", "path": "assets/sfx/whoosh.wav", "at": "beat:3"}),
            json!({"op": "add_sound", "kind": "music", "path": "assets/track.wav"}),
        ]).unwrap();
        assert_eq!(a.created, vec!["whoosh1", "whoosh2", "music"]);
        assert_eq!(a.project.audio.sfx.len(), 2);
        assert_eq!(a.project.audio.sfx[0].gain_db, -6.0);
        assert!(a.project.audio.music.is_some());
        // Retime and cut an effect through the generic set op.
        let b = apply(&a.project, &[json!({"op": "set", "target": "whoosh2.at", "value": 1.5}), json!({"op": "set", "target": "whoosh2.length", "value": 0.4})]).unwrap();
        assert_eq!(b.project.audio.sfx[1].length, Some(0.4));
        // Undo restores the previous timing.
        let undone = apply(&b.project, &b.inverse.iter().rev().cloned().collect::<Vec<_>>()).unwrap();
        assert_eq!(undone.project.audio.sfx[1], a.project.audio.sfx[1]);
        // Remove, then undo puts it back at the same place.
        let c = apply(&a.project, &[json!({"op": "remove", "id": "whoosh1"}), json!({"op": "remove", "id": "music"})]).unwrap();
        assert_eq!(c.project.audio.sfx.len(), 1);
        assert!(c.project.audio.music.is_none());
        let back = apply(&c.project, &c.inverse.iter().rev().cloned().collect::<Vec<_>>()).unwrap();
        assert_eq!(back.project.audio, a.project.audio);
        // Ids stay unique across layers and sounds.
        assert!(apply(&a.project, &[json!({"op": "add_sound", "id": "card", "path": "x.wav"})]).is_err());
        assert!(apply(&a.project, &[json!({"op": "add_sound", "kind": "music", "path": "x.wav"})]).is_err());
    }

    #[test]
    fn batch_applies_and_inverts_exactly() {
        let p0 = base();
        let ops = vec![
            json!({"op": "add", "scene": "s1", "id": "title", "kind": "text", "text": "Hello"}),
            json!({"op": "add", "scene": "s1", "kind": "shape", "shape": "circle", "fill": "#f00"}),
            json!({"op": "set", "target": "title.transform.position", "value": [960, 540]}),
            json!({"op": "key", "target": "title.transform.opacity", "at": 1, "from": 0, "v": 1, "ease": "glide"}),
            json!({"op": "key", "target": "title.transform.opacity", "at": 3, "v": 0}),
            json!({"op": "anchor", "target": "title.transform.opacity", "key": 1, "at": 1.5, "anchor": "peak", "dur": "8f"}),
            json!({"op": "group", "ids": ["title", "shape-1"], "id": "pair"}),
            json!({"op": "add_scene", "id": "s2", "start": 5}),
            json!({"op": "move", "id": "pair", "scene": "s2"}),
            json!({"op": "set", "target": "project.settings.duration", "value": 12}),
        ];
        let a = apply(&p0, &ops).unwrap();
        assert_eq!(a.created, vec!["title", "shape-1", "pair", "s2"]);
        assert_eq!(a.project.settings.duration, 12.0);
        assert_eq!(a.project.scenes[1].layers[0].id, "pair");
        let undone = apply(&a.project, &a.inverse).unwrap();
        assert_eq!(undone.project, p0);
        // Redo through the inverse of the inverse.
        let redone = apply(&undone.project, &undone.inverse).unwrap();
        assert_eq!(redone.project, a.project);
    }

    #[test]
    fn failing_batch_changes_nothing() {
        let p0 = base();
        let ops = vec![
            json!({"op": "add", "scene": "s1", "id": "a", "kind": "shape", "shape": "rect"}),
            json!({"op": "add", "scene": "s1", "id": "a", "kind": "shape", "shape": "rect"}),
        ];
        let err = apply(&p0, &ops).unwrap_err();
        assert!(err.0.contains("op 2") && err.0.contains("already exists"), "{err}");
        let err = apply(&p0, &[json!({"op": "set", "target": "nope.size", "value": 1})]).unwrap_err();
        assert!(err.0.contains("nope"));
        let err = apply(&p0, &[json!({"op": "add", "scene": "s1", "id": "b", "kind": "shape", "shape": "blob"})]).unwrap_err();
        assert!(err.0.contains("blob"), "{err}");
    }

    #[test]
    fn remove_and_ungroup_invert() {
        let p0 = base();
        let a = apply(&p0, &[
            json!({"op": "add", "scene": "s1", "id": "g", "kind": "comp", "layers": [
                {"id": "x", "kind": "shape", "shape": "rect"}, {"id": "y", "kind": "shape", "shape": "circle"}]}),
        ])
        .unwrap();
        let b = apply(&a.project, &[json!({"op": "remove", "id": "x"}), json!({"op": "ungroup", "id": "g"})]).unwrap();
        assert_eq!(b.project.scenes[0].layers.len(), 1);
        let back = apply(&b.project, &b.inverse).unwrap();
        assert_eq!(back.project, a.project);
    }
}

// ------------------------------------------------------------ time edits

fn time_base(project: &Project) -> crate::time::TimeBase {
    let mut base = crate::time::TimeBase::new(project.settings.fps);
    base.beat = project.markers.get("beat").and_then(|t| t.median_interval());
    base.bar = project.markers.get("bar").and_then(|t| t.median_interval());
    base
}

fn seconds_arg(op: &Value, key: &str, base: &crate::time::TimeBase) -> Result<f64> {
    let v = op.get(key).ok_or_else(|| format!("needs \"{key}\""))?;
    let d = match v {
        Value::Number(n) => crate::time::Duration::Seconds(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => crate::time::Duration::parse(s)?,
        _ => bail!("\"{key}\" is a time in seconds or \"12f\""),
    };
    d.seconds(base)
}

/// A literal time in the document: seconds, or "48f"/"2s" (frames are
/// kept as frames when the shift is whole frames). References (markers,
/// `#id`, beats) are None: they resolve on their own.
fn literal_time(v: &Value, base: &crate::time::TimeBase) -> Option<(f64, bool)> {
    match v {
        Value::Number(n) => Some((n.as_f64()?, false)),
        Value::String(s) => match crate::timeref::parse(s).ok()? {
            crate::timeref::Parsed { base: crate::timeref::Base::Seconds(t), offsets } if offsets.is_empty() => Some((t, false)),
            crate::timeref::Parsed { base: crate::timeref::Base::Duration(d), offsets } if offsets.is_empty() => match d {
                crate::time::Duration::Frames(_) => Some((d.seconds(base).ok()?, true)),
                crate::time::Duration::Seconds(t) => Some((t, false)),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

fn write_time(v: &mut Value, t: f64, frames: bool, base: &crate::time::TimeBase) {
    let f = t / base.fps.frame();
    *v = if frames && (f - f.round()).abs() < 1e-6 {
        json!(format!("{}f", f.round()))
    } else {
        json!((t * 1e6).round() / 1e6)
    };
}

/// Moves the time in `v` by `d` when `keep(t)`; returns whether it moved.
fn move_time(v: &mut Value, d: f64, base: &crate::time::TimeBase, keep: &dyn Fn(f64) -> bool) -> bool {
    match literal_time(v, base) {
        Some((t, frames)) if keep(t) => {
            write_time(v, t + d, frames, base);
            true
        }
        _ => false,
    }
}

/// Seconds a `time.offset` / `pre_roll` duration stands for (0 when absent).
fn duration_secs(v: Option<&Value>, base: &crate::time::TimeBase) -> Result<f64> {
    let Some(v) = v else { return Ok(0.0) };
    let d = match v {
        Value::Number(n) => crate::time::Duration::Seconds(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => crate::time::Duration::parse(s)?,
        Value::Null => return Ok(0.0),
        _ => bail!("bad duration {v}"),
    };
    d.seconds(base)
}

/// Every literal time in a layer list. `k`: local time = timeline + k (a
/// scene's pre-roll, minus the offsets of enclosing comps). `when(t)` is
/// asked with the timeline time.
fn layers_times(layers: &mut Value, k: f64, d: f64, base: &crate::time::TimeBase, when: &dyn Fn(f64) -> bool, n: &mut usize) -> Result<()> {
    let Some(list) = layers.as_array_mut() else { return Ok(()) };
    for layer in list {
        layer_times(layer, k, d, base, when, n)?;
    }
    Ok(())
}

fn layer_times(layer: &mut Value, k: f64, d: f64, base: &crate::time::TimeBase, when: &dyn Fn(f64) -> bool, n: &mut usize) -> Result<()> {
    let at_local = |t: f64| when(t - k);
    if let Some(time) = layer.get_mut("time").and_then(Value::as_object_mut) {
        for f in ["start", "end"] {
            if let Some(v) = time.get_mut(f) {
                *n += move_time(v, d, base, &at_local) as usize;
            }
        }
    }
    let offset = duration_secs(layer.get("time").and_then(|t| t.get("offset")), base)?;
    if let Some(obj) = layer.as_object_mut() {
        for (key, v) in obj.iter_mut() {
            match key.as_str() {
                "layers" => layers_times(v, k - offset, d, base, when, n)?,
                "time" | "follow" | "id" => {}
                _ => any_times(v, k, d, base, when, n),
            }
        }
    }
    Ok(())
}

/// `at` fields anywhere below (keys, animators, effects).
fn any_times(v: &mut Value, k: f64, d: f64, base: &crate::time::TimeBase, when: &dyn Fn(f64) -> bool, n: &mut usize) {
    match v {
        Value::Object(o) => {
            for (key, child) in o.iter_mut() {
                if key == "at" || key == "t" {
                    *n += move_time(child, d, base, &|t| when(t - k)) as usize;
                } else if key != "follow" && key != "layers" {
                    any_times(child, k, d, base, when, n);
                }
            }
        }
        Value::Array(a) => {
            for child in a {
                any_times(child, k, d, base, when, n);
            }
        }
        _ => {}
    }
}

fn scene_times(scene: &mut Value, d: f64, base: &crate::time::TimeBase, when: &dyn Fn(f64) -> bool, n: &mut usize) -> Result<()> {
    for f in ["start", "end"] {
        if let Some(v) = scene.get_mut(f) {
            *n += move_time(v, d, base, when) as usize;
        }
    }
    let pre_roll = duration_secs(scene.get("pre_roll"), base)?;
    if let Some(cam) = scene.get_mut("camera") {
        any_times(cam, pre_roll, d, base, when, n);
    }
    if let Some(layers) = scene.get_mut("layers") {
        layers_times(layers, pre_roll, d, base, when, n)?;
    }
    Ok(())
}

fn sounds_times(doc: &mut Value, d: f64, base: &crate::time::TimeBase, when: &dyn Fn(f64) -> bool, n: &mut usize) {
    let Some(audio) = doc.get_mut("audio").and_then(Value::as_object_mut) else { return };
    for (key, v) in audio.iter_mut() {
        let clips: Vec<&mut Value> = match (key.as_str(), v) {
            ("music" | "vo", v @ Value::Object(_)) => vec![v],
            (_, Value::Array(list)) => list.iter_mut().collect(),
            _ => Vec::new(),
        };
        for clip in clips {
            if let Some(at) = clip.get_mut("at") {
                *n += move_time(at, d, base, when) as usize;
            }
        }
    }
}

/// Inserts `duration` of time at `at`: everything timed at or after `at`
/// (by a literal time: seconds or frames) moves later. Scenes, layers,
/// keys, animators, effects, sounds, the project length, marker tracks
/// and the narration ripple; references resolve on their own.
fn op_insert_time(doc: &mut Value, op: &Value, base: &crate::time::TimeBase, ripples: &mut Vec<(f64, f64)>) -> OpResult {
    let at = seconds_arg(op, "at", base)?;
    let d = seconds_arg(op, "duration", base)?;
    // Negative durations are the undo of an insert (exact there); asked
    // for directly they would fold later times onto earlier ones.
    if d <= 0.0 && op.get("undo").and_then(Value::as_bool) != Some(true) {
        bail!("insert_time needs a positive \"duration\"");
    }
    let when = |t: f64| t >= at - 1e-9;
    let mut n = 0;
    if let Some(scenes) = doc.get_mut("scenes").and_then(Value::as_array_mut) {
        for scene in scenes {
            scene_times(scene, d, base, &when, &mut n)?;
        }
    }
    sounds_times(doc, d, base, &when, &mut n);
    if let Some(dur) = doc.pointer_mut("/settings/duration") {
        if let Some(v) = dur.as_f64() {
            if v > at {
                *dur = json!(((v + d).max(at) * 1e6).round() / 1e6);
            }
        }
    }
    ripples.push((at, d));
    let inverse = vec![json!({"op": "insert_time", "at": at + d, "duration": -d, "undo": true})];
    Ok((inverse, Vec::new(), format!("inserted {d:.3} s at {at:.3} s ({n} times moved)")))
}

/// Moves elements in time by `by`: scenes (with their layers), layers
/// (with their keys and children) and sounds, by id.
fn op_shift(doc: &mut Value, op: &Value, base: &crate::time::TimeBase) -> OpResult {
    let by = seconds_arg(op, "by", base)?;
    let ids: Vec<String> = match op.get("ids") {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => bail!("shift needs \"ids\": a list of scene, layer or sound ids"),
    };
    let all = |_: f64| true;
    let mut n = 0;
    for id in &ids {
        let (ptr, _, _) = find(doc, id).ok_or_else(|| format!("no element '{id}'"))?;
        let el = doc.pointer_mut(&ptr).ok_or_else(|| format!("no element '{id}'"))?;
        if ptr.starts_with("/audio") {
            if let Some(at) = el.get_mut("at") {
                n += move_time(at, by, base, &all) as usize;
            }
        } else if ptr.matches("/layers/").count() == 0 && ptr.starts_with("/scenes/") {
            scene_times(el, by, base, &all, &mut n)?;
        } else {
            layer_times(el, 0.0, by, base, &all, &mut n)?;
        }
    }
    let inverse = vec![json!({"op": "shift", "ids": ids, "by": -by})];
    Ok((inverse, ids.clone(), format!("shifted {} by {by:.3} s ({n} times moved)", ids.join(", "))))
}

/// Marker tracks and the narration follow inserted time. Music analysis
/// tracks move only with the music; the narration is re-derived.
fn ripple_markers(next: &mut Project, before: &Project, at: f64, d: f64) {
    const MUSIC: [&str; 9] = ["beat", "bar", "half", "sixteenth", "triplet", "hit", "stop", "drop", "music.start"];
    let music_moved = before.audio.music.as_ref().is_some_and(|m| matches!(m.at, crate::timeref::TimeRef::Seconds(t) if t >= at - 1e-9));
    let narration_tracks = ["line", "sentence", "word", "cue"];
    let has_narration = next.narration.is_some();
    for track in next.markers.tracks.values_mut() {
        let music = MUSIC.contains(&track.name.as_str()) || track.name.starts_with("onset") || track.name == "music.end";
        if music && !music_moved || has_narration && narration_tracks.contains(&track.name.as_str()) {
            continue;
        }
        for m in &mut track.markers {
            if music || m.t >= at - 1e-9 {
                m.t += d;
                if let Some(e) = &mut m.end {
                    *e += d;
                }
            } else if let Some(e) = &mut m.end {
                // A marker spanning the insert grows.
                if *e >= at - 1e-9 {
                    *e += d;
                }
            }
        }
    }
    if let Some(narration) = &mut next.narration {
        for clip in &mut narration.clips {
            if clip.at >= at - 1e-9 {
                clip.at += d;
            }
        }
        for t in narration.tracks() {
            next.markers.insert(t);
        }
    }

}

#[cfg(test)]
mod time_tests {
    use super::*;
    use crate::time::Fps;

    fn base() -> Project {
        let mut p = Project::new(1920, 1080, Fps::whole(30), 10.0);
        p.scenes.push(crate::document::Scene::new("s1"));
        p
    }

    #[test]
    fn insert_time_ripples_literal_times_and_undoes_exactly() {
        let mut p = Project::new(1920, 1080, Fps::whole(30), 20.0);
        p.markers.insert(crate::markers::MarkerTrack::from_times("beat", (0..40).map(|i| i as f64 * 0.5)));
        p.markers.insert(crate::markers::MarkerTrack { name: "user".into(), source: Some("user".into()), markers: vec![
            crate::markers::Marker { t: 1.0, ..Default::default() }, crate::markers::Marker { t: 6.0, ..Default::default() }] });
        let doc = json!({"scenes": [
            {"id": "a", "start": 0, "layers": [
                {"id": "early", "kind": "shape", "shape": "rect", "time": {"start": 0.5, "end": 3.0},
                 "transform": {"opacity": {"keys": [{"at": 0.5, "v": 0}, {"at": "60f", "v": 1}, {"at": "beat:3", "v": 0.5}]}}}]},
            {"id": "b", "start": 4.0, "pre_roll": 0.5, "layers": [
                {"id": "late", "kind": "shape", "shape": "rect", "time": {"start": "#early.end + 0.25"},
                 "transform": {"position": {"keys": [{"at": 4.6, "v": [0, 0]}, {"at": 6.0, "v": [100, 0]}]}}},
                {"id": "box", "kind": "comp", "time": {"start": 5.0, "offset": 3.0}, "layers": [
                    {"id": "inner", "kind": "shape", "shape": "circle", "transform": {"scale": {"keys": [{"at": 1.0, "v": [1, 1]}, {"at": 3.0, "v": [2, 2]}]}}}]},
                {"id": "path", "kind": "shape", "shape": "path", "d": "M 0 0 L 100 0"},
                {"id": "rider", "kind": "shape", "shape": "circle", "follow": {"layer": "path", "at": 0.5}}]}]});
        let mut p2: Project = serde_json::from_value(json!({"settings": serde_json::to_value(&p.settings).unwrap(), "scenes": doc["scenes"]})).unwrap();
        p2.markers = p.markers.clone();
        let p = apply(&p2, &[
            json!({"op": "add_sound", "path": "a.wav", "id": "boom", "at": 4.5}),
            json!({"op": "add_sound", "path": "b.wav", "id": "tick", "at": 1.0}),
            json!({"op": "add_sound", "kind": "music", "path": "m.wav"}),
        ]).unwrap().project;
        let a = apply(&p, &[json!({"op": "insert_time", "at": 2.0, "duration": 5})]).unwrap();
        let q = &a.project;
        let v = serde_json::to_value(q).unwrap();
        let early = &v["scenes"][0]["layers"][0];
        assert_eq!(early["time"]["start"], 0.5, "before the insert: stays");
        assert_eq!(early["time"]["end"], 8.0, "a layer spanning the insert grows");
        assert_eq!(early["transform"]["opacity"]["keys"][1]["at"], "210f", "frames stay frames: 60f + 150f");
        assert_eq!(early["transform"]["opacity"]["keys"][2]["at"], "beat:3", "references are left alone");
        assert_eq!(v["scenes"][1]["start"], 9.0);
        let late = &v["scenes"][1]["layers"][0];
        // Scene b's layers run 0.5 s ahead (pre-roll): 4.6 is timeline 4.1.
        assert_eq!(late["transform"]["position"]["keys"][0]["at"], 9.6);
        let inner = &v["scenes"][1]["layers"][1]["layers"][0];
        // Inside the comp (offset 3, pre-roll 0.5): local 1.0 is timeline 3.5.
        assert_eq!(inner["transform"]["scale"]["keys"][0]["at"], 6.0);
        assert_eq!(v["scenes"][1]["layers"][1]["time"]["start"], 10.0);
        assert_eq!(v["scenes"][1]["layers"][3]["follow"]["at"], 0.5, "a follow position is not a time");
        assert_eq!(q.audio.sfx[0].at, crate::timeref::TimeRef::Seconds(9.5));
        assert_eq!(q.audio.sfx[1].at, crate::timeref::TimeRef::Seconds(1.0));
        assert_eq!(q.settings.duration, 25.0);
        // The music starts at 0 and does not move, nor does its beat grid;
        // the user's marker after the insert does.
        assert_eq!(q.markers.get("beat").unwrap().markers[3].t, 1.5);
        assert_eq!(q.markers.get("user").unwrap().markers.iter().map(|m| m.t).collect::<Vec<_>>(), vec![1.0, 11.0]);
        assert!(a.markers_changed);
        // Undo is exact.
        let undone = apply(q, &a.inverse).unwrap().project;
        assert_eq!(serde_json::to_value(&undone).unwrap(), serde_json::to_value(&p).unwrap());
        assert_eq!(undone.markers, p.markers);
        assert!(apply(&p, &[json!({"op": "insert_time", "at": 2.0, "duration": -1})]).is_err());
    }

    #[test]
    fn shift_moves_elements_with_everything_in_them() {
        let p = base();
        let p = apply(&p, &[
            json!({"op": "add", "scene": "s1", "id": "card", "kind": "shape", "shape": "rect", "time": {"start": 2.0, "end": 4.0},
                   "transform": {"opacity": {"keys": [{"at": 2.0, "v": 0}, {"at": 2.5, "v": 1}]}}}),
            json!({"op": "add_sound", "path": "a.wav", "id": "pop", "at": 2.0}),
        ]).unwrap().project;
        let a = apply(&p, &[json!({"op": "shift", "ids": ["card", "pop"], "by": "15f"})]).unwrap();
        let v = serde_json::to_value(&a.project).unwrap();
        let card = &v["scenes"][0]["layers"][0];
        assert_eq!(card["time"]["start"], 2.5);
        assert_eq!(card["transform"]["opacity"]["keys"][1]["at"], 3.0);
        assert_eq!(a.project.audio.sfx[0].at, crate::timeref::TimeRef::Seconds(2.5));
        let undone = apply(&a.project, &a.inverse).unwrap().project;
        assert_eq!(serde_json::to_value(&undone).unwrap(), serde_json::to_value(&p).unwrap());
    }
}
