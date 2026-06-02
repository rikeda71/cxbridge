use std::path::Path;

use anyhow::Context;
use serde_json::{Map, Value};

use crate::core::ir::{
    new_node, DegradeInfo, DiagLevel, Diagnostic, DroppedInfo, IRField, IRNode, Kind, Loss, Tool,
};
use crate::core::mappings::{
    applies_direction, index_by_claude_field, index_by_codex_field, DomainMap,
};
use crate::core::transforms::{apply_transforms, ConvDir, TransformCtx};
use crate::handlers::{EmitFile, EmitPlan, Handler, LowerOpts};

/// Handler for the MCP domain.
pub struct McpHandler {
    pub map: DomainMap,
}

impl Handler for McpHandler {
    fn kind(&self) -> Kind {
        Kind::Mcp
    }

    fn detect(&self, path: &Path) -> bool {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        name == ".mcp.json"
    }

    fn parse(&self, path: &Path) -> anyhow::Result<Value> {
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if file_name == "config.toml" {
            // config.toml is parsed as TOML
            parse_toml_mcp_config(path)
        } else {
            // .mcp.json is parsed as JSON
            crate::core::serialize::json::parse_json_file(path)
        }
    }

    fn lift(&self, parsed: &Value, dir: ConvDir) -> anyhow::Result<IRNode> {
        let source_tool = match dir {
            ConvDir::C2x => Tool::Claude,
            ConvDir::X2c => Tool::Codex,
        };
        let source_path = parsed["path"].as_str().unwrap_or("").to_string();
        let mut node = new_node(Kind::Mcp, source_tool, &source_path);

        match dir {
            ConvDir::C2x => self.lift_c2x(parsed, &mut node)?,
            ConvDir::X2c => self.lift_x2c(parsed, &mut node)?,
        }

        Ok(node)
    }

    fn lower(&self, ir: &IRNode, dir: ConvDir, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        match dir {
            ConvDir::C2x => self.lower_c2x(ir, opts),
            ConvDir::X2c => self.lower_x2c(ir, opts),
        }
    }
}

impl McpHandler {
    /// Lift Claude .mcp.json → IR (c2x direction).
    fn lift_c2x(&self, parsed: &Value, node: &mut IRNode) -> anyhow::Result<()> {
        let frontmatter = parsed["frontmatter"]
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("Expected object at top level of .mcp.json"))?;

        // .mcp.json top level: { "mcpServers": { "<name>": { ... } } }
        let servers = match frontmatter.get("mcpServers").and_then(|v| v.as_object()) {
            Some(s) => s,
            None => return Ok(()),
        };

        let idx = index_by_claude_field(&self.map);

        // Record the mcp.format entry
        if let Some(entry) = idx.get("mcpServers") {
            if applies_direction(entry, ConvDir::C2x) {
                let ctx = TransformCtx {
                    direction: ConvDir::C2x,
                    args: None,
                    field: entry,
                };
                let (v, applied) = apply_transforms(
                    &Value::Object(servers.clone()),
                    entry.transform.as_deref(),
                    &ctx,
                );
                node.fields.insert(
                    "mcp.format".to_string(),
                    IRField {
                        id: "mcp.format".to_string(),
                        value: v,
                        loss: Loss::Lossless,
                        transforms_applied: applied,
                        degrade: None,
                        warning: None,
                        dropped: None,
                    },
                );
            }
        }

        // Process each server as a child IRNode
        for (server_name, server_cfg) in servers {
            let child = self.lift_server_c2x(server_name, server_cfg, &idx)?;
            node.children.push(child);
        }

        Ok(())
    }

    fn lift_server_c2x(
        &self,
        server_name: &str,
        server_cfg: &Value,
        idx: &std::collections::HashMap<String, &crate::core::mappings::MapEntry>,
    ) -> anyhow::Result<IRNode> {
        let mut child = new_node(Kind::Mcp, Tool::Claude, server_name);

        let cfg = match server_cfg.as_object() {
            Some(o) => o,
            None => return Ok(child),
        };

        for (key, value) in cfg {
            match key.as_str() {
                // Transport determination: the "type" field requires special handling
                "type" => {
                    let transport_type = value.as_str().unwrap_or("");
                    match transport_type {
                        "sse" => {
                            // Dropped under its own mapping id (mcp.transport_sse), not the
                            // surviving-transport id. Recorded via IRField.dropped; build_report
                            // surfaces it (no separate diagnostic, matching the dropped-field pattern).
                            child.fields.insert(
                                "mcp.transport_sse".to_string(),
                                IRField {
                                    id: "mcp.transport_sse".to_string(),
                                    value: value.clone(),
                                    loss: Loss::Dropped,
                                    transforms_applied: vec![],
                                    degrade: None,
                                    warning: Some(
                                        "SSE transport not supported by Codex".to_string(),
                                    ),
                                    dropped: Some(DroppedInfo {
                                        reason: "SSE transport not supported".to_string(),
                                    }),
                                },
                            );
                        }
                        "ws" => {
                            // Dropped under its own mapping id (mcp.transport_ws). Recorded via
                            // IRField.dropped; build_report surfaces it.
                            child.fields.insert(
                                "mcp.transport_ws".to_string(),
                                IRField {
                                    id: "mcp.transport_ws".to_string(),
                                    value: value.clone(),
                                    loss: Loss::Dropped,
                                    transforms_applied: vec![],
                                    degrade: None,
                                    warning: Some("WebSocket transport not supported".to_string()),
                                    dropped: Some(DroppedInfo {
                                        reason: "WebSocket transport not supported".to_string(),
                                    }),
                                },
                            );
                        }
                        _ => {
                            // stdio/http/streamable-http: the "type" field is implicit in Codex, so just record it
                            child.fields.insert(
                                "mcp.transport_type".to_string(),
                                IRField {
                                    id: "mcp.transport_type".to_string(),
                                    value: value.clone(),
                                    loss: Loss::Lossy,
                                    transforms_applied: vec![],
                                    degrade: None,
                                    warning: None,
                                    dropped: None,
                                },
                            );
                        }
                    }
                }
                // headers: special handling for Authorization Bearer
                "headers" => {
                    if let Some(headers) = value.as_object() {
                        self.lift_headers_c2x(headers, &mut child, idx);
                    }
                }
                // oauth: nested sub-object — iterate sub-keys and look up via "oauth.<sub_key>"
                "oauth" => {
                    if let Some(oauth_obj) = value.as_object() {
                        self.lift_oauth_c2x(oauth_obj, &mut child, idx);
                    }
                }
                // All other fields
                _ => {
                    if let Some(entry) = idx.get(key.as_str()) {
                        if !applies_direction(entry, ConvDir::C2x) {
                            continue;
                        }
                        let ctx = TransformCtx {
                            direction: ConvDir::C2x,
                            args: None,
                            field: entry,
                        };
                        let (v, applied) =
                            apply_transforms(value, entry.transform.as_deref(), &ctx);

                        let loss = Loss::from(&entry.loss);
                        let degrade_info = entry.degrade.as_ref().map(|d| DegradeInfo {
                            to: d.to.clone(),
                            target: d.target.clone(),
                        });
                        let dropped_info = if matches!(loss, Loss::Dropped) {
                            Some(DroppedInfo {
                                reason: entry
                                    .notes
                                    .clone()
                                    .unwrap_or_else(|| format!("{} dropped in Codex", key)),
                            })
                        } else {
                            None
                        };
                        // Dropped fields are recorded via IRField.dropped; no additional
                        // diagnostic is needed.  For genuinely lossy warn:true fields,
                        // emit one Warn diagnostic so build_report routes them correctly.
                        if entry.warn == Some(true) && !matches!(loss, Loss::Dropped) {
                            child.diagnostics.push(Diagnostic {
                                level: DiagLevel::Warn,
                                id: Some(entry.id.clone()),
                                message: entry.notes.clone().unwrap_or_else(|| entry.id.clone()),
                            });
                        }
                        child.fields.insert(
                            entry.id.clone(),
                            IRField {
                                id: entry.id.clone(),
                                value: v,
                                loss,
                                transforms_applied: applied,
                                degrade: degrade_info,
                                warning: None,
                                dropped: dropped_info,
                            },
                        );
                    } else {
                        // unknown field → drop
                        child.diagnostics.push(Diagnostic {
                            level: DiagLevel::Drop,
                            id: None,
                            message: format!("unknown MCP server field: {}", key),
                        });
                    }
                }
            }
        }

        // For http/streamable-http transport, env entries with ${VAR} values are
        // consumed into env_http_headers.  Convert them, then replace the
        // mcp.env IRField (which the generic arm inserted as Lossless) with a
        // Lossy marker so the report accurately reflects the transformation.
        let transport = cfg.get("type").and_then(|v| v.as_str()).unwrap_or("stdio");
        if transport == "http" || transport == "streamable-http" {
            if let Some(env_obj) = cfg.get("env").and_then(|v| v.as_object()) {
                self.convert_env_to_http_headers(env_obj, &mut child);
            }
            // Replace the Lossless mcp.env marker with a Lossy one to show
            // the field was transformed, not passed through unchanged.
            if let Some(env_field) = child.fields.get_mut("mcp.env") {
                env_field.loss = Loss::Lossy;
                env_field.warning =
                    Some("env converted to env_http_headers for http transport".to_string());
            }
        }

        // Store server_name as a tag
        child.source_path = server_name.to_string();

        Ok(child)
    }

    fn lift_headers_c2x(
        &self,
        headers: &Map<String, Value>,
        child: &mut IRNode,
        _idx: &std::collections::HashMap<String, &crate::core::mappings::MapEntry>,
    ) {
        // Authorization: "Bearer ${VAR}" → bearer_token_env_var: "VAR"
        if let Some(auth) = headers.get("Authorization") {
            if let Some(auth_str) = auth.as_str() {
                if let Some(var_name) = extract_bearer_env_var(auth_str) {
                    child.fields.insert(
                        "mcp.bearer".to_string(),
                        IRField {
                            id: "mcp.bearer".to_string(),
                            value: Value::String(var_name),
                            loss: Loss::Lossy,
                            transforms_applied: vec!["extract:bearer_env".to_string()],
                            degrade: None,
                            warning: Some(
                                "Bearer token extracted to bearer_token_env_var".to_string(),
                            ),
                            dropped: None,
                        },
                    );
                    // Apply the same ${VAR} split logic for remaining headers as the
                    // non-Bearer path: route ${VAR} values to env_http_headers and
                    // literal values to http_headers with a Warn diagnostic.
                    let mut env_http_headers: Map<String, Value> = Map::new();
                    let mut static_headers: Map<String, Value> = Map::new();
                    for (k, v) in headers.iter().filter(|(k, _)| *k != "Authorization") {
                        if let Some(val_str) = v.as_str() {
                            if let Some(bare) = extract_env_var_ref(val_str) {
                                env_http_headers.insert(k.clone(), Value::String(bare.to_string()));
                            } else {
                                child.diagnostics.push(Diagnostic {
                                    level: DiagLevel::Warn,
                                    id: Some("mcp.env_http_headers".to_string()),
                                    message: format!(
                                        "Header '{}' has literal value '{}': cannot auto-convert to env_http_headers (manual action required)",
                                        k, val_str
                                    ),
                                });
                                static_headers.insert(k.clone(), v.clone());
                            }
                        } else {
                            static_headers.insert(k.clone(), v.clone());
                        }
                    }
                    if !static_headers.is_empty() {
                        child.fields.insert(
                            "mcp.headers".to_string(),
                            IRField {
                                id: "mcp.headers".to_string(),
                                value: Value::Object(static_headers),
                                loss: Loss::Lossless,
                                transforms_applied: vec!["rename".to_string()],
                                degrade: None,
                                warning: None,
                                dropped: None,
                            },
                        );
                    }
                    if !env_http_headers.is_empty() {
                        child.fields.insert(
                            "mcp.env_http_headers".to_string(),
                            IRField {
                                id: "mcp.env_http_headers".to_string(),
                                value: Value::Object(env_http_headers),
                                loss: Loss::Lossy,
                                transforms_applied: vec![],
                                degrade: None,
                                warning: Some(
                                    "${VAR} headers converted to env_http_headers".to_string(),
                                ),
                                dropped: None,
                            },
                        );
                    }
                    return;
                }
            }
        }

        // When Authorization does not contain Bearer, convert all headers to http_headers.
        // Headers with ${VAR} patterns are converted to env_http_headers.
        let mut env_http_headers: Map<String, Value> = Map::new();
        let mut static_headers: Map<String, Value> = Map::new();

        for (k, v) in headers {
            if let Some(val_str) = v.as_str() {
                if let Some(var_name) = extract_env_var_ref(val_str) {
                    env_http_headers.insert(k.clone(), Value::String(var_name.to_string()));
                } else {
                    // Literal value — warn
                    child.diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some("mcp.env_http_headers".to_string()),
                        message: format!(
                            "Header '{}' has literal value '{}': cannot auto-convert to env_http_headers (manual action required)",
                            k, val_str
                        ),
                    });
                    static_headers.insert(k.clone(), v.clone());
                }
            } else {
                static_headers.insert(k.clone(), v.clone());
            }
        }

        if !static_headers.is_empty() {
            child.fields.insert(
                "mcp.headers".to_string(),
                IRField {
                    id: "mcp.headers".to_string(),
                    value: Value::Object(static_headers),
                    loss: Loss::Lossless,
                    transforms_applied: vec!["rename".to_string()],
                    degrade: None,
                    warning: None,
                    dropped: None,
                },
            );
        }

        if !env_http_headers.is_empty() {
            child.fields.insert(
                "mcp.env_http_headers".to_string(),
                IRField {
                    id: "mcp.env_http_headers".to_string(),
                    value: Value::Object(env_http_headers),
                    loss: Loss::Lossy,
                    transforms_applied: vec![],
                    degrade: None,
                    warning: Some("${VAR} headers converted to env_http_headers".to_string()),
                    dropped: None,
                },
            );
        }
    }

    /// Converts the oauth sub-object from Claude .mcp.json into IR fields.
    ///
    /// Each sub-key is looked up as `"oauth.<sub_key>"` in the index, and IR fields
    /// are produced using the same logic as the generic `_` arm of `lift_server_c2x`.
    fn lift_oauth_c2x(
        &self,
        oauth_obj: &Map<String, Value>,
        child: &mut IRNode,
        idx: &std::collections::HashMap<String, &crate::core::mappings::MapEntry>,
    ) {
        for (sub_key, value) in oauth_obj {
            let dot_key = format!("oauth.{}", sub_key);
            if let Some(entry) = idx.get(dot_key.as_str()) {
                if !applies_direction(entry, ConvDir::C2x) {
                    continue;
                }
                let ctx = TransformCtx {
                    direction: ConvDir::C2x,
                    args: None,
                    field: entry,
                };
                let (v, applied) = apply_transforms(value, entry.transform.as_deref(), &ctx);

                let loss = Loss::from(&entry.loss);
                let degrade_info = entry.degrade.as_ref().map(|d| DegradeInfo {
                    to: d.to.clone(),
                    target: d.target.clone(),
                });
                let dropped_info = if matches!(loss, Loss::Dropped) {
                    Some(DroppedInfo {
                        reason: entry
                            .notes
                            .clone()
                            .unwrap_or_else(|| format!("{} dropped in Codex", dot_key)),
                    })
                } else {
                    None
                };
                // Dropped fields are recorded via IRField.dropped; no additional
                // diagnostic is needed.  For genuinely lossy warn:true fields,
                // emit one Warn diagnostic so build_report routes them correctly.
                if entry.warn == Some(true) && !matches!(loss, Loss::Dropped) {
                    child.diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some(entry.id.clone()),
                        message: entry.notes.clone().unwrap_or_else(|| entry.id.clone()),
                    });
                }
                child.fields.insert(
                    entry.id.clone(),
                    IRField {
                        id: entry.id.clone(),
                        value: v,
                        loss,
                        transforms_applied: applied,
                        degrade: degrade_info,
                        warning: None,
                        dropped: dropped_info,
                    },
                );
            } else {
                child.diagnostics.push(Diagnostic {
                    level: DiagLevel::Drop,
                    id: None,
                    message: format!("unknown MCP oauth field: {}", sub_key),
                });
            }
        }
    }

    /// Converts the oauth sub-object from Codex config.toml into IR fields.
    ///
    /// Each sub-key is looked up in the x2c index as `"oauth.<sub_key>"` first, then as
    /// the bare `sub_key`. Codex's `scopes` maps to the `mcp.oauth.scopes` entry whose
    /// `codex.field` is `"scopes"` (bare), so the dot-prefixed form is tried first.
    fn lift_oauth_x2c(
        &self,
        oauth_obj: &Map<String, Value>,
        child: &mut IRNode,
        idx: &std::collections::HashMap<String, &crate::core::mappings::MapEntry>,
    ) {
        for (sub_key, value) in oauth_obj {
            let dot_key = format!("oauth.{}", sub_key);
            let entry = idx
                .get(dot_key.as_str())
                .or_else(|| idx.get(sub_key.as_str()));
            if let Some(entry) = entry {
                if !applies_direction(entry, ConvDir::X2c) {
                    continue;
                }
                let ctx = TransformCtx {
                    direction: ConvDir::X2c,
                    args: None,
                    field: entry,
                };
                let (v, applied) = apply_transforms(value, entry.transform.as_deref(), &ctx);

                let loss = Loss::from(&entry.loss);
                let dropped_info = if matches!(loss, Loss::Dropped) {
                    Some(DroppedInfo {
                        reason: entry
                            .notes
                            .clone()
                            .unwrap_or_else(|| format!("{} Codex-only field", sub_key)),
                    })
                } else {
                    None
                };
                // Dropped fields are recorded via IRField.dropped; no additional
                // diagnostic is needed.  For genuinely lossy warn:true fields,
                // emit one Warn diagnostic so build_report routes them correctly.
                if entry.warn == Some(true) && !matches!(loss, Loss::Dropped) {
                    child.diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some(entry.id.clone()),
                        message: entry.notes.clone().unwrap_or_else(|| entry.id.clone()),
                    });
                }
                child.fields.insert(
                    entry.id.clone(),
                    IRField {
                        id: entry.id.clone(),
                        value: v,
                        loss,
                        transforms_applied: applied,
                        degrade: None,
                        warning: None,
                        dropped: dropped_info,
                    },
                );
            } else {
                child.diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: None,
                    message: format!("unknown Codex MCP oauth field (x2c): {}", sub_key),
                });
            }
        }
    }

    /// Converts http-transport `env` entries into `env_http_headers`, merging
    /// with any existing `mcp.env_http_headers` IRField produced by `lift_headers_c2x`.
    ///
    /// When both `headers` and `env` are present on an http server, each source
    /// independently contributes entries.  Inserting a fresh IRField would
    /// overwrite the headers-derived entries, causing silent data loss.  Instead
    /// we extract the existing object (if any) and merge the new entries into it.
    fn convert_env_to_http_headers(&self, env: &Map<String, Value>, child: &mut IRNode) {
        // Collect new entries from env.
        let mut new_entries: Map<String, Value> = Map::new();
        for (k, v) in env {
            if let Some(val_str) = v.as_str() {
                if let Some(var_name) = extract_env_var_ref(val_str) {
                    new_entries.insert(k.clone(), Value::String(var_name.to_string()));
                } else {
                    // Literal value — cannot safely emit as env_http_headers.
                    child.diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some("mcp.env_http_headers".to_string()),
                        message: format!(
                            "http transport env '{}' has literal value: cannot emit as env_http_headers safely (manual action required)",
                            k
                        ),
                    });
                }
            }
        }

        if new_entries.is_empty() {
            return;
        }

        // Merge into the existing IRField if one was already inserted by
        // lift_headers_c2x, or insert a fresh one.
        if let Some(existing) = child.fields.get_mut("mcp.env_http_headers") {
            if let Value::Object(ref mut existing_map) = existing.value {
                for (k, v) in new_entries {
                    if existing_map.contains_key(&k) {
                        // Key collision: warn and keep the first (headers-derived) value.
                        child.diagnostics.push(Diagnostic {
                            level: DiagLevel::Warn,
                            id: Some("mcp.env_http_headers".to_string()),
                            message: format!(
                                "env_http_headers key '{}' from env conflicts with headers-derived entry; headers value kept",
                                k
                            ),
                        });
                    } else {
                        existing_map.insert(k, v);
                    }
                }
            }
        } else {
            child.fields.insert(
                "mcp.env_http_headers".to_string(),
                IRField {
                    id: "mcp.env_http_headers".to_string(),
                    value: Value::Object(new_entries),
                    loss: Loss::Lossy,
                    transforms_applied: vec![],
                    degrade: None,
                    warning: Some(
                        "env converted to env_http_headers for http transport".to_string(),
                    ),
                    dropped: None,
                },
            );
        }
    }

    /// Lift Codex config.toml → IR (x2c direction).
    fn lift_x2c(&self, parsed: &Value, node: &mut IRNode) -> anyhow::Result<()> {
        // For config.toml, mcp_servers is stored under "frontmatter"
        let frontmatter = parsed["frontmatter"]
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("Expected object at frontmatter"))?;

        // Tolerate missing mcp_servers (treat as empty map)
        let servers = match frontmatter.get("mcp_servers").and_then(|v| v.as_object()) {
            Some(s) => s,
            None => return Ok(()),
        };

        let idx = index_by_codex_field(&self.map);

        for (server_name, server_cfg) in servers {
            let child = self.lift_server_x2c(server_name, server_cfg, &idx)?;
            node.children.push(child);
        }

        Ok(())
    }

    fn lift_server_x2c(
        &self,
        server_name: &str,
        server_cfg: &Value,
        idx: &std::collections::HashMap<String, &crate::core::mappings::MapEntry>,
    ) -> anyhow::Result<IRNode> {
        let mut child = new_node(Kind::Mcp, Tool::Codex, server_name);

        let cfg = match server_cfg.as_object() {
            Some(o) => o,
            None => return Ok(child),
        };

        // Exclude entries with enabled: false
        if let Some(enabled) = cfg.get("enabled") {
            if enabled == &Value::Bool(false) {
                // Push exactly one Drop diagnostic so build_report records one entry.
                // lower_x2c detects disabled servers via this diagnostic; no IRField
                // is inserted to avoid surfacing internal bookkeeping in the report.
                child.diagnostics.push(Diagnostic {
                    level: DiagLevel::Drop,
                    id: Some("mcp.enabled".to_string()),
                    message: format!(
                        "Server '{}' has enabled=false: excluded from output",
                        server_name
                    ),
                });
                return Ok(child);
            }
        }

        // Transport determination: command present → stdio; url present → http
        let has_command = cfg.contains_key("command");
        let has_url = cfg.contains_key("url");
        let transport_type = if has_command {
            "stdio"
        } else if has_url {
            "http"
        } else {
            "stdio"
        };
        child.fields.insert(
            "mcp.transport_type".to_string(),
            IRField {
                id: "mcp.transport_type".to_string(),
                value: Value::String(transport_type.to_string()),
                loss: Loss::Lossy,
                transforms_applied: vec![],
                degrade: None,
                warning: None,
                dropped: None,
            },
        );

        for (key, value) in cfg {
            match key.as_str() {
                "enabled" => {
                    // already handled above
                    child.fields.insert(
                        "mcp.enabled".to_string(),
                        IRField {
                            id: "mcp.enabled".to_string(),
                            value: value.clone(),
                            loss: Loss::Dropped,
                            transforms_applied: vec![],
                            degrade: None,
                            warning: None,
                            dropped: Some(DroppedInfo {
                                reason: "Codex-only field".to_string(),
                            }),
                        },
                    );
                }
                "http_headers" => {
                    // http_headers → headers (rename)
                    child.fields.insert(
                        "mcp.headers".to_string(),
                        IRField {
                            id: "mcp.headers".to_string(),
                            value: value.clone(),
                            loss: Loss::Lossless,
                            transforms_applied: vec!["rename".to_string()],
                            degrade: None,
                            warning: None,
                            dropped: None,
                        },
                    );
                }
                // oauth: nested sub-object in Codex config.toml
                "oauth" => {
                    if let Some(oauth_obj) = value.as_object() {
                        self.lift_oauth_x2c(oauth_obj, &mut child, idx);
                    }
                }
                "bearer_token_env_var" => {
                    // bearer_token_env_var → headers.Authorization: "Bearer ${VAR}"
                    if let Some(var_name) = value.as_str() {
                        let auth_val = format!("Bearer ${{{}}}", var_name);
                        child.fields.insert(
                            "mcp.bearer".to_string(),
                            IRField {
                                id: "mcp.bearer".to_string(),
                                value: Value::String(auth_val),
                                loss: Loss::Lossy,
                                transforms_applied: vec!["extract:bearer_env".to_string()],
                                degrade: None,
                                warning: None,
                                dropped: None,
                            },
                        );
                    }
                }
                "tool_timeout_sec" => {
                    // tool_timeout_sec → timeout (unit:sec_to_ms)
                    if let Some(entry) = idx.get(key.as_str()) {
                        if applies_direction(entry, ConvDir::X2c) {
                            let ctx = TransformCtx {
                                direction: ConvDir::X2c,
                                args: None,
                                field: entry,
                            };
                            let (v, applied) =
                                apply_transforms(value, entry.transform.as_deref(), &ctx);
                            child.fields.insert(
                                "mcp.timeout".to_string(),
                                IRField {
                                    id: "mcp.timeout".to_string(),
                                    value: v,
                                    loss: Loss::Lossless,
                                    transforms_applied: applied,
                                    degrade: None,
                                    warning: None,
                                    dropped: None,
                                },
                            );
                        }
                    }
                }
                _ => {
                    if let Some(entry) = idx.get(key.as_str()) {
                        if !applies_direction(entry, ConvDir::X2c) {
                            continue;
                        }
                        let ctx = TransformCtx {
                            direction: ConvDir::X2c,
                            args: None,
                            field: entry,
                        };
                        let (v, applied) =
                            apply_transforms(value, entry.transform.as_deref(), &ctx);

                        let loss = Loss::from(&entry.loss);
                        let dropped_info = if matches!(loss, Loss::Dropped) {
                            Some(DroppedInfo {
                                reason: entry
                                    .notes
                                    .clone()
                                    .unwrap_or_else(|| format!("{} Codex-only field", key)),
                            })
                        } else {
                            None
                        };
                        // Dropped fields are recorded via IRField.dropped; no additional
                        // diagnostic is needed.  For genuinely lossy warn:true fields,
                        // emit one Warn diagnostic so build_report routes them correctly.
                        if entry.warn == Some(true) && !matches!(loss, Loss::Dropped) {
                            child.diagnostics.push(Diagnostic {
                                level: DiagLevel::Warn,
                                id: Some(entry.id.clone()),
                                message: entry.notes.clone().unwrap_or_else(|| entry.id.clone()),
                            });
                        }
                        child.fields.insert(
                            entry.id.clone(),
                            IRField {
                                id: entry.id.clone(),
                                value: v,
                                loss,
                                transforms_applied: applied,
                                degrade: None,
                                warning: None,
                                dropped: dropped_info,
                            },
                        );
                    } else {
                        // Record unknown Codex-specific fields with a warning
                        child.diagnostics.push(Diagnostic {
                            level: DiagLevel::Warn,
                            id: None,
                            message: format!("unknown Codex MCP field (x2c): {}", key),
                        });
                    }
                }
            }
        }

        child.source_path = server_name.to_string();
        Ok(child)
    }

    /// Generate Claude .mcp.json (c2x direction).
    fn lower_c2x(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let mut files = Vec::new();
        let diagnostics = Vec::new();

        let out_root = opts.out.as_deref().unwrap_or(".");
        let output_path = format!("{}/.mcp.json", out_root);

        let mut mcp_servers: Map<String, Value> = Map::new();

        for child in &ir.children {
            let server_name = child.source_path.clone();
            let server_cfg = self.build_codex_server_cfg(child)?;
            mcp_servers.insert(server_name, server_cfg);
        }

        let mcp_json = serde_json::json!({
            "mcpServers": mcp_servers
        });

        files.push(EmitFile {
            path: output_path,
            content: serde_json::to_string_pretty(&mcp_json)
                .with_context(|| "Failed to serialize .mcp.json")?,
        });

        Ok(EmitPlan { files, diagnostics })
    }

    /// x2c: Codex config.toml [mcp_servers.*] → Claude .mcp.json
    fn lower_x2c(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let mut files = Vec::new();
        let diagnostics = Vec::new();

        let out_root = opts.out.as_deref().unwrap_or(".");

        // x2c outputs .mcp.json only (config.toml is not emitted)
        let mut mcp_servers_map: Map<String, Value> = Map::new();

        for child in &ir.children {
            // Skip entries with enabled=false. Drop diagnostics are already pushed
            // to child.diagnostics by lift_server_x2c, so they are not added to
            // plan.diagnostics here.
            let is_disabled = child
                .diagnostics
                .iter()
                .any(|d| d.id.as_deref() == Some("mcp.enabled") && d.level == DiagLevel::Drop);
            if is_disabled {
                continue;
            }

            let server_name = child.source_path.clone();
            let server_cfg = self.build_claude_server_cfg(child)?;
            mcp_servers_map.insert(server_name, server_cfg);
        }

        let mcp_json_path = format!("{}/.mcp.json", out_root);
        if !mcp_servers_map.is_empty() {
            let mcp_json = serde_json::json!({ "mcpServers": mcp_servers_map });
            files.push(EmitFile {
                path: mcp_json_path,
                content: serde_json::to_string_pretty(&mcp_json)
                    .with_context(|| "Failed to serialize .mcp.json")?,
            });
        }

        Ok(EmitPlan { files, diagnostics })
    }

    /// Build Codex MCP server configuration from an IRNode child (c2x).
    fn build_codex_server_cfg(&self, child: &IRNode) -> anyhow::Result<Value> {
        let mut cfg: Map<String, Value> = Map::new();

        for (id, field) in &child.fields {
            match id.as_str() {
                "mcp.format" | "mcp.transport_type" => {
                    // Not emitted directly; transport_type is implied by command/url.
                }
                "mcp.command" => {
                    if let Some(entry) = self.map.entries.iter().find(|e| e.id == "mcp.command") {
                        if let Some(codex_field) =
                            entry.codex.as_ref().and_then(|c| c.field.as_ref())
                        {
                            cfg.insert(codex_field.clone(), field.value.clone());
                        }
                    }
                }
                "mcp.args" => {
                    if let Some(entry) = self.map.entries.iter().find(|e| e.id == "mcp.args") {
                        if let Some(codex_field) =
                            entry.codex.as_ref().and_then(|c| c.field.as_ref())
                        {
                            cfg.insert(codex_field.clone(), field.value.clone());
                        }
                    }
                }
                "mcp.env" => {
                    // env is stdio-only; for http/streamable-http it has already been
                    // converted to env_http_headers, so skip it.
                    let transport = child
                        .fields
                        .get("mcp.transport_type")
                        .and_then(|f| f.value.as_str())
                        .unwrap_or("stdio");
                    if transport == "http" || transport == "streamable-http" {
                        // Already converted to env_http_headers for http transport
                    } else if let Some(entry) = self.map.entries.iter().find(|e| e.id == "mcp.env")
                    {
                        if let Some(codex_field) =
                            entry.codex.as_ref().and_then(|c| c.field.as_ref())
                        {
                            cfg.insert(codex_field.clone(), field.value.clone());
                        }
                    }
                }
                "mcp.url" => {
                    if let Some(entry) = self.map.entries.iter().find(|e| e.id == "mcp.url") {
                        if let Some(codex_field) =
                            entry.codex.as_ref().and_then(|c| c.field.as_ref())
                        {
                            cfg.insert(codex_field.clone(), field.value.clone());
                        }
                    }
                }
                "mcp.headers" => {
                    // headers → http_headers (rename)
                    cfg.insert("http_headers".to_string(), field.value.clone());
                }
                "mcp.bearer" => {
                    // bearer_token_env_var
                    cfg.insert("bearer_token_env_var".to_string(), field.value.clone());
                }
                "mcp.env_http_headers" => {
                    cfg.insert("env_http_headers".to_string(), field.value.clone());
                }
                "mcp.timeout" => {
                    // timeout → tool_timeout_sec (unit:ms_to_sec)
                    cfg.insert("tool_timeout_sec".to_string(), field.value.clone());
                }
                "mcp.cwd" => {
                    cfg.insert("cwd".to_string(), field.value.clone());
                }
                "mcp.oauth.client_id" => {
                    // oauth.client_id (rename)
                    let oauth = cfg
                        .entry("oauth".to_string())
                        .or_insert_with(|| Value::Object(Map::new()));
                    if let Some(obj) = oauth.as_object_mut() {
                        obj.insert("client_id".to_string(), field.value.clone());
                    }
                }
                "mcp.oauth.callback_port" => {
                    cfg.insert("mcp_oauth_callback_port".to_string(), field.value.clone());
                }
                "mcp.oauth.scopes" => {
                    let oauth = cfg
                        .entry("oauth".to_string())
                        .or_insert_with(|| Value::Object(Map::new()));
                    if let Some(obj) = oauth.as_object_mut() {
                        obj.insert("scopes".to_string(), field.value.clone());
                    }
                }
                _ => {
                    // Dropped fields are already recorded via IRField.dropped and
                    // reported by build_report.  No additional diagnostic needed here.
                }
            }
        }

        Ok(Value::Object(cfg))
    }

    /// Build Claude MCP server configuration from an IRNode child (x2c).
    fn build_claude_server_cfg(&self, child: &IRNode) -> anyhow::Result<Value> {
        let mut cfg: Map<String, Value> = Map::new();

        // transport_type → "type" field
        if let Some(f) = child.fields.get("mcp.transport_type") {
            cfg.insert("type".to_string(), f.value.clone());
        }

        for (id, field) in &child.fields {
            match id.as_str() {
                "mcp.transport_type" | "mcp.enabled" => {}
                "mcp.command" => {
                    cfg.insert("command".to_string(), field.value.clone());
                }
                "mcp.args" => {
                    cfg.insert("args".to_string(), field.value.clone());
                }
                "mcp.env" => {
                    cfg.insert("env".to_string(), field.value.clone());
                }
                "mcp.url" => {
                    cfg.insert("url".to_string(), field.value.clone());
                }
                "mcp.headers" => {
                    // http_headers → headers (rename)
                    cfg.insert("headers".to_string(), field.value.clone());
                }
                "mcp.bearer" => {
                    // bearer_token_env_var → headers.Authorization: "Bearer ${VAR}"
                    let headers = cfg
                        .entry("headers".to_string())
                        .or_insert_with(|| Value::Object(Map::new()));
                    if let Some(obj) = headers.as_object_mut() {
                        obj.insert("Authorization".to_string(), field.value.clone());
                    }
                }
                "mcp.env_http_headers" => {
                    // env_http_headers → headers: wrap bare var names in ${...}
                    let headers = cfg
                        .entry("headers".to_string())
                        .or_insert_with(|| Value::Object(Map::new()));
                    if let Some(obj) = headers.as_object_mut() {
                        if let Some(env_headers) = field.value.as_object() {
                            for (k, v) in env_headers {
                                let expanded = if let Some(var_name) = v.as_str() {
                                    Value::String(format!("${{{}}}", var_name))
                                } else {
                                    v.clone()
                                };
                                obj.insert(k.clone(), expanded);
                            }
                        }
                    }
                }
                "mcp.timeout" => {
                    // tool_timeout_sec → timeout (ms)
                    cfg.insert("timeout".to_string(), field.value.clone());
                }
                "mcp.cwd" => {
                    cfg.insert("cwd".to_string(), field.value.clone());
                }
                "mcp.oauth.client_id" => {
                    let oauth = cfg
                        .entry("oauth".to_string())
                        .or_insert_with(|| Value::Object(Map::new()));
                    if let Some(obj) = oauth.as_object_mut() {
                        obj.insert("clientId".to_string(), field.value.clone());
                    }
                }
                "mcp.oauth.callback_port" => {
                    let oauth = cfg
                        .entry("oauth".to_string())
                        .or_insert_with(|| Value::Object(Map::new()));
                    if let Some(obj) = oauth.as_object_mut() {
                        obj.insert("callbackPort".to_string(), field.value.clone());
                    }
                }
                "mcp.oauth.scopes" => {
                    let oauth = cfg
                        .entry("oauth".to_string())
                        .or_insert_with(|| Value::Object(Map::new()));
                    if let Some(obj) = oauth.as_object_mut() {
                        obj.insert("scopes".to_string(), field.value.clone());
                    }
                }
                _ => {
                    // Dropped fields are already recorded via IRField.dropped and
                    // reported by build_report.  No additional diagnostic needed here.
                }
            }
        }

        Ok(Value::Object(cfg))
    }
}

/// Parses config.toml and returns a Value conforming to the handler's parse() contract.
/// The [mcp_servers.*] section is stored as "mcp_servers" under "frontmatter".
fn parse_toml_mcp_config(path: &Path) -> anyhow::Result<Value> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config.toml: {}", path.display()))?;

    let abs_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    // Convert TOML to serde_json::Value using the toml crate
    let toml_val: toml::Value = toml::from_str(&content)
        .with_context(|| format!("Failed to parse TOML: {}", path.display()))?;

    // Convert TOML Value → JSON Value
    let json_val = crate::core::serialize::toml_to_json(&toml_val)?;

    Ok(serde_json::json!({
        "frontmatter": json_val,
        "body": "",
        "path": abs_path.to_str().unwrap_or("")
    }))
}

/// Extracts VAR from a "Bearer ${VAR}" string.
fn extract_bearer_env_var(s: &str) -> Option<String> {
    if let Some(rest) = s.strip_prefix("Bearer ${") {
        if let Some(end) = rest.rfind('}') {
            let var_name = &rest[..end];
            if !var_name.is_empty() {
                return Some(var_name.to_string());
            }
        }
    }
    None
}

/// Extracts VAR from a "${VAR}" string.
/// Only pure environment-variable references of the form `${VAR}` are extracted.
/// Composite values such as `${VAR} suffix` return None (the trailing `}` is
/// verified to prevent partial matches).
fn extract_env_var_ref(s: &str) -> Option<String> {
    if s.starts_with("${") && s.ends_with('}') {
        let inner = &s[2..s.len() - 1];
        // For ${VAR:-default}, ignore the default part and extract only VAR
        let var_name = inner.split(":-").next().unwrap_or(inner);
        if !var_name.is_empty() {
            return Some(var_name.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mappings::load_mappings;
    use std::path::Path;
    use tempfile::TempDir;

    fn make_handler() -> McpHandler {
        let maps = load_mappings(Path::new("mappings"));
        McpHandler {
            map: maps["mcp"].clone(),
        }
    }

    fn default_opts() -> LowerOpts {
        LowerOpts {
            out: None,
            only: vec![],
            scope: crate::handlers::Scope::Project,
            dual_manifest: false,
            hooks_target: crate::handlers::Scope::User,
            skill_target: crate::handlers::SkillTargetMode::Skill,
            interactive: false,
            rewrite_body: false,
            keep_claude_frontmatter: false,
        }
    }

    #[test]
    fn test_mcp_detect() {
        let h = make_handler();
        assert!(h.detect(Path::new(".mcp.json")));
        assert!(!h.detect(Path::new("SKILL.md")));
    }

    #[test]
    fn test_mcp_lift_c2x_basic() {
        let dir = TempDir::new().unwrap();
        let mcp_path = dir.path().join(".mcp.json");
        std::fs::write(
            &mcp_path,
            r#"{
  "mcpServers": {
    "my-server": {
      "command": "npx",
      "args": ["-y", "@example/mcp-server"],
      "env": {"API_KEY": "test123"}
    }
  }
}"#,
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&mcp_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        assert_eq!(ir.kind, Kind::Mcp);
        assert_eq!(ir.children.len(), 1);
        let child = &ir.children[0];
        assert_eq!(child.source_path, "my-server");
        assert!(child.fields.contains_key("mcp.command"));
        assert!(child.fields.contains_key("mcp.args"));
    }

    #[test]
    fn test_mcp_lift_c2x_sse_ws_dropped_under_own_ids() {
        let dir = TempDir::new().unwrap();
        let mcp_path = dir.path().join(".mcp.json");
        std::fs::write(
            &mcp_path,
            r#"{
  "mcpServers": {
    "sse-server": {"type": "sse", "url": "https://example.com/sse"},
    "ws-server": {"type": "ws", "url": "wss://example.com/ws"}
  }
}"#,
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&mcp_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        let sse = ir
            .children
            .iter()
            .find(|c| c.source_path == "sse-server")
            .unwrap();
        // The drop is attributed to its own mapping id, not mcp.transport_type.
        assert!(sse.fields.contains_key("mcp.transport_sse"));
        assert!(!sse.fields.contains_key("mcp.transport_type"));
        assert_eq!(sse.fields["mcp.transport_sse"].loss, Loss::Dropped);

        let ws = ir
            .children
            .iter()
            .find(|c| c.source_path == "ws-server")
            .unwrap();
        assert!(ws.fields.contains_key("mcp.transport_ws"));
        assert_eq!(ws.fields["mcp.transport_ws"].loss, Loss::Dropped);
    }

    #[test]
    fn test_mcp_lift_c2x_timeout() {
        let dir = TempDir::new().unwrap();
        let mcp_path = dir.path().join(".mcp.json");
        std::fs::write(
            &mcp_path,
            r#"{"mcpServers": {"srv": {"command": "node", "timeout": 60000}}}"#,
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&mcp_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        let child = &ir.children[0];
        let timeout = child.fields.get("mcp.timeout").unwrap();
        // 60000ms → 60.0sec
        assert_eq!(timeout.value.as_f64().unwrap(), 60.0);
    }

    #[test]
    fn test_mcp_lift_c2x_bearer() {
        let dir = TempDir::new().unwrap();
        let mcp_path = dir.path().join(".mcp.json");
        std::fs::write(
            &mcp_path,
            r#"{"mcpServers": {"srv": {"url": "https://api.example.com", "headers": {"Authorization": "Bearer ${MY_TOKEN}"}}}}"#,
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&mcp_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        let child = &ir.children[0];
        let bearer = child.fields.get("mcp.bearer").unwrap();
        assert_eq!(bearer.value.as_str().unwrap(), "MY_TOKEN");
    }

    #[test]
    fn test_mcp_lower_c2x_generates_mcp_json() {
        let dir = TempDir::new().unwrap();
        let mcp_path = dir.path().join(".mcp.json");
        std::fs::write(
            &mcp_path,
            r#"{"mcpServers": {"my-server": {"command": "npx", "args": ["-y", "@example/mcp-server"]}}}"#,
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        let mut opts = default_opts();
        opts.out = Some(out_dir.to_str().unwrap().to_string());

        let h = make_handler();
        let parsed = h.parse(&mcp_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // Verify that .mcp.json was generated.
        // c2x converts to Codex, so .mcp.json is the output format.
        assert!(!plan.files.is_empty());
        let mcp_file = plan.files.iter().find(|f| f.path.ends_with(".mcp.json"));
        assert!(mcp_file.is_some(), "Expected .mcp.json in output");
        let content: Value = serde_json::from_str(&mcp_file.unwrap().content).unwrap();
        assert!(content["mcpServers"]["my-server"]["command"]
            .as_str()
            .is_some());
    }

    #[test]
    fn test_mcp_lift_c2x_dropped_fields() {
        let dir = TempDir::new().unwrap();
        let mcp_path = dir.path().join(".mcp.json");
        std::fs::write(
            &mcp_path,
            r#"{"mcpServers": {"srv": {"command": "node", "alwaysLoad": true, "headersHelper": "echo {}"}}}"#,
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&mcp_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        // alwaysLoad, headersHelper are Claude-specific, so either Drop diagnostics
        // are emitted or they are dropped as unknown fields.
        let child = &ir.children[0];
        let drop_diags: Vec<_> = child
            .diagnostics
            .iter()
            .filter(|d| d.level == DiagLevel::Drop)
            .collect();
        // alwaysLoad and headersHelper are either unknown MCP fields or dropped
        assert!(
            !drop_diags.is_empty()
                || child
                    .fields
                    .iter()
                    .any(|(_, f)| matches!(f.loss, Loss::Dropped))
        );
    }

    #[test]
    fn test_extract_bearer_env_var() {
        assert_eq!(
            extract_bearer_env_var("Bearer ${MY_TOKEN}"),
            Some("MY_TOKEN".to_string())
        );
        assert_eq!(extract_bearer_env_var("Token ${OTHER}"), None);
    }

    #[test]
    fn test_extract_env_var_ref() {
        assert_eq!(
            extract_env_var_ref("${API_KEY}"),
            Some("API_KEY".to_string())
        );
        assert_eq!(
            extract_env_var_ref("${API_KEY:-default}"),
            Some("API_KEY".to_string())
        );
        assert_eq!(extract_env_var_ref("literal_value"), None);
    }

    // gap 5/42: OAuth nested fields silently dropped

    /// c2x: oauth sub-object must produce mcp.oauth.client_id (lossless),
    /// mcp.oauth.scopes (lossless, array), mcp.oauth.callback_port (lossy),
    /// and mcp.oauth.auth_server_metadata_url (dropped+warn).
    #[test]
    fn test_mcp_lift_c2x_oauth_roundtrip() {
        let dir = TempDir::new().unwrap();
        let mcp_path = dir.path().join(".mcp.json");
        std::fs::write(
            &mcp_path,
            r#"{
  "mcpServers": {
    "s": {
      "type": "http",
      "url": "https://x.com",
      "oauth": {
        "clientId": "id",
        "scopes": "a:read b:write",
        "callbackPort": 9876,
        "authServerMetadataUrl": "https://auth.example.com"
      }
    }
  }
}"#,
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&mcp_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let child = &ir.children[0];

        // mcp.oauth.client_id: lossless
        let cid = child
            .fields
            .get("mcp.oauth.client_id")
            .expect("mcp.oauth.client_id must be in IR");
        assert_eq!(cid.value, Value::String("id".to_string()));
        assert!(matches!(cid.loss, Loss::Lossless));

        // mcp.oauth.scopes: lossless, array after str_to_list:space
        let scopes = child
            .fields
            .get("mcp.oauth.scopes")
            .expect("mcp.oauth.scopes must be in IR");
        assert!(matches!(scopes.loss, Loss::Lossless));
        let arr = scopes.value.as_array().expect("scopes must be array");
        assert_eq!(
            arr,
            &vec![
                Value::String("a:read".to_string()),
                Value::String("b:write".to_string()),
            ]
        );

        // mcp.oauth.callback_port: lossy
        let cp = child
            .fields
            .get("mcp.oauth.callback_port")
            .expect("mcp.oauth.callback_port must be in IR");
        assert!(matches!(cp.loss, Loss::Lossy));
        assert_eq!(cp.value, Value::Number(serde_json::Number::from(9876)));

        // mcp.oauth.auth_server_metadata_url: dropped
        // The field is represented via IRField.loss == Dropped; build_report reads
        // it from ir.fields.  No additional Diagnostic is pushed (doing so would
        // cause each dropped field to be counted multiple times in the summary).
        let asm = child
            .fields
            .get("mcp.oauth.auth_server_metadata_url")
            .expect("mcp.oauth.auth_server_metadata_url must be in IR");
        assert!(matches!(asm.loss, Loss::Dropped));
        // No spurious Diagnostic must be pushed for this dropped field.
        let has_spurious_diag = child
            .diagnostics
            .iter()
            .any(|d| d.id.as_deref() == Some("mcp.oauth.auth_server_metadata_url"));
        assert!(
            !has_spurious_diag,
            "mcp.oauth.auth_server_metadata_url must NOT push a redundant Diagnostic \
             (the IRField.dropped entry is the canonical source); diagnostics: {:?}",
            child.diagnostics
        );

        // no unknown-field diagnostic for oauth
        let has_unknown = child
            .diagnostics
            .iter()
            .any(|d| d.message.contains("unknown MCP server field: oauth"));
        assert!(
            !has_unknown,
            "oauth must not produce unknown-field diagnostic"
        );
    }

    /// c2x: server with both headers (${VAR}) and env (${VAR}) must merge both
    /// into a single env_http_headers IRField — no silent overwrite.
    #[test]
    fn test_lift_c2x_merges_headers_and_env_into_env_http_headers() {
        let parsed = serde_json::json!({
            "frontmatter": {
                "mcpServers": {
                    "s": {
                        "type": "http",
                        "url": "https://x.com",
                        "headers": { "X-From-Headers": "${FROM_HEADERS}" },
                        "env":     { "API_KEY": "${API_KEY}" }
                    }
                }
            },
            "body": ""
        });

        let h = make_handler();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let server = ir.children.iter().find(|c| c.source_path == "s").unwrap();

        // Only one env_http_headers field must exist (merged)
        let env_hdr = server
            .fields
            .get("mcp.env_http_headers")
            .expect("mcp.env_http_headers must be present");
        let hdr_obj = env_hdr
            .value
            .as_object()
            .expect("env_http_headers must be an object");

        assert!(
            hdr_obj.contains_key("X-From-Headers"),
            "headers-derived entry must survive merge: {:?}",
            hdr_obj
        );
        assert_eq!(
            hdr_obj["X-From-Headers"],
            Value::String("FROM_HEADERS".to_string())
        );
        assert!(
            hdr_obj.contains_key("API_KEY"),
            "env-derived entry must survive merge: {:?}",
            hdr_obj
        );
        assert_eq!(hdr_obj["API_KEY"], Value::String("API_KEY".to_string()));

        // mcp.env must NOT remain as a separate Lossless IRField for http transport
        // (it was fully consumed by env_http_headers, so it should not show lossless)
        if let Some(env_field) = server.fields.get("mcp.env") {
            assert!(
                !matches!(env_field.loss, Loss::Lossless),
                "mcp.env must not be Lossless for http transport (it was transformed)"
            );
        }
    }

    /// x2c: Codex config.toml with [oauth] sub-table must produce
    /// mcp.oauth.client_id (lossless) and mcp.oauth.scopes (lossless, joined string).
    #[test]
    fn test_mcp_lift_x2c_oauth() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"[mcp_servers.s]
url = "https://x.com"

[mcp_servers.s.oauth]
client_id = "id"
scopes = ["a:read", "b:write"]
"#,
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&config_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::X2c).unwrap();
        let child = &ir.children[0];

        // mcp.oauth.client_id: lossless
        let cid = child
            .fields
            .get("mcp.oauth.client_id")
            .expect("mcp.oauth.client_id must be in x2c IR");
        assert_eq!(cid.value, Value::String("id".to_string()));
        assert!(matches!(cid.loss, Loss::Lossless));

        // mcp.oauth.scopes: lossless, joined by space
        let scopes = child
            .fields
            .get("mcp.oauth.scopes")
            .expect("mcp.oauth.scopes must be in x2c IR");
        assert!(matches!(scopes.loss, Loss::Lossless));
        assert_eq!(
            scopes.value,
            Value::String("a:read b:write".to_string()),
            "scopes must be joined by space in x2c"
        );
    }
}
