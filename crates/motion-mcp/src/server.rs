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

const INSTRUCTIONS: &str = "Lahza motion engine. Loop: project_create (or project_open) → audio_analyze (music) / markers_import (narration words) → markers_query → edit_apply (scenes, layers, keys anchored to markers) → check (sync drift ≤ ±1 f, no errors) → snapshot (contact sheet at sync points) → fix → render. Read lahza://guide first, lahza://schema for the format and lahza://catalog for presets, layer kinds and time references.";

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
    Doc { uri: "lahza://schema", name: "schema", description: "Project folder and JSON format", mime: "text/markdown" },
    Doc { uri: "lahza://catalog", name: "catalog", description: "Presets with measured numbers, raw curves, anchors, time references, layer kinds, ops", mime: "application/json" },
    Doc { uri: "lahza://styles", name: "styles", description: "Style packs with palettes, defaults and rules", mime: "application/json" },
];

fn read_doc(uri: &str) -> Option<(String, &'static str)> {
    Some(match uri {
        "lahza://guide" => (catalog::GUIDE.to_string(), "text/markdown"),
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

struct PromptDef {
    name: &'static str,
    description: &'static str,
    style: &'static str,
    shape: &'static str,
}

const PROMPTS: &[PromptDef] = &[
    PromptDef { name: "make_promo", description: "Beat-synced kinetic-type promo", style: "promo.kinetic", shape: "One scene per 1–2 bars, cut on bars with a −1 f lead. Words enter per glyph ('enter', 1 f stagger) anchored to beats; logo pops ('pop') on the largest hit; every settled element drifts. Sync mode: grid." },
    PromptDef { name: "make_explainer", description: "Narrated evidence explainer", style: "explainer.evidence", shape: "One row per spoken line; visuals appear on the word that names them (word markers). Elements step at 12 fps, the camera pushes smoothly. No still over 3 s. Sync mode: words." },
    PromptDef { name: "make_product_demo", description: "Product/UI demo", style: "product.ui", shape: "Cards and UI pieces snap in ('snap', 'glide') with 1–3 f staggers; the click is the transition; nothing stops dead. Sync mode: hits or free." },
    PromptDef { name: "make_ident", description: "Short flat ident", style: "ident.flat", shape: "One scene per bar; shapes land with 'land' springs on downbeats; transitions peak on the beat ('gesture', anchor peak). Sync mode: grid." },
];

fn prompt_text(p: &PromptDef, brief: &str) -> String {
    format!(
        "Make a video with the Lahza motion tools.\n\nBrief: {brief}\n\nStyle pack: {style} (read lahza://styles/{style}). {shape}\n\n\
        First write a visual script as a table — one row per line or musical section: words | visual | sync anchor (e.g. bar:5, word:canal, hit:2) | style notes — and get it approved. \
        Then follow lahza://guide: create the project with the style, analyse the music, query markers, build scene by scene with edit_apply, run check after each scene \
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
        let prompts = PROMPTS
            .iter()
            .map(|p| Prompt::new(p.name, Some(p.description), Some(vec![PromptArgument::new("brief").with_description("what the video is about, audience, length, assets").with_required(true)])))
            .collect();
        Ok(ListPromptsResult::with_all_items(prompts))
    }

    async fn get_prompt(&self, request: GetPromptRequestParams, _: RequestContext<RoleServer>) -> Result<GetPromptResponse, McpError> {
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
