use anyhow::Context;
use serde_json::{Map, Value};

use crate::core::ir::{DiagLevel, IRNode};
use crate::handlers::{EmitFile, EmitPlan, LowerOpts};

use super::McpHandler;

impl McpHandler {
    /// Generate Claude .mcp.json (c2x direction).
    pub(super) fn lower_c2x(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
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
    pub(super) fn lower_x2c(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
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
    pub(super) fn build_codex_server_cfg(&self, child: &IRNode) -> anyhow::Result<Value> {
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
    pub(super) fn build_claude_server_cfg(&self, child: &IRNode) -> anyhow::Result<Value> {
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
