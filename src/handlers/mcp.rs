use std::path::Path;

use anyhow::Context;
use serde_json::{Map, Value};

use crate::core::ir::{
    new_node, DegradeInfo, DiagLevel, Diagnostic, DroppedInfo, IRField, IRNode, Kind, Loss, Tool,
};
use crate::core::mappings::{
    applies_direction, index_by_claude_field, index_by_codex_field, DomainMap, LossSpec,
};
use crate::core::transforms::{apply_transforms, ConvDir, TransformCtx};
use crate::handlers::{EmitFile, EmitPlan, Handler, LowerOpts};

/// MCP ドメインのハンドラ。
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
            // config.toml は TOML 形式でパース
            parse_toml_mcp_config(path)
        } else {
            // .mcp.json は JSON 形式でパース
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
    /// Claude .mcp.json → IR（c2x 方向）
    fn lift_c2x(&self, parsed: &Value, node: &mut IRNode) -> anyhow::Result<()> {
        let frontmatter = parsed["frontmatter"]
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("Expected object at top level of .mcp.json"))?;

        // .mcp.json トップレベル: { "mcpServers": { "<name>": { ... } } }
        let servers = match frontmatter.get("mcpServers").and_then(|v| v.as_object()) {
            Some(s) => s,
            None => return Ok(()),
        };

        let idx = index_by_claude_field(&self.map);

        // mcp.format エントリの記録
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

        // 各サーバーを子 IRNode として処理
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
                // transport 判定: type フィールドは特殊処理
                "type" => {
                    let transport_type = value.as_str().unwrap_or("");
                    match transport_type {
                        "sse" => {
                            child.diagnostics.push(Diagnostic {
                                level: DiagLevel::Drop,
                                id: Some("mcp.transport_sse".to_string()),
                                message: "SSE transport not supported by Codex (dropped)"
                                    .to_string(),
                            });
                            child.fields.insert(
                                "mcp.transport_type".to_string(),
                                IRField {
                                    id: "mcp.transport_type".to_string(),
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
                            child.diagnostics.push(Diagnostic {
                                level: DiagLevel::Drop,
                                id: Some("mcp.transport_ws".to_string()),
                                message: "WebSocket transport not supported by Codex (dropped)"
                                    .to_string(),
                            });
                            child.fields.insert(
                                "mcp.transport_type".to_string(),
                                IRField {
                                    id: "mcp.transport_type".to_string(),
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
                            // stdio/http/streamable-http: type フィールドは Codex では暗黙なので記録だけ
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
                // headers: Authorization Bearer の特殊処理
                "headers" => {
                    if let Some(headers) = value.as_object() {
                        self.lift_headers_c2x(headers, &mut child, idx);
                    }
                }
                // その他のフィールド
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

                        let loss = match entry.loss {
                            LossSpec::Lossless => Loss::Lossless,
                            LossSpec::Lossy => Loss::Lossy,
                            LossSpec::Dropped => Loss::Dropped,
                        };
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
                        if entry.warn == Some(true) {
                            child.diagnostics.push(Diagnostic {
                                level: if matches!(loss, Loss::Dropped) {
                                    DiagLevel::Drop
                                } else {
                                    DiagLevel::Warn
                                },
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

        // env の http_transport 特殊変換
        if let Some(env_obj) = cfg.get("env").and_then(|v| v.as_object()) {
            // transport type を確認
            let transport = cfg.get("type").and_then(|v| v.as_str()).unwrap_or("stdio");
            if transport == "http" || transport == "streamable-http" {
                // http transport の env → env_http_headers 変換
                self.convert_env_to_http_headers(env_obj, &mut child);
            }
        }

        // server_name をタグとして保存
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
                    // 他のヘッダを http_headers として収集
                    let other_headers: Map<String, Value> = headers
                        .iter()
                        .filter(|(k, _)| *k != "Authorization")
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    if !other_headers.is_empty() {
                        child.fields.insert(
                            "mcp.headers".to_string(),
                            IRField {
                                id: "mcp.headers".to_string(),
                                value: Value::Object(other_headers),
                                loss: Loss::Lossless,
                                transforms_applied: vec!["rename".to_string()],
                                degrade: None,
                                warning: None,
                                dropped: None,
                            },
                        );
                    }
                    return;
                }
            }
        }

        // Authorization に Bearer がない場合は全ヘッダを http_headers に変換
        // ${VAR} パターンのヘッダは env_http_headers に変換
        let mut env_http_headers: Map<String, Value> = Map::new();
        let mut static_headers: Map<String, Value> = Map::new();

        for (k, v) in headers {
            if let Some(val_str) = v.as_str() {
                if let Some(var_name) = extract_env_var_ref(val_str) {
                    env_http_headers.insert(k.clone(), Value::String(format!("${}", var_name)));
                } else {
                    // リテラル値は warn
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

    /// http transport の env フィールドを env_http_headers に変換する
    fn convert_env_to_http_headers(&self, env: &Map<String, Value>, child: &mut IRNode) {
        let mut env_http: Map<String, Value> = Map::new();
        for (k, v) in env {
            if let Some(val_str) = v.as_str() {
                if let Some(var_name) = extract_env_var_ref(val_str) {
                    // ${VAR} → $VAR
                    env_http.insert(k.clone(), Value::String(format!("${}", var_name)));
                } else {
                    // リテラル値 → warn
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
        if !env_http.is_empty() {
            child.fields.insert(
                "mcp.env_http_headers".to_string(),
                IRField {
                    id: "mcp.env_http_headers".to_string(),
                    value: Value::Object(env_http),
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

    /// Codex config.toml → IR（x2c 方向）
    fn lift_x2c(&self, parsed: &Value, node: &mut IRNode) -> anyhow::Result<()> {
        // config.toml の場合は frontmatter に mcp_servers が格納されている
        let frontmatter = parsed["frontmatter"]
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("Expected object at frontmatter"))?;

        // mcp_servers が存在しない場合も許容（空 map として処理）
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

        // enabled: false のエントリは除外
        if let Some(enabled) = cfg.get("enabled") {
            if enabled == &Value::Bool(false) {
                child.diagnostics.push(Diagnostic {
                    level: DiagLevel::Drop,
                    id: Some("mcp.enabled".to_string()),
                    message: format!(
                        "Server '{}' has enabled=false: excluded from output",
                        server_name
                    ),
                });
                // enabled=false フラグを設定して lower で除外できるようにする
                child.fields.insert(
                    "__disabled".to_string(),
                    IRField {
                        id: "__disabled".to_string(),
                        value: Value::Bool(true),
                        loss: Loss::Dropped,
                        transforms_applied: vec![],
                        degrade: None,
                        warning: None,
                        dropped: None,
                    },
                );
                return Ok(child);
            }
        }

        // transport 判定: command 有 → stdio、url 有 → http
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

                        let loss = match entry.loss {
                            LossSpec::Lossless => Loss::Lossless,
                            LossSpec::Lossy => Loss::Lossy,
                            LossSpec::Dropped => Loss::Dropped,
                        };
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
                        if entry.warn == Some(true) {
                            child.diagnostics.push(Diagnostic {
                                level: if matches!(loss, Loss::Dropped) {
                                    DiagLevel::Drop
                                } else {
                                    DiagLevel::Warn
                                },
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
                        // Codex 固有のフィールドはそのまま記録しておく（warn）
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

    /// Claude .mcp.json を生成（c2x 方向）
    fn lower_c2x(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let mut files = Vec::new();
        let mut diagnostics = Vec::new();

        let out_root = opts.out.as_deref().unwrap_or(".");
        let output_path = format!("{}/.mcp.json", out_root);

        let mut mcp_servers: Map<String, Value> = Map::new();

        for child in &ir.children {
            let server_name = child.source_path.clone();
            let server_cfg = self.build_codex_server_cfg(child, &mut diagnostics)?;
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

    /// x2c: Codex config.toml の [mcp_servers.*] → Claude .mcp.json
    fn lower_x2c(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let mut files = Vec::new();
        let mut diagnostics = Vec::new();

        let out_root = opts.out.as_deref().unwrap_or(".");

        // x2c は .mcp.json のみ出力する（config.toml は出力しない）
        let mut mcp_servers_map: Map<String, Value> = Map::new();

        for child in &ir.children {
            // enabled=false は除外
            if child.fields.contains_key("__disabled") {
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Drop,
                    id: Some("mcp.enabled".to_string()),
                    message: format!("Server '{}' excluded (enabled=false)", child.source_path),
                });
                continue;
            }

            let server_name = child.source_path.clone();
            let server_cfg = self.build_claude_server_cfg(child, &mut diagnostics)?;
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

    /// IRNode(child) から Codex MCP サーバー設定を構築する（c2x）
    fn build_codex_server_cfg(
        &self,
        child: &IRNode,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> anyhow::Result<Value> {
        let mut cfg: Map<String, Value> = Map::new();

        for (id, field) in &child.fields {
            match id.as_str() {
                "mcp.format" | "mcp.transport_type" | "__disabled" => {
                    // これらは直接出力しない（transport_type は command/url から暗黙に決まる）
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
                    // env は stdio 専用。http/streamable-http では env_http_headers に変換済みなのでスキップ。
                    let transport = child
                        .fields
                        .get("mcp.transport_type")
                        .and_then(|f| f.value.as_str())
                        .unwrap_or("stdio");
                    if transport == "http" || transport == "streamable-http" {
                        // http transport では env は env_http_headers に変換済みなのでスキップ
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
                    // dropped フィールドは出力しない
                    if matches!(field.loss, Loss::Dropped) {
                        diagnostics.push(Diagnostic {
                            level: DiagLevel::Drop,
                            id: Some(id.clone()),
                            message: format!("{} dropped in c2x", id),
                        });
                    }
                }
            }
        }

        Ok(Value::Object(cfg))
    }

    /// IRNode(child) から Claude MCP サーバー設定を構築する（x2c）
    fn build_claude_server_cfg(
        &self,
        child: &IRNode,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> anyhow::Result<Value> {
        let mut cfg: Map<String, Value> = Map::new();

        // transport_type → type フィールド
        if let Some(f) = child.fields.get("mcp.transport_type") {
            cfg.insert("type".to_string(), f.value.clone());
        }

        for (id, field) in &child.fields {
            match id.as_str() {
                "mcp.transport_type" | "__disabled" | "mcp.enabled" => {}
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
                    // env_http_headers → headers (${VAR} 展開)
                    let headers = cfg
                        .entry("headers".to_string())
                        .or_insert_with(|| Value::Object(Map::new()));
                    if let Some(obj) = headers.as_object_mut() {
                        if let Some(env_headers) = field.value.as_object() {
                            for (k, v) in env_headers {
                                obj.insert(k.clone(), v.clone());
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
                    if matches!(field.loss, Loss::Dropped) {
                        diagnostics.push(Diagnostic {
                            level: DiagLevel::Drop,
                            id: Some(id.clone()),
                            message: format!("{} dropped in x2c", id),
                        });
                    }
                }
            }
        }

        Ok(Value::Object(cfg))
    }
}

/// config.toml をパースして handler の parse() 契約に従う Value を返す。
/// [mcp_servers.*] セクションを mcpServers として frontmatter に格納する。
fn parse_toml_mcp_config(path: &Path) -> anyhow::Result<Value> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config.toml: {}", path.display()))?;

    let abs_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    // toml を serde_json::Value に変換
    // toml クレートを使って型変換
    let toml_val: toml::Value = content
        .parse()
        .with_context(|| format!("Failed to parse TOML: {}", path.display()))?;

    // TOML Value → JSON Value の変換
    let json_val = toml_to_json(&toml_val);

    Ok(serde_json::json!({
        "frontmatter": json_val,
        "body": "",
        "path": abs_path.to_str().unwrap_or("")
    }))
}

/// toml::Value → serde_json::Value の変換ヘルパ。
fn toml_to_json(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::String(s.clone()),
        toml::Value::Integer(i) => Value::Number(serde_json::Number::from(*i)),
        toml::Value::Float(f) => {
            Value::Number(serde_json::Number::from_f64(*f).unwrap_or(serde_json::Number::from(0)))
        }
        toml::Value::Boolean(b) => Value::Bool(*b),
        toml::Value::Array(arr) => Value::Array(arr.iter().map(toml_to_json).collect()),
        toml::Value::Table(tbl) => {
            let mut map = serde_json::Map::new();
            for (k, v) in tbl {
                map.insert(k.clone(), toml_to_json(v));
            }
            Value::Object(map)
        }
        toml::Value::Datetime(dt) => Value::String(dt.to_string()),
    }
}

/// "Bearer ${VAR}" から VAR を抽出するヘルパ。
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

/// "${VAR}" から VAR を抽出するヘルパ。
/// 値全体が `${VAR}` 形式（純粋な環境変数参照）のみ抽出する。
/// `${VAR} suffix` のような複合値は None を返す（部分マッチを防止するため末尾 '}' を検証）。
fn extract_env_var_ref(s: &str) -> Option<String> {
    if s.starts_with("${") && s.ends_with('}') {
        let inner = &s[2..s.len() - 1];
        // ${VAR:-default} のデフォルト値部分は無視し VAR のみ取得
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
            scope: crate::handlers::Scope::Project,
            dual_manifest: false,
            hooks_target: crate::handlers::Scope::User,
            skill_target: crate::handlers::SkillTargetMode::Skill,
            interactive: false,
            rewrite_body: false,
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

        // .mcp.json が生成されているか確認（c2x では Claude の .mcp.json のまま変換先）
        // 実際は Codex 側への変換なので .mcp.json が生成される
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

        // alwaysLoad, headersHelper は Claude 固有なので Drop 診断が出るか、
        // or 未知フィールドとして Drop される
        let child = &ir.children[0];
        let drop_diags: Vec<_> = child
            .diagnostics
            .iter()
            .filter(|d| d.level == DiagLevel::Drop)
            .collect();
        // alwaysLoad と headersHelper は unknown MCP fields or dropped
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
}
