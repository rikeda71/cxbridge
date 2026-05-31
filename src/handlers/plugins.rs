use std::path::Path;

use anyhow::Context;
use serde_json::{Map, Value};

use crate::core::ir::{
    new_node, DiagLevel, Diagnostic, DroppedInfo, IRField, IRNode, Kind, Loss, SideArtifact, Tool,
};
use crate::core::mappings::{applies_direction, DomainMap, LossSpec};
use crate::core::transforms::{apply_transforms, ConvDir, TransformCtx};
use crate::handlers::{EmitFile, EmitPlan, Handler, LowerOpts};

/// plugins ドメインのハンドラ。
/// plugin.json の lift/lower に加え、配下の skills/hooks/.mcp.json を
/// 各ハンドラに委譲して再帰変換し、children に格納する。
pub struct PluginsHandler {
    pub map: DomainMap,
}

impl Handler for PluginsHandler {
    fn kind(&self) -> Kind {
        Kind::Plugin
    }

    fn detect(&self, path: &Path) -> bool {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        name == "plugin.json"
    }

    fn parse(&self, path: &Path) -> anyhow::Result<Value> {
        crate::core::serialize::json::parse_json_file(path)
    }

    fn lift(&self, parsed: &Value, dir: ConvDir) -> anyhow::Result<IRNode> {
        let source_tool = match dir {
            ConvDir::C2x => Tool::Claude,
            ConvDir::X2c => Tool::Codex,
        };
        let source_path = parsed["path"].as_str().unwrap_or("").to_string();
        let mut node = new_node(Kind::Plugin, source_tool, &source_path);

        let frontmatter = match parsed["frontmatter"].as_object() {
            Some(fm) => fm,
            None => return Ok(node),
        };

        // scope:"plugin" のエントリのみを索引する（marketplace などの同名フィールドと衝突しないため）
        let idx = build_plugin_scope_index(&self.map, dir);

        // manifest フィールドを mappings 駆動で lift する
        self.lift_manifest_fields(frontmatter, &idx, dir, &mut node);

        // 配下の子コンポーネントを再帰変換する
        // plugin.json の親ディレクトリをプラグインルートとして使用
        let plugin_root = Path::new(&source_path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        // skills/ ディレクトリを SkillsHandler で再帰変換
        self.lift_child_skills(&plugin_root, frontmatter, dir, &mut node);

        // hooks ファイルを HooksHandler で再帰変換
        self.lift_child_hooks(&plugin_root, frontmatter, dir, &mut node);

        // .mcp.json を McpHandler で再帰変換
        self.lift_child_mcp(&plugin_root, frontmatter, dir, &mut node);

        // marketplace.json を処理（同一ディレクトリに存在すれば）
        self.lift_marketplace(&plugin_root, dir, &mut node);

        Ok(node)
    }

    fn lower(&self, ir: &IRNode, dir: ConvDir, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        match dir {
            ConvDir::C2x => self.lower_c2x(ir, opts),
            ConvDir::X2c => self.lower_x2c(ir, opts),
        }
    }
}

/// scope:"plugin" のエントリのみを索引する（marketplace などの同名フィールドと衝突しないため）。
/// c2x なら claude フィールド、x2c なら codex フィールドで索引する。
fn build_plugin_scope_index(
    map: &DomainMap,
    dir: ConvDir,
) -> std::collections::HashMap<String, crate::core::mappings::MapEntry> {
    let mut idx = std::collections::HashMap::new();
    for entry in &map.entries {
        let spec = match dir {
            ConvDir::C2x => entry.claude.as_ref(),
            ConvDir::X2c => entry.codex.as_ref(),
        };
        let Some(spec) = spec else { continue };
        // scope:"plugin" のみ対象（marketplace / null を除外）
        if spec.scope.as_deref() != Some("plugin") {
            continue;
        }
        let Some(field) = spec.field.as_ref() else {
            continue;
        };
        // 全角括弧（U+FF08）で始まるプレースホルダはスキップ
        if field.starts_with('\u{FF08}') {
            continue;
        }
        // 先に登録されたエントリを優先（後から来た重複フィールドは上書きしない）
        idx.entry(field.clone()).or_insert_with(|| entry.clone());
    }
    idx
}

impl PluginsHandler {
    /// manifest フィールドを mappings 駆動で lift する。
    fn lift_manifest_fields(
        &self,
        frontmatter: &Map<String, Value>,
        idx: &std::collections::HashMap<String, crate::core::mappings::MapEntry>,
        dir: ConvDir,
        node: &mut IRNode,
    ) {
        // userConfig の未解決変数チェックを後で行うため、userConfig を保存
        let user_config = frontmatter.get("userConfig");

        for (key, value) in frontmatter {
            // experimental は特殊処理（サブフィールドを展開）
            if key == "experimental" {
                if let Some(exp_obj) = value.as_object() {
                    for (sub_key, sub_value) in exp_obj {
                        let full_key = format!("experimental.{}", sub_key);
                        self.lift_single_field(&full_key, sub_value, idx, dir, node);
                    }
                }
                continue;
            }

            self.lift_single_field(key, value, idx, dir, node);
        }

        // c2x: userConfig が存在し、MCP/hooks 本文などに ${user_config.KEY} が残る可能性を warn
        if dir == ConvDir::C2x {
            if let Some(uc) = user_config {
                if uc.is_object() || uc.is_array() {
                    node.diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some("plugins.userConfig".to_string()),
                        message: "userConfig found: ${user_config.KEY} references in MCP/hooks may remain unresolved after c2x conversion (Codex has no userConfig equivalent)".to_string(),
                    });
                }
            }
        }
    }

    fn lift_single_field(
        &self,
        key: &str,
        value: &Value,
        idx: &std::collections::HashMap<String, crate::core::mappings::MapEntry>,
        dir: ConvDir,
        node: &mut IRNode,
    ) {
        let Some(entry) = idx.get(key) else {
            // 未知フィールド: dropped 扱い
            node.diagnostics.push(Diagnostic {
                level: DiagLevel::Drop,
                id: None,
                message: format!("unknown plugin manifest field: {key}"),
            });
            return;
        };

        if !applies_direction(entry, dir) {
            return;
        }

        let ctx = TransformCtx {
            direction: dir,
            args: None,
            field: entry,
        };
        let (v, applied) = apply_transforms(value, entry.transform.as_deref(), &ctx);

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
                    .unwrap_or_else(|| format!("{} has no equivalent", key)),
            })
        } else {
            None
        };

        let warning = if entry.warn == Some(true) {
            Some(format!(
                "{}: {}",
                entry.id,
                entry.notes.as_deref().unwrap_or("warn")
            ))
        } else {
            None
        };

        if entry.warn == Some(true) {
            node.diagnostics.push(Diagnostic {
                level: if matches!(loss, Loss::Dropped) {
                    DiagLevel::Drop
                } else {
                    DiagLevel::Warn
                },
                id: Some(entry.id.clone()),
                message: entry
                    .notes
                    .clone()
                    .unwrap_or_else(|| format!("{} (warn)", entry.id)),
            });
        }

        node.fields.insert(
            entry.id.clone(),
            IRField {
                id: entry.id.clone(),
                value: v,
                loss,
                transforms_applied: applied,
                degrade: None,
                warning,
                dropped: dropped_info,
            },
        );
    }

    /// skills/ ディレクトリを再帰変換して children に追加する。
    fn lift_child_skills(
        &self,
        plugin_root: &str,
        frontmatter: &Map<String, Value>,
        dir: ConvDir,
        node: &mut IRNode,
    ) {
        // skills パスを決定（manifest の skills フィールド or デフォルト ./skills/）
        let skills_dir = frontmatter
            .get("skills")
            .and_then(|v| v.as_str())
            .unwrap_or("./skills/");

        // パスを正規化（./skills/ → skills）
        let skills_rel = skills_dir.trim_start_matches("./").trim_end_matches('/');

        let skills_path = format!("{}/{}", plugin_root, skills_rel);
        let skills_path = Path::new(&skills_path);

        if !skills_path.exists() {
            return;
        }

        let maps = crate::core::mappings::load_mappings(Path::new("mappings"));
        let skills_handler = crate::handlers::skills::SkillsHandler {
            map: maps["skills"].clone(),
        };

        // skills/ 配下の各 SKILL.md を処理
        if let Ok(entries) = std::fs::read_dir(skills_path) {
            for entry in entries.flatten() {
                let skill_dir = entry.path();
                if !skill_dir.is_dir() {
                    continue;
                }
                let skill_md = skill_dir.join("SKILL.md");
                if !skill_md.exists() {
                    continue;
                }

                match skills_handler.parse(&skill_md) {
                    Ok(parsed) => match skills_handler.lift(&parsed, dir) {
                        Ok(child_ir) => {
                            node.children.push(child_ir);
                        }
                        Err(e) => {
                            node.diagnostics.push(Diagnostic {
                                level: DiagLevel::Warn,
                                id: None,
                                message: format!("Failed to lift skill {:?}: {}", skill_md, e),
                            });
                        }
                    },
                    Err(e) => {
                        node.diagnostics.push(Diagnostic {
                            level: DiagLevel::Warn,
                            id: None,
                            message: format!("Failed to parse skill {:?}: {}", skill_md, e),
                        });
                    }
                }
            }
        }
    }

    /// hooks ファイルを再帰変換して children に追加する。
    fn lift_child_hooks(
        &self,
        plugin_root: &str,
        frontmatter: &Map<String, Value>,
        dir: ConvDir,
        node: &mut IRNode,
    ) {
        // hooks パスを決定（manifest の hooks フィールド or デフォルト ./hooks/hooks.json）
        let hooks_path_str = frontmatter
            .get("hooks")
            .and_then(|v| v.as_str())
            .unwrap_or("./hooks/hooks.json");

        let hooks_rel = hooks_path_str.trim_start_matches("./");
        let hooks_path = format!("{}/{}", plugin_root, hooks_rel);
        let hooks_path = Path::new(&hooks_path);

        if !hooks_path.exists() {
            return;
        }

        // hooks がオブジェクト（インライン）の場合は warn のみ
        if let Some(hooks_obj) = frontmatter.get("hooks").and_then(|v| v.as_object()) {
            node.diagnostics.push(Diagnostic {
                level: DiagLevel::Warn,
                id: Some("plugins.hooks".to_string()),
                message: format!(
                    "Inline hooks object in plugin.json has {} entries; writing to hooks file for Codex compatibility",
                    hooks_obj.len()
                ),
            });
        }

        let maps = crate::core::mappings::load_mappings(Path::new("mappings"));
        let hooks_handler = crate::handlers::hooks::HooksHandler {
            map: maps["hooks"].clone(),
        };

        match hooks_handler.parse(hooks_path) {
            Ok(parsed) => match hooks_handler.lift(&parsed, dir) {
                Ok(child_ir) => {
                    // hooks #16430 warn: plugin 同梱 hooks は Codex で読まれない可能性
                    let mut child_ir = child_ir;
                    child_ir.diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some("plugins.hooks".to_string()),
                        message: "Plugin-bundled hooks may not be loaded by Codex (#16430). Use --hooks-target=user|project to output hooks to ~/.codex/hooks.json or .codex/config.toml instead.".to_string(),
                    });
                    node.children.push(child_ir);
                }
                Err(e) => {
                    node.diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: None,
                        message: format!("Failed to lift hooks {:?}: {}", hooks_path, e),
                    });
                }
            },
            Err(e) => {
                node.diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: None,
                    message: format!("Failed to parse hooks {:?}: {}", hooks_path, e),
                });
            }
        }
    }

    /// .mcp.json を再帰変換して children に追加する。
    fn lift_child_mcp(
        &self,
        plugin_root: &str,
        frontmatter: &Map<String, Value>,
        dir: ConvDir,
        node: &mut IRNode,
    ) {
        // mcpServers パスを決定（manifest の mcpServers フィールド or デフォルト ./.mcp.json）
        let mcp_path_str = frontmatter
            .get("mcpServers")
            .and_then(|v| v.as_str())
            .unwrap_or("./.mcp.json");

        // インラインオブジェクト形式は lossy（パス参照のみ対応）
        if frontmatter
            .get("mcpServers")
            .map(|v| v.is_object())
            .unwrap_or(false)
        {
            node.diagnostics.push(Diagnostic {
                level: DiagLevel::Warn,
                id: Some("plugins.mcpServers".to_string()),
                message: "Inline mcpServers object in plugin.json: Codex requires a file path reference. Will attempt to emit as .mcp.json.".to_string(),
            });
        }

        let mcp_rel = mcp_path_str.trim_start_matches("./");
        let mcp_path = format!("{}/{}", plugin_root, mcp_rel);
        let mcp_path = Path::new(&mcp_path);

        if !mcp_path.exists() {
            return;
        }

        let maps = crate::core::mappings::load_mappings(Path::new("mappings"));
        let mcp_handler = crate::handlers::mcp::McpHandler {
            map: maps["mcp"].clone(),
        };

        match mcp_handler.parse(mcp_path) {
            Ok(parsed) => match mcp_handler.lift(&parsed, dir) {
                Ok(child_ir) => {
                    node.children.push(child_ir);
                }
                Err(e) => {
                    node.diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: None,
                        message: format!("Failed to lift .mcp.json {:?}: {}", mcp_path, e),
                    });
                }
            },
            Err(e) => {
                node.diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: None,
                    message: format!("Failed to parse .mcp.json {:?}: {}", mcp_path, e),
                });
            }
        }
    }

    /// marketplace.json を処理して side_artifacts に格納する。
    /// plugin_root は plugin.json が存在するディレクトリ（例: `.claude-plugin/`）。
    fn lift_marketplace(&self, plugin_root: &str, dir: ConvDir, node: &mut IRNode) {
        // marketplace.json は plugin.json と同じディレクトリに置かれる
        // Claude: .claude-plugin/marketplace.json（= {plugin_root}/marketplace.json）
        // Codex: .agents/plugins/marketplace.json（= {plugin_root}/marketplace.json）
        let local_marketplace = format!("{}/marketplace.json", plugin_root);

        let marketplace_path = match dir {
            ConvDir::C2x => {
                let p = Path::new(&local_marketplace);
                if p.exists() {
                    Some(p.to_path_buf())
                } else {
                    None
                }
            }
            ConvDir::X2c => {
                let p = Path::new(&local_marketplace);
                if p.exists() {
                    Some(p.to_path_buf())
                } else {
                    None
                }
            }
        };

        let Some(mp_path) = marketplace_path else {
            return;
        };

        match std::fs::read_to_string(&mp_path) {
            Ok(content) => {
                // marketplace.json を保存（lower で変換して emit する）
                node.side_artifacts.push(SideArtifact {
                    path: mp_path.to_string_lossy().to_string(),
                    content,
                    note: "marketplace.json".to_string(),
                });
            }
            Err(e) => {
                node.diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: None,
                    message: format!("Failed to read marketplace.json {:?}: {}", mp_path, e),
                });
            }
        }
    }

    /// c2x: Claude plugin → Codex plugin 変換
    fn lower_c2x(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let mut files = Vec::new();
        let mut diagnostics = Vec::new();

        let out_root = opts.out.as_deref().unwrap_or(".");

        // manifest JSON を構築
        let codex_manifest = self.build_codex_manifest(ir, &mut diagnostics);

        // --dual-manifest: .claude-plugin/ を残置しつつ .codex-plugin/ を追加生成
        if opts.dual_manifest {
            // Claude 側 manifest を残置（元ファイルから読み直して emit）
            if let Ok(content) = std::fs::read_to_string(&ir.source_path) {
                files.push(EmitFile {
                    path: format!("{}/.claude-plugin/plugin.json", out_root),
                    content,
                });
            }
        }

        // Codex 側 manifest を生成
        let codex_json = serde_json::to_string_pretty(&codex_manifest)
            .with_context(|| "Failed to serialize Codex plugin.json")?;
        files.push(EmitFile {
            path: format!("{}/.codex-plugin/plugin.json", out_root),
            content: codex_json,
        });

        // 子ノードの EmitPlan をマージ
        // skills children
        let maps = crate::core::mappings::load_mappings(Path::new("mappings"));
        for child_ir in &ir.children {
            match child_ir.kind {
                Kind::Skill => {
                    let skill_handler = crate::handlers::skills::SkillsHandler {
                        map: maps["skills"].clone(),
                    };
                    match skill_handler.lower(child_ir, ConvDir::C2x, opts) {
                        Ok(plan) => {
                            files.extend(plan.files);
                            diagnostics.extend(plan.diagnostics);
                        }
                        Err(e) => {
                            diagnostics.push(Diagnostic {
                                level: DiagLevel::Warn,
                                id: None,
                                message: format!("Failed to lower skill child: {}", e),
                            });
                        }
                    }
                }
                Kind::Hooks => {
                    let hooks_handler = crate::handlers::hooks::HooksHandler {
                        map: maps["hooks"].clone(),
                    };
                    match hooks_handler.lower(child_ir, ConvDir::C2x, opts) {
                        Ok(plan) => {
                            files.extend(plan.files);
                            diagnostics.extend(plan.diagnostics);
                        }
                        Err(e) => {
                            diagnostics.push(Diagnostic {
                                level: DiagLevel::Warn,
                                id: None,
                                message: format!("Failed to lower hooks child: {}", e),
                            });
                        }
                    }
                }
                Kind::Mcp => {
                    let mcp_handler = crate::handlers::mcp::McpHandler {
                        map: maps["mcp"].clone(),
                    };
                    match mcp_handler.lower(child_ir, ConvDir::C2x, opts) {
                        Ok(plan) => {
                            files.extend(plan.files);
                            diagnostics.extend(plan.diagnostics);
                        }
                        Err(e) => {
                            diagnostics.push(Diagnostic {
                                level: DiagLevel::Warn,
                                id: None,
                                message: format!("Failed to lower MCP child: {}", e),
                            });
                        }
                    }
                }
                _ => {}
            }
        }

        // marketplace.json の変換
        for artifact in &ir.side_artifacts {
            if artifact.note == "marketplace.json" {
                let transformed =
                    self.transform_marketplace_c2x(&artifact.content, &mut diagnostics);
                files.push(EmitFile {
                    path: format!("{}/.agents/plugins/marketplace.json", out_root),
                    content: transformed,
                });
            }
        }

        Ok(EmitPlan { files, diagnostics })
    }

    /// x2c: Codex plugin → Claude plugin 変換
    fn lower_x2c(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let mut files = Vec::new();
        let mut diagnostics = Vec::new();

        let out_root = opts.out.as_deref().unwrap_or(".");

        // manifest JSON を構築
        let claude_manifest = self.build_claude_manifest(ir, &mut diagnostics);

        // Claude 側 manifest を生成
        let claude_json = serde_json::to_string_pretty(&claude_manifest)
            .with_context(|| "Failed to serialize Claude plugin.json")?;
        files.push(EmitFile {
            path: format!("{}/.claude-plugin/plugin.json", out_root),
            content: claude_json,
        });

        // 子ノードの EmitPlan をマージ
        let maps = crate::core::mappings::load_mappings(Path::new("mappings"));
        for child_ir in &ir.children {
            match child_ir.kind {
                Kind::Skill => {
                    let skill_handler = crate::handlers::skills::SkillsHandler {
                        map: maps["skills"].clone(),
                    };
                    match skill_handler.lower(child_ir, ConvDir::X2c, opts) {
                        Ok(plan) => {
                            files.extend(plan.files);
                            diagnostics.extend(plan.diagnostics);
                        }
                        Err(e) => {
                            diagnostics.push(Diagnostic {
                                level: DiagLevel::Warn,
                                id: None,
                                message: format!("Failed to lower skill child: {}", e),
                            });
                        }
                    }
                }
                Kind::Hooks => {
                    let hooks_handler = crate::handlers::hooks::HooksHandler {
                        map: maps["hooks"].clone(),
                    };
                    match hooks_handler.lower(child_ir, ConvDir::X2c, opts) {
                        Ok(plan) => {
                            files.extend(plan.files);
                            diagnostics.extend(plan.diagnostics);
                        }
                        Err(e) => {
                            diagnostics.push(Diagnostic {
                                level: DiagLevel::Warn,
                                id: None,
                                message: format!("Failed to lower hooks child: {}", e),
                            });
                        }
                    }
                }
                Kind::Mcp => {
                    let mcp_handler = crate::handlers::mcp::McpHandler {
                        map: maps["mcp"].clone(),
                    };
                    match mcp_handler.lower(child_ir, ConvDir::X2c, opts) {
                        Ok(plan) => {
                            files.extend(plan.files);
                            diagnostics.extend(plan.diagnostics);
                        }
                        Err(e) => {
                            diagnostics.push(Diagnostic {
                                level: DiagLevel::Warn,
                                id: None,
                                message: format!("Failed to lower MCP child: {}", e),
                            });
                        }
                    }
                }
                _ => {}
            }
        }

        // marketplace.json の変換
        for artifact in &ir.side_artifacts {
            if artifact.note == "marketplace.json" {
                let transformed =
                    self.transform_marketplace_x2c(&artifact.content, &mut diagnostics);
                files.push(EmitFile {
                    path: format!("{}/.claude-plugin/marketplace.json", out_root),
                    content: transformed,
                });
            }
        }

        Ok(EmitPlan { files, diagnostics })
    }

    /// IR から Codex 向け plugin.json を構築する（c2x）。
    fn build_codex_manifest(&self, ir: &IRNode, diagnostics: &mut Vec<Diagnostic>) -> Value {
        let mut manifest = Map::new();

        // fields から Codex フィールドへ変換
        for (id, field) in &ir.fields {
            // dropped フィールドはスキップ（report 用の診断のみ追加）
            if matches!(field.loss, Loss::Dropped) {
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Drop,
                    id: Some(id.clone()),
                    message: format!("{} dropped (no Codex equivalent)", id),
                });
                continue;
            }

            // entry から Codex フィールド名を取得
            let codex_field = self
                .map
                .entries
                .iter()
                .find(|e| e.id == *id)
                .and_then(|e| e.codex.as_ref())
                .and_then(|c| c.field.as_ref())
                .map(|s| s.as_str());

            let Some(cf) = codex_field else {
                continue;
            };

            // ネストフィールド（interface.displayName 等）の処理
            if let Some(dot_pos) = cf.find('.') {
                let parent = &cf[..dot_pos];
                let child_key = &cf[dot_pos + 1..];
                let parent_obj = manifest
                    .entry(parent.to_string())
                    .or_insert_with(|| Value::Object(Map::new()));
                if let Some(obj) = parent_obj.as_object_mut() {
                    obj.insert(child_key.to_string(), field.value.clone());
                }
            } else {
                manifest.insert(cf.to_string(), field.value.clone());
            }
        }

        // version が存在しない場合に semver "0.0.0" を補完する
        if !manifest.contains_key("version") {
            manifest.insert("version".to_string(), Value::String("0.0.0".to_string()));
            diagnostics.push(Diagnostic {
                level: DiagLevel::Warn,
                id: Some("plugins.version".to_string()),
                message: "version field missing: auto-completed as '0.0.0' (Codex requires strict semver)".to_string(),
            });
        } else if let Some(ver) = manifest.get("version").and_then(|v| v.as_str()) {
            // semver 補完: メジャー.マイナー.パッチ形式でなければ補完
            let completed = complete_semver(ver);
            if completed != ver {
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("plugins.version".to_string()),
                    message: format!(
                        "version '{}' completed to semver '{}' (Codex requires strict semver)",
                        ver, completed
                    ),
                });
                manifest.insert("version".to_string(), Value::String(completed));
            }
        }

        // description がない場合は name から補完
        if !manifest.contains_key("description") {
            if let Some(name) = manifest.get("name").and_then(|v| v.as_str()) {
                manifest.insert(
                    "description".to_string(),
                    Value::String(format!("Plugin: {}", name)),
                );
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("plugins.description".to_string()),
                    message: "description field missing: auto-filled from name (Codex requires description)".to_string(),
                });
            }
        }

        Value::Object(manifest)
    }

    /// IR から Claude 向け plugin.json を構築する（x2c）。
    fn build_claude_manifest(&self, ir: &IRNode, diagnostics: &mut Vec<Diagnostic>) -> Value {
        let mut manifest = Map::new();

        for (id, field) in &ir.fields {
            if matches!(field.loss, Loss::Dropped) {
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Drop,
                    id: Some(id.clone()),
                    message: format!("{} dropped (no Claude equivalent)", id),
                });
                continue;
            }

            // entry から Claude フィールド名を取得
            let claude_field = self
                .map
                .entries
                .iter()
                .find(|e| e.id == *id)
                .and_then(|e| e.claude.as_ref())
                .and_then(|c| c.field.as_ref())
                .map(|s| s.as_str());

            let Some(cf) = claude_field else {
                continue;
            };

            // ネストフィールド（experimental.themes 等）の処理
            if let Some(dot_pos) = cf.find('.') {
                let parent = &cf[..dot_pos];
                let child_key = &cf[dot_pos + 1..];
                let parent_obj = manifest
                    .entry(parent.to_string())
                    .or_insert_with(|| Value::Object(Map::new()));
                if let Some(obj) = parent_obj.as_object_mut() {
                    obj.insert(child_key.to_string(), field.value.clone());
                }
            } else {
                manifest.insert(cf.to_string(), field.value.clone());
            }
        }

        Value::Object(manifest)
    }

    /// marketplace.json を Codex 向けに変換する（c2x）。
    /// - source スキーマを正規化（Claude `relative`/string → Codex `{source:"local",...}`）
    /// - policy がなければデフォルト値を補完
    fn transform_marketplace_c2x(
        &self,
        content: &str,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> String {
        let Ok(mut json): Result<Value, _> = serde_json::from_str(content) else {
            return content.to_string();
        };

        if let Some(plugins) = json.get_mut("plugins").and_then(|v| v.as_array_mut()) {
            for plugin_entry in plugins.iter_mut() {
                if let Some(obj) = plugin_entry.as_object_mut() {
                    // source スキーマ正規化
                    normalize_marketplace_source_c2x(obj);

                    // policy が未設定なら既定値を補完
                    if !obj.contains_key("policy") {
                        obj.insert(
                            "policy".to_string(),
                            serde_json::json!({
                                "installation": "AVAILABLE",
                                "authentication": "ON_INSTALL"
                            }),
                        );
                        diagnostics.push(Diagnostic {
                            level: DiagLevel::Warn,
                            id: Some("plugins.marketplace.plugins.policy".to_string()),
                            message: "marketplace plugin.policy auto-filled with defaults (installation=AVAILABLE, authentication=ON_INSTALL)".to_string(),
                        });
                    }
                }
            }
        }

        serde_json::to_string_pretty(&json).unwrap_or_else(|_| content.to_string())
    }

    /// marketplace.json を Claude 向けに変換する（x2c）。
    /// - source スキーマを正規化（Codex `local` → Claude 相対パス）
    /// - policy は Claude に対応なし（dropped）
    fn transform_marketplace_x2c(
        &self,
        content: &str,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> String {
        let Ok(mut json): Result<Value, _> = serde_json::from_str(content) else {
            return content.to_string();
        };

        if let Some(plugins) = json.get_mut("plugins").and_then(|v| v.as_array_mut()) {
            for plugin_entry in plugins.iter_mut() {
                if let Some(obj) = plugin_entry.as_object_mut() {
                    // source スキーマ正規化
                    normalize_marketplace_source_x2c(obj);

                    // policy は Claude に対応なし（dropped）
                    if obj.remove("policy").is_some() {
                        diagnostics.push(Diagnostic {
                            level: DiagLevel::Drop,
                            id: Some("plugins.marketplace.plugins.policy".to_string()),
                            message: "marketplace plugin.policy dropped (no Claude equivalent)"
                                .to_string(),
                        });
                    }
                }
            }
        }

        serde_json::to_string_pretty(&json).unwrap_or_else(|_| content.to_string())
    }
}

/// semver を補完する（メジャーのみ → メジャー.0.0、メジャー.マイナー → メジャー.マイナー.0）。
fn complete_semver(ver: &str) -> String {
    // git SHA (40 hex chars) の場合は "0.0.0" に変換
    if ver.len() == 40 && ver.chars().all(|c| c.is_ascii_hexdigit()) {
        return "0.0.0".to_string();
    }

    let parts: Vec<&str> = ver.split('.').collect();
    match parts.len() {
        1 => {
            // メジャーのみ
            if parts[0].parse::<u64>().is_ok() {
                format!("{}.0.0", parts[0])
            } else {
                "0.0.0".to_string()
            }
        }
        2 => {
            // メジャー.マイナー
            if parts[0].parse::<u64>().is_ok() && parts[1].parse::<u64>().is_ok() {
                format!("{}.{}.0", parts[0], parts[1])
            } else {
                "0.0.0".to_string()
            }
        }
        _ => ver.to_string(), // 3 要素以上はそのまま
    }
}

/// marketplace.json の source スキーマを Codex 向けに正規化する。
/// - 相対パス文字列 → `{source: "local", path: "..."}`
/// - `github` は概ねそのまま（フィールド名の違いがあれば warn）
fn normalize_marketplace_source_c2x(obj: &mut Map<String, Value>) {
    if let Some(source) = obj.get("source").cloned() {
        match &source {
            Value::String(s) => {
                // 相対パス文字列 → Codex local 形式
                let normalized = serde_json::json!({
                    "source": "local",
                    "path": s
                });
                obj.insert("source".to_string(), normalized);
            }
            Value::Object(src_obj) => {
                // すでにオブジェクト形式の場合は source タイプを確認
                if let Some(src_type) = src_obj.get("source").and_then(|v| v.as_str()) {
                    if src_type == "relative" {
                        // Claude `relative` → Codex `local`
                        let mut new_src = src_obj.clone();
                        new_src.insert("source".to_string(), Value::String("local".to_string()));
                        obj.insert("source".to_string(), Value::Object(new_src));
                    }
                    // npm は Codex に対応なし
                    if src_type == "npm" {
                        obj.insert("source".to_string(), Value::Null);
                    }
                }
            }
            _ => {}
        }
    }
}

/// marketplace.json の source スキーマを Claude 向けに正規化する。
/// - `{source: "local", path: "..."}` → 相対パス文字列
fn normalize_marketplace_source_x2c(obj: &mut Map<String, Value>) {
    if let Some(source) = obj.get("source").cloned() {
        if let Some(src_obj) = source.as_object() {
            if let Some(src_type) = src_obj.get("source").and_then(|v| v.as_str()) {
                if src_type == "local" {
                    // Codex `local` → Claude 相対パス文字列
                    if let Some(path) = src_obj.get("path").and_then(|v| v.as_str()) {
                        obj.insert("source".to_string(), Value::String(path.to_string()));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mappings::load_mappings;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn make_handler() -> PluginsHandler {
        let maps = load_mappings(Path::new("mappings"));
        PluginsHandler {
            map: maps["plugins"].clone(),
        }
    }

    fn default_opts(out: &str) -> LowerOpts {
        LowerOpts {
            out: Some(out.to_string()),
            scope: crate::handlers::Scope::Project,
            dual_manifest: false,
            hooks_target: crate::handlers::Scope::User,
            skill_target: crate::handlers::SkillTargetMode::Skill,
            interactive: false,
            rewrite_body: false,
        }
    }

    /// 基本的なプラグインフィクスチャを作成する。
    fn create_claude_plugin_fixture(dir: &Path) -> std::path::PathBuf {
        // .claude-plugin/plugin.json を作成
        let plugin_dir = dir.join(".claude-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();
        let plugin_json = plugin_dir.join("plugin.json");
        fs::write(
            &plugin_json,
            r#"{
  "name": "test-plugin",
  "version": "1.2.3",
  "description": "A test plugin",
  "author": {"name": "Test Author", "email": "test@example.com"},
  "homepage": "https://example.com",
  "license": "MIT",
  "keywords": ["test", "plugin"],
  "skills": "./skills/"
}"#,
        )
        .unwrap();

        // skills/ ディレクトリと SKILL.md を作成
        let skills_dir = dir.join(".claude-plugin").join("skills").join("my-skill");
        fs::create_dir_all(&skills_dir).unwrap();
        fs::write(
            skills_dir.join("SKILL.md"),
            "---\nname: my-skill\ndescription: My skill\n---\nDo something.\n",
        )
        .unwrap();

        // .mcp.json を作成
        let mcp_json = dir.join(".claude-plugin").join(".mcp.json");
        fs::write(
            &mcp_json,
            r#"{"mcpServers": {"my-server": {"command": "npx", "args": ["-y", "@my/server"]}}}"#,
        )
        .unwrap();

        plugin_json
    }

    #[test]
    fn test_plugins_detect() {
        let h = make_handler();
        assert!(h.detect(Path::new("plugin.json")));
        assert!(!h.detect(Path::new("SKILL.md")));
        assert!(!h.detect(Path::new(".mcp.json")));
    }

    #[test]
    fn test_plugins_lift_c2x_basic() {
        let dir = TempDir::new().unwrap();
        let plugin_json = create_claude_plugin_fixture(dir.path());

        let h = make_handler();
        let parsed = h.parse(&plugin_json).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        assert_eq!(ir.kind, Kind::Plugin);
        // name, description, version は lossless で lift されるはず
        assert!(ir.fields.contains_key("plugins.name"));
        assert!(ir.fields.contains_key("plugins.version"));
        assert!(ir.fields.contains_key("plugins.description"));
        let name_f = &ir.fields["plugins.name"];
        assert_eq!(name_f.value, Value::String("test-plugin".to_string()));
        assert_eq!(name_f.loss, Loss::Lossless);
    }

    #[test]
    fn test_plugins_lift_c2x_dropped_fields() {
        let dir = TempDir::new().unwrap();
        let plugin_dir = dir.path().join(".claude-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();
        let plugin_json = plugin_dir.join("plugin.json");
        // dropped フィールドを含む plugin.json
        fs::write(
            &plugin_json,
            r#"{
  "name": "test-plugin",
  "version": "1.0.0",
  "description": "A test plugin",
  "lspServers": "./lsp.json",
  "outputStyles": "./styles/",
  "channels": [],
  "settings": {"agent": "test"},
  "dependencies": ["other-plugin"],
  "userConfig": {"MY_KEY": {"type": "string", "title": "My Key", "description": "desc"}}
}"#,
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&plugin_json).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        // dropped fields should be present with Loss::Dropped
        let has_lsp_dropped = ir
            .fields
            .get("plugins.lspServers")
            .map(|f| matches!(f.loss, Loss::Dropped))
            .unwrap_or(false);
        let has_output_dropped = ir
            .fields
            .get("plugins.outputStyles")
            .map(|f| matches!(f.loss, Loss::Dropped))
            .unwrap_or(false);
        let has_channels_dropped = ir
            .fields
            .get("plugins.channels")
            .map(|f| matches!(f.loss, Loss::Dropped))
            .unwrap_or(false);
        let has_settings_dropped = ir
            .fields
            .get("plugins.settings")
            .map(|f| matches!(f.loss, Loss::Dropped))
            .unwrap_or(false);
        let has_deps_dropped = ir
            .fields
            .get("plugins.dependencies")
            .map(|f| matches!(f.loss, Loss::Dropped))
            .unwrap_or(false);
        let has_user_config_dropped = ir
            .fields
            .get("plugins.userConfig")
            .map(|f| matches!(f.loss, Loss::Dropped))
            .unwrap_or(false);

        assert!(has_lsp_dropped, "lspServers should be dropped");
        assert!(has_output_dropped, "outputStyles should be dropped");
        assert!(has_channels_dropped, "channels should be dropped");
        assert!(has_settings_dropped, "settings should be dropped");
        assert!(has_deps_dropped, "dependencies should be dropped");
        assert!(has_user_config_dropped, "userConfig should be dropped");

        // userConfig に対する追加 warn が出るはず
        let has_user_config_warn = ir
            .diagnostics
            .iter()
            .any(|d| d.id.as_deref() == Some("plugins.userConfig") && d.level == DiagLevel::Warn);
        assert!(has_user_config_warn, "Expected userConfig warn diagnostic");
    }

    #[test]
    fn test_plugins_lift_c2x_with_recursion() {
        let dir = TempDir::new().unwrap();
        let plugin_json = create_claude_plugin_fixture(dir.path());

        let h = make_handler();
        let parsed = h.parse(&plugin_json).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        // skills/ と .mcp.json が子ノードとして再帰変換される
        let skill_children: Vec<_> = ir
            .children
            .iter()
            .filter(|c| c.kind == Kind::Skill)
            .collect();
        assert!(
            !skill_children.is_empty(),
            "Expected skill children from recursion"
        );

        let mcp_children: Vec<_> = ir.children.iter().filter(|c| c.kind == Kind::Mcp).collect();
        assert!(
            !mcp_children.is_empty(),
            "Expected MCP children from recursion"
        );
    }

    #[test]
    fn test_plugins_lower_c2x_generates_codex_manifest() {
        let dir = TempDir::new().unwrap();
        let plugin_json = create_claude_plugin_fixture(dir.path());

        let out_dir = dir.path().join("out");
        let opts = default_opts(out_dir.to_str().unwrap());

        let h = make_handler();
        let parsed = h.parse(&plugin_json).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // .codex-plugin/plugin.json が生成されているか確認
        let codex_manifest = plan
            .files
            .iter()
            .find(|f| f.path.contains(".codex-plugin") && f.path.ends_with("plugin.json"));
        assert!(
            codex_manifest.is_some(),
            "Expected .codex-plugin/plugin.json"
        );

        let content: Value = serde_json::from_str(&codex_manifest.unwrap().content).unwrap();
        assert_eq!(content["name"].as_str(), Some("test-plugin"));
        assert_eq!(content["version"].as_str(), Some("1.2.3"));
    }

    #[test]
    fn test_plugins_lower_c2x_dual_manifest() {
        let dir = TempDir::new().unwrap();
        let plugin_json = create_claude_plugin_fixture(dir.path());

        let out_dir = dir.path().join("out");
        let mut opts = default_opts(out_dir.to_str().unwrap());
        opts.dual_manifest = true;

        let h = make_handler();
        let parsed = h.parse(&plugin_json).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // .claude-plugin/plugin.json と .codex-plugin/plugin.json の両方が生成されているか確認
        let has_claude = plan
            .files
            .iter()
            .any(|f| f.path.contains(".claude-plugin") && f.path.ends_with("plugin.json"));
        let has_codex = plan
            .files
            .iter()
            .any(|f| f.path.contains(".codex-plugin") && f.path.ends_with("plugin.json"));
        assert!(
            has_claude,
            "Expected .claude-plugin/plugin.json with dual-manifest"
        );
        assert!(
            has_codex,
            "Expected .codex-plugin/plugin.json with dual-manifest"
        );
    }

    #[test]
    fn test_plugins_c2x_version_semver_completion() {
        let dir = TempDir::new().unwrap();
        let plugin_dir = dir.path().join(".claude-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();
        let plugin_json = plugin_dir.join("plugin.json");
        // version が省略されている場合
        fs::write(
            &plugin_json,
            r#"{"name": "test-plugin", "description": "A test plugin"}"#,
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        let opts = default_opts(out_dir.to_str().unwrap());

        let h = make_handler();
        let parsed = h.parse(&plugin_json).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // version 補完の warn が出るはず
        let has_version_warn = plan
            .diagnostics
            .iter()
            .any(|d| d.id.as_deref() == Some("plugins.version") || d.message.contains("version"));
        assert!(
            has_version_warn,
            "Expected version semver completion warning"
        );

        // 生成された manifest の version が "0.0.0" になっているはず
        let codex_manifest = plan
            .files
            .iter()
            .find(|f| f.path.contains(".codex-plugin") && f.path.ends_with("plugin.json"))
            .unwrap();
        let content: Value = serde_json::from_str(&codex_manifest.content).unwrap();
        assert_eq!(content["version"].as_str(), Some("0.0.0"));
    }

    #[test]
    fn test_plugins_c2x_marketplace_policy_defaults() {
        let dir = TempDir::new().unwrap();
        let plugin_dir = dir.path().join(".claude-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();

        // plugin.json
        fs::write(
            plugin_dir.join("plugin.json"),
            r#"{"name": "test-plugin", "version": "1.0.0", "description": "Test"}"#,
        )
        .unwrap();

        // marketplace.json without policy
        fs::write(
            plugin_dir.join("marketplace.json"),
            r#"{
  "plugins": [
    {
      "name": "test-plugin",
      "source": "./",
      "category": "productivity"
    }
  ]
}"#,
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        let opts = default_opts(out_dir.to_str().unwrap());

        let h = make_handler();
        let parsed = h.parse(&plugin_dir.join("plugin.json")).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // marketplace.json が出力されているか確認
        let marketplace_file = plan
            .files
            .iter()
            .find(|f| f.path.contains("marketplace.json"));
        assert!(
            marketplace_file.is_some(),
            "Expected marketplace.json in output"
        );

        let content: Value = serde_json::from_str(&marketplace_file.unwrap().content).unwrap();
        let plugins = content["plugins"].as_array().unwrap();
        assert!(!plugins.is_empty());

        // policy が補完されているか確認
        let policy = &plugins[0]["policy"];
        assert!(policy.is_object(), "Expected policy object");
        assert_eq!(policy["installation"].as_str(), Some("AVAILABLE"));
        assert_eq!(policy["authentication"].as_str(), Some("ON_INSTALL"));

        // policy 補完の warn が出るはず
        let has_policy_warn = plan
            .diagnostics
            .iter()
            .any(|d| d.message.contains("policy"));
        assert!(has_policy_warn, "Expected policy auto-fill warning");
    }

    #[test]
    fn test_complete_semver() {
        assert_eq!(complete_semver("1"), "1.0.0");
        assert_eq!(complete_semver("1.2"), "1.2.0");
        assert_eq!(complete_semver("1.2.3"), "1.2.3");
        // git SHA
        let sha = "a".repeat(40);
        assert_eq!(complete_semver(&sha), "0.0.0");
    }
}
