//! MCP server over stdio on the official Rust SDK (`rmcp`). Tool calls run
//! on blocking threads (rendering and analysis are CPU/GPU work) against
//! one shared [`Toolbox`].

use crate::tools::{png_base64, tool_defs, Toolbox};
use lahza_motion_core::catalog;
use rmcp::model::*;
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const INSTRUCTIONS: &str = "Lahza motion engine. Loop: project_create (or project_open) → audio_analyze (music) / narration_import (a narration manifest: one clip per line with the voice provider's word or character timings) → markers_query → edit_apply (scenes, layers, keys anchored to markers) → check (sync drift ≤ ±1 f, no errors) → sync_check (flagged: true: events that miss their marker or hit nothing) → snapshot (contact sheet at sync points) → fix → review (the whole film before calling it done: scenes, every cut, phone width; notes as P0 t=4.2s · where · problem → exact change) → render. Before building each shot, write its breakdown (prompt `breakdown`, or lahza://guide/breakdown): every element's kind, structure, material, lighting, motion and how it will be built. Build geometry from engine shapes, 3D meshes or Blender; measurements of a reference set parameters only, never traced shapes. Narrated pieces time everything with narration references — cue:<name> (from [name] in the script), word:<word> (word:'81' for numbers), line:<id>, sentence#n — never literal seconds, so re-narrating moves every scene. A film is not a web page (lahza://guide, 'A film, not a page'): one idea per shot shown by the picture; besides captions at most ~12 words on screen and no text under 2.6% of the frame's short side; no header, footer, frame counter or progress bar; no text repeating the caption; cuts with a cause, never a crossfade between two worlds or a cut onto an empty frame; the guide's banned defaults stay out unless the brand uses them. Declare each layer's role (background, subject, foreground, caption, decoration) and the facing of anything that travels, so check proves depth order and direction; keep a rule broken on purpose with allow, never by ignoring check. Read lahza://guide first, lahza://schema for the format and lahza://catalog for presets, layer kinds and time references.";

#[derive(Clone)]
pub struct Server {
    toolbox: Arc<Mutex<Toolbox>>,
}

impl Server {
    pub fn new(toolbox: Toolbox) -> Self {
        Self { toolbox: Arc::new(Mutex::new(toolbox)) }
    }
}

struct Doc {
    uri: &'static str,
    name: &'static str,
    description: &'static str,
    mime: &'static str,
}

const DOCS: &[Doc] = &[
    Doc { uri: "lahza://guide", name: "guide", description: "The production loop, timing rules and review checklist", mime: "text/markdown" },
    Doc { uri: "lahza://guide/breakdown", name: "breakdown", description: "The per-shot breakdown template (kind, structure, material, lighting, motion, build) with an example; write one before building a shot", mime: "text/markdown" },
    Doc { uri: "lahza://schema", name: "schema", description: "Project folder and JSON format", mime: "text/markdown" },
    Doc { uri: "lahza://catalog", name: "catalog", description: "Presets with measured numbers, raw curves, anchors, time references, layer kinds, ops", mime: "application/json" },
    Doc { uri: "lahza://styles", name: "styles", description: "Style packs with palettes, defaults and rules", mime: "application/json" },
];

fn read_doc(uri: &str) -> Option<(String, &'static str)> {
    Some(match uri {
        "lahza://guide" => (catalog::GUIDE.to_string(), "text/markdown"),
        "lahza://guide/breakdown" => (breakdown_section().to_string(), "text/markdown"),
        "lahza://schema" => (catalog::SCHEMA.to_string(), "text/markdown"),
        "lahza://catalog" => {
            let mut c = catalog::catalog_json();
            c["sounds"] = serde_json::Value::Array(
                lahza_motion_audio::sfx::SOUNDS.iter().map(|(name, seconds, use_for)| serde_json::json!({"name": name, "seconds": seconds, "use_for": use_for})).collect(),
            );
            (serde_json::to_string_pretty(&c).ok()?, "application/json")
        }
        "lahza://styles" => (serde_json::to_string_pretty(&catalog::styles_json()).ok()?, "application/json"),
        other => {
            let name = other.strip_prefix("lahza://styles/")?;
            let pack = catalog::styles_json().as_array()?.iter().find(|p| p["name"] == name)?.clone();
            (serde_json::to_string_pretty(&pack).ok()?, "application/json")
        }
    })
}

/// The guide's "Breakdown" section (its heading to the next `## `).
pub fn breakdown_section() -> &'static str {
    let guide = catalog::GUIDE;
    let start = guide.find("## Breakdown").unwrap_or(0);
    let end = guide[start + 2..].find("\n## ").map(|i| start + 2 + i).unwrap_or(guide.len());
    guide[start..end].trim()
}

/// The `breakdown` prompt: the template for one shot.
pub fn breakdown_prompt(shot: &str) -> String {
    format!(
        "Write the breakdown for this shot before building anything for it.\n\nShot: {shot}\n\n\
        Fill one block per element (kind, structure, material, lighting, motion, build) as in the template and example below; a few words each, \
        concrete numbers where you have them (counts, colours, key direction, sync anchors). If there is a reference, measure it with track / fit_curve \
        for timing, positions, colours and sizes, but choose the build from what the element is (a lit 3D object is a 3D mesh or Blender, not traced \
        shapes). Then build the shot as the breakdown says and check it with check and compare.\n\n{}",
        breakdown_section()
    )
}

struct PromptDef {
    name: &'static str,
    description: &'static str,
    style: &'static str,
    shape: &'static str,
}

const PROMPTS: &[PromptDef] = &[
    PromptDef { name: "make_promo", description: "Beat-synced kinetic-type promo", style: "promo.kinetic", shape: "One scene per 1–2 bars, cut on bars with a −1 f lead. Words enter per glyph ('enter', 1 f stagger) anchored to beats; logo pops ('pop') on the largest hit; every settled element drifts. Sync mode: grid." },
    PromptDef { name: "make_explainer", description: "Narrated evidence explainer", style: "explainer.evidence", shape: "One row per spoken line; visuals appear on the word that names them. Narrate first: write the script with [cue] markup at the moments visuals hit, make the voice with an adapter (tools/narration), narration_import the manifest. Then time every scene, layer, key and sound with cue:/word:/line: references (scene start \"line:s03\", a reveal at \"cue:strike\"), never literal seconds. Elements step at 12 fps, the camera pushes smoothly. No still over 3 s. Sync mode: words." },
    PromptDef { name: "make_product_demo", description: "Product/UI demo", style: "product.ui", shape: "Cards and UI pieces snap in ('snap', 'glide') with 1–3 f staggers; the click is the transition; nothing stops dead. Sync mode: hits or free." },
    PromptDef { name: "make_ident", description: "Short flat ident", style: "ident.flat", shape: "One scene per bar; shapes land with 'land' springs on downbeats; transitions peak on the beat ('gesture', anchor peak). Sync mode: grid." },
];

fn prompt_text(p: &PromptDef, brief: &str) -> String {
    format!(
        "Make a video with the Lahza motion tools.\n\nBrief: {brief}\n\nStyle pack: {style} (read lahza://styles/{style}). {shape}\n\n\
        First write a visual script as a table — one row per line or musical section: words | visual | sync anchor (e.g. bar:5, word:canal, hit:2) | style notes — and get it approved. \
        Then follow lahza://guide: create the project with the style, analyse the music, query markers, write each shot's breakdown (lahza://guide/breakdown) before building it, build scene by scene with edit_apply, run check after each scene \
        (no errors, drift ≤ ±1 frame), review a snapshot contact sheet at its sync points, and render a draft before the final.",
        style = p.style,
        shape = p.shape
    )
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().enable_resources().enable_prompts().build())
            .with_server_info(Implementation::new("lahza-motion", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(&self, _: Option<PaginatedRequestParams>, _: RequestContext<RoleServer>) -> Result<ListToolsResult, McpError> {
        let tools = tool_defs()
            .into_iter()
            .map(|d| {
                let schema = match d.schema {
                    Value::Object(m) => Arc::new(m),
                    _ => Arc::new(Default::default()),
                };
                let mut tool = Tool::new(d.name, d.description, schema);
                let mut ann = ToolAnnotations::default();
                ann.read_only_hint = Some(d.read_only);
                tool.annotations = Some(ann);
                tool
            })
            .collect();
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(&self, request: CallToolRequestParams, _: RequestContext<RoleServer>) -> Result<CallToolResponse, McpError> {
        let name = request.name.to_string();
        if !tool_defs().iter().any(|d| d.name == name) {
            return Err(McpError::invalid_params(format!("unknown tool '{name}'"), None));
        }
        let args = request.arguments.map(Value::Object).unwrap_or(json!({}));
        let toolbox = self.toolbox.clone();
        let result = tokio::task::spawn_blocking(move || {
            let mut tb = toolbox.lock().unwrap_or_else(|e| e.into_inner());
            tb.call(&name, args)
        })
        .await
        .map_err(|e| McpError::internal_error(format!("tool panicked: {e}"), None))?;
        Ok(match result {
            Ok(out) => {
                let mut content = vec![ContentBlock::text(serde_json::to_string_pretty(&out.json).unwrap_or_default())];
                content.extend(out.images.iter().map(|png| ContentBlock::image(png_base64(png), "image/png")));
                CallToolResult::success(content)
            }
            Err(e) => CallToolResult::error(vec![ContentBlock::text(e)]),
        }
        .into())
    }

    async fn list_resources(&self, _: Option<PaginatedRequestParams>, _: RequestContext<RoleServer>) -> Result<ListResourcesResult, McpError> {
        let mut list: Vec<Resource> = DOCS.iter().map(|d| Resource::new(d.uri, d.name).with_description(d.description).with_mime_type(d.mime)).collect();
        for pack in catalog::STYLE_PACKS {
            list.push(Resource::new(format!("lahza://styles/{}", pack.name), format!("style {}", pack.name)).with_description(pack.description).with_mime_type("application/json"));
        }
        Ok(ListResourcesResult::with_all_items(list))
    }

    async fn read_resource(&self, request: ReadResourceRequestParams, _: RequestContext<RoleServer>) -> Result<ReadResourceResponse, McpError> {
        let (text, mime) = read_doc(&request.uri).ok_or_else(|| McpError::resource_not_found(format!("no resource {}", request.uri), None))?;
        Ok(ReadResourceResult::new(vec![ResourceContents::text(text, request.uri).with_mime_type(mime)]).into())
    }

    async fn list_prompts(&self, _: Option<PaginatedRequestParams>, _: RequestContext<RoleServer>) -> Result<ListPromptsResult, McpError> {
        let mut prompts: Vec<Prompt> = PROMPTS
            .iter()
            .map(|p| Prompt::new(p.name, Some(p.description), Some(vec![PromptArgument::new("brief").with_description("what the video is about, audience, length, assets").with_required(true)])))
            .collect();
        prompts.push(Prompt::new(
            "breakdown",
            Some("Break a shot down before building it: each element's kind, structure, material, lighting, motion and build method"),
            Some(vec![PromptArgument::new("shot").with_description("the shot: its time range, sync points and what it shows (or the reference's moment)").with_required(true)]),
        ));
        Ok(ListPromptsResult::with_all_items(prompts))
    }

    async fn get_prompt(&self, request: GetPromptRequestParams, _: RequestContext<RoleServer>) -> Result<GetPromptResponse, McpError> {
        if request.name == "breakdown" {
            let shot = request.arguments.as_ref().and_then(|a| a.get("shot")).and_then(Value::as_str).unwrap_or("(ask which shot)");
            return Ok(GetPromptResult::new(vec![PromptMessage::new_text(Role::User, breakdown_prompt(shot))]).with_description("Per-shot breakdown before building").into());
        }
        let p = PROMPTS.iter().find(|p| p.name == request.name).ok_or_else(|| McpError::invalid_params(format!("no prompt {}", request.name), None))?;
        let brief = request.arguments.as_ref().and_then(|a| a.get("brief")).and_then(Value::as_str).unwrap_or("(ask the person for the brief)");
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(Role::User, prompt_text(p, brief))]).with_description(p.description).into())
    }
}

/// Serves MCP on stdin/stdout until the client disconnects.
pub fn serve(toolbox: Toolbox) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|e| e.to_string())?;
    runtime.block_on(async move {
        let service = Server::new(toolbox).serve(rmcp::transport::stdio()).await.map_err(|e| e.to_string())?;
        service.waiting().await.map_err(|e| e.to_string())?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breakdown_is_a_guide_section_a_resource_and_a_prompt() {
        let section = breakdown_section();
        assert!(section.starts_with("## Breakdown"), "{section}");
        assert!(!section.contains("\n## "), "only one section: {section}");
        for field in ["kind", "structure", "material", "lighting", "motion", "build", "torus split into 4"] {
            assert!(section.contains(field), "missing {field}");
        }
        let (text, mime) = read_doc("lahza://guide/breakdown").unwrap();
        assert_eq!((text.as_str(), mime), (section, "text/markdown"));
        assert!(DOCS.iter().any(|d| d.uri == "lahza://guide/breakdown"));
        let prompt = breakdown_prompt("shot 3, 4.2–6.0 s");
        assert!(prompt.contains("shot 3, 4.2–6.0 s") && prompt.contains(section));
        assert!(INSTRUCTIONS.contains("breakdown"));
    }
}
