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
//! ```

use crate::document::{valid_id, Project};
use crate::{bail, Result};
use serde_json::{json, Map, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct Applied {
    pub project: Project,
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
    let mut inverse = Vec::new();
    let mut created = Vec::new();
    let mut changed = Vec::new();
    let mut summary = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        let name = op.get("op").and_then(Value::as_str).unwrap_or("");
        let result = match name {
            "add" => op_add(&mut doc, op, &mut created),
            "add_scene" => op_add_scene(&mut doc, op, &mut created),
            "set" => op_set(&mut doc, op),
            "remove" => op_remove(&mut doc, op),
            "move" => op_move(&mut doc, op),
            "key" => op_key(&mut doc, op),
            "anchor" => op_anchor(&mut doc, op),
            "group" => op_group(&mut doc, op, &mut created),
            "ungroup" => op_ungroup(&mut doc, op),
            "" => Err("op needs an \"op\" field".into()),
            other => Err(format!(
                "unknown op '{other}': use add, add_scene, set, remove, move, key, anchor, group, ungroup"
            )
            .into()),
        };
        let (inv, ids, line) = result.map_err(|e| format!("op {} ({name}): {e}", i + 1))?;
        inverse.splice(0..0, inv);
        changed.extend(ids);
        summary.push(line);
    }
    let mut next: Project = serde_json::from_value(doc).map_err(|e| format!("result is not a valid project: {e}"))?;
    next.markers = project.markers.clone();
    changed.sort();
    changed.dedup();
    Ok(Applied { project: next, inverse, created, changed, summary })
}

type OpResult = Result<(Vec<Value>, Vec<String>, String)>;

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
    None
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
    let (_, container, index) = find(doc, id).ok_or_else(|| format!("no layer or scene with id '{id}'"))?;
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
        "transform.rotation" | "camera.rotation" | "radius" | "tracking" => json!(0),
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
