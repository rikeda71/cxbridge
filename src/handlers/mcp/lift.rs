use serde_json::{Map, Value};

use crate::core::ir::{
    new_node, DegradeInfo, DiagLevel, Diagnostic, DroppedInfo, IRField, IRNode, Kind, Loss, Tool,
};
use crate::core::mappings::{applies_direction, index_by_claude_field, index_by_codex_field};
use crate::core::transforms::{apply_transforms, ConvDir, TransformCtx};

use super::parse::{extract_bearer_env_var, extract_env_var_ref};
use super::McpHandler;

impl McpHandler {
    /// Lift Claude .mcp.json → IR (c2x direction).
    pub(super) fn lift_c2x(&self, parsed: &Value, node: &mut IRNode) -> anyhow::Result<()> {
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

    pub(super) fn lift_server_c2x(
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

    pub(super) fn lift_headers_c2x(
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
    pub(super) fn lift_oauth_c2x(
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
    pub(super) fn lift_oauth_x2c(
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
    pub(super) fn convert_env_to_http_headers(&self, env: &Map<String, Value>, child: &mut IRNode) {
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
    pub(super) fn lift_x2c(&self, parsed: &Value, node: &mut IRNode) -> anyhow::Result<()> {
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

    pub(super) fn lift_server_x2c(
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
}
