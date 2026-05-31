use std::path::Path;

use anyhow::Context;
use serde_json::Value;

use crate::core::ir::{
    new_node, BodySegment, DegradeInfo, DiagLevel, Diagnostic, DroppedInfo, IRField, IRNode, Kind,
    Loss, SideArtifact, Tool,
};
use crate::core::mappings::{
    applies_direction, index_by_claude_field, index_by_codex_field, DomainMap, LossSpec,
};
use crate::core::transforms::{apply_transforms, ConvDir, TransformCtx};
use crate::degrade::rules::degrade_allowed_tools;
use crate::degrade::subagent::{decide_skill_target, degrade_to_subagent, SkillTarget};
use crate::handlers::{EmitFile, EmitPlan, Handler, LowerOpts};
use crate::scanner::body::{rewrite_body, scan_body};

/// skills ドメインのハンドラ。
pub struct SkillsHandler {
    pub map: DomainMap,
}

impl Handler for SkillsHandler {
    fn kind(&self) -> Kind {
        Kind::Skill
    }

    fn detect(&self, path: &Path) -> bool {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        name == "SKILL.md"
    }

    fn parse(&self, path: &Path) -> anyhow::Result<Value> {
        crate::core::serialize::frontmatter::parse_frontmatter_file(path)
    }

    fn lift(&self, parsed: &Value, dir: ConvDir) -> anyhow::Result<IRNode> {
        let source_tool = match dir {
            ConvDir::C2x => Tool::Claude,
            ConvDir::X2c => Tool::Codex,
        };
        let source_path = parsed["path"].as_str().unwrap_or("").to_string();
        let mut node = new_node(Kind::Skill, source_tool, &source_path);

        let idx = match dir {
            ConvDir::C2x => index_by_claude_field(&self.map),
            ConvDir::X2c => index_by_codex_field(&self.map),
        };

        let frontmatter = match parsed["frontmatter"].as_object() {
            Some(fm) => fm,
            None => {
                return Ok(node);
            }
        };

        for (key, value) in frontmatter {
            let Some(entry) = idx.get(key.as_str()) else {
                node.diagnostics.push(Diagnostic {
                    level: DiagLevel::Drop,
                    id: None,
                    message: format!("unknown frontmatter key: {key}"),
                });
                continue;
            };

            if !applies_direction(entry, dir) {
                continue;
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

            let degrade_info = entry.degrade.as_ref().map(|d| DegradeInfo {
                to: d.to.clone(),
                target: d.target.clone(),
            });

            let dropped_info = if matches!(loss, Loss::Dropped) {
                Some(DroppedInfo {
                    reason: entry
                        .notes
                        .clone()
                        .unwrap_or_else(|| format!("{} has no Codex equivalent", key)),
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

            node.fields.insert(
                entry.id.clone(),
                IRField {
                    id: entry.id.clone(),
                    value: v,
                    loss,
                    transforms_applied: applied,
                    degrade: degrade_info,
                    warning: warning.clone(),
                    dropped: dropped_info,
                },
            );

            if entry.warn == Some(true) {
                if let Some(msg) = &warning {
                    node.diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some(entry.id.clone()),
                        message: msg.clone(),
                    });
                }
            }
        }

        // x2c: openai.yaml があれば policy.allow_implicit_invocation を読み込む
        if dir == ConvDir::X2c {
            let openai_yaml = load_openai_yaml(&source_path);
            if let Some(allow_implicit) = openai_yaml {
                // allow_implicit_invocation=false means disable-model-invocation=true (polarity invert)
                let disable_val = Value::Bool(!allow_implicit);
                if let Some(entry) = self
                    .map
                    .entries
                    .iter()
                    .find(|e| e.id == "skills.disable-model-invocation")
                {
                    node.fields.insert(
                        "skills.disable-model-invocation".to_string(),
                        IRField {
                            id: "skills.disable-model-invocation".to_string(),
                            value: disable_val,
                            loss: Loss::Lossless,
                            transforms_applied: vec!["polarity:invert".to_string()],
                            degrade: None,
                            warning: None,
                            dropped: None,
                        },
                    );
                    let _ = entry;
                }
            }
        }

        // 本文スキャン
        let body_raw = parsed["body"].as_str().unwrap_or("").to_string();
        let findings = scan_body(&body_raw, dir);
        node.body = Some(BodySegment {
            raw: body_raw,
            findings,
        });

        Ok(node)
    }

    fn lower(&self, ir: &IRNode, dir: ConvDir, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        match dir {
            ConvDir::C2x => self.lower_c2x(ir, opts),
            ConvDir::X2c => self.lower_x2c(ir, opts),
        }
    }
}

impl SkillsHandler {
    fn lower_c2x(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let mut files = Vec::new();
        let mut diagnostics = Vec::new();
        let mut side_artifacts: Vec<SideArtifact> = Vec::new();

        // skill 名を source_path から抽出
        let skill_name = extract_skill_name(&ir.source_path);
        let out_root = opts.out.as_deref().unwrap_or(".");

        // frontmatter を構築
        let mut fm = serde_json::Map::new();

        // name
        if let Some(f) = ir.fields.get("skills.name") {
            fm.insert("name".to_string(), f.value.clone());
        }

        // description: skills.description + skills.when_to_use を連結
        let desc = ir
            .fields
            .get("skills.description")
            .and_then(|f| f.value.as_str())
            .unwrap_or("");
        let when_to_use = ir
            .fields
            .get("skills.when_to_use")
            .and_then(|f| f.value.as_str())
            .unwrap_or("");
        let combined_desc = if when_to_use.is_empty() {
            desc.to_string()
        } else if desc.is_empty() {
            when_to_use.to_string()
        } else {
            format!("{}\n\n{}", desc, when_to_use)
        };
        if !combined_desc.is_empty() {
            fm.insert("description".to_string(), Value::String(combined_desc));
            if !when_to_use.is_empty() {
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("skills.when_to_use".to_string()),
                    message: "when_to_use concatenated into description (lossy)".to_string(),
                });
            }
        }

        // determine skill target
        let target = decide_skill_target(ir, opts);

        // allowed-tools → degrade
        if let Some(f) = ir.fields.get("skills.allowed-tools") {
            let tools = json_to_string_list(&f.value);
            let (arts, diags) = degrade_allowed_tools(&skill_name, &tools, true);
            side_artifacts.extend(arts);
            diagnostics.extend(diags);
        }

        // disallowed-tools → degrade
        if let Some(f) = ir.fields.get("skills.disallowed-tools") {
            let tools = json_to_string_list(&f.value);
            let (arts, diags) = degrade_allowed_tools(&skill_name, &tools, false);
            side_artifacts.extend(arts);
            diagnostics.extend(diags);
        }

        // hooks → degrade
        if let Some(f) = ir.fields.get("skills.hooks") {
            let (arts, diags) = crate::degrade::hooks_scope::degrade_skill_hooks(
                &skill_name,
                &f.value,
                &opts.hooks_target,
            );
            side_artifacts.extend(arts);
            diagnostics.extend(diags);
        }

        // disable-model-invocation → SideArtifact: agents/openai.yaml
        // polarity:invert was applied in lift: disable-model-invocation=true (Claude) → IR holds false
        // because invert(true)==false means allow_implicit_invocation=false in openai.yaml
        if let Some(f) = ir.fields.get("skills.disable-model-invocation") {
            if f.value == Value::Bool(false) {
                let openai_yaml_path = format!(
                    "{}/.agents/skills/{}/agents/openai.yaml",
                    out_root, skill_name
                );
                let content = "policy:\n  allow_implicit_invocation: false\n".to_string();
                side_artifacts.push(SideArtifact {
                    path: format!(".agents/skills/{}/agents/openai.yaml", skill_name),
                    content,
                    note: "disable-model-invocation=true → policy.allow_implicit_invocation: false"
                        .to_string(),
                });
                let _ = openai_yaml_path;
            }
        }

        // model/effort/context:fork → subagent degrade
        let has_model = ir.fields.contains_key("skills.model");
        let has_effort = ir.fields.contains_key("skills.effort");
        let has_fork = ir.fields.contains_key("skills.context-fork");

        if matches!(target, SkillTarget::Subagent) && (has_model || has_effort || has_fork) {
            let (arts, diags) = degrade_to_subagent(&skill_name, ir);
            side_artifacts.extend(arts);
            diagnostics.extend(diags);
        }

        // paths/user-invocable/arguments/argument-hint → dropped (already handled in lift)
        // shell: powershell → propose only (warn)
        if let Some(f) = ir.fields.get("skills.shell") {
            if f.value.as_str() == Some("powershell") {
                diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("skills.shell".to_string()),
                    message: "shell: powershell – propose mapping to hooks.commandWindows (manual action required)".to_string(),
                });
            }
        }

        // 本文
        let body_raw = ir.body.as_ref().map(|b| b.raw.as_str()).unwrap_or("");
        let body_out = if opts.rewrite_body {
            if let Some(body_seg) = &ir.body {
                rewrite_body(body_raw, &body_seg.findings)
            } else {
                body_raw.to_string()
            }
        } else {
            body_raw.to_string()
        };

        // 出力 SKILL.md
        let skill_md_path = format!("{}/.agents/skills/{}/SKILL.md", out_root, skill_name);

        // frontmatter → YAML 文字列
        let fm_yaml = if fm.is_empty() {
            String::new()
        } else {
            let yaml_val = Value::Object(fm);
            serde_saphyr::to_string(&yaml_val)
                .with_context(|| "Failed to serialize frontmatter as YAML")?
        };

        let skill_md_content = if fm_yaml.is_empty() {
            body_out.clone()
        } else {
            format!("---\n{}---\n{}", fm_yaml, body_out)
        };

        files.push(EmitFile {
            path: skill_md_path,
            content: skill_md_content,
        });

        // SideArtifacts → EmitFiles
        for art in &side_artifacts {
            files.push(EmitFile {
                path: format!("{}/{}", out_root, art.path),
                content: art.content.clone(),
            });
        }

        Ok(EmitPlan { files, diagnostics })
    }

    fn lower_x2c(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let mut files = Vec::new();
        let diagnostics = Vec::new();

        let skill_name = extract_skill_name(&ir.source_path);
        let out_root = opts.out.as_deref().unwrap_or(".");

        let idx = index_by_codex_field(&self.map);

        let mut fm = serde_json::Map::new();

        // Codex フィールド → Claude フィールドへ変換
        for (key, value) in &ir.fields {
            // key は entry.id。codex フィールド名を探す
            let Some(entry) = self.map.entries.iter().find(|e| e.id == *key) else {
                continue;
            };
            if !applies_direction(entry, ConvDir::X2c) {
                continue;
            }
            // Claudeフィールド名を取得
            let claude_field = entry
                .claude
                .as_ref()
                .and_then(|c| c.field.as_ref())
                .map(|s| s.as_str());
            let Some(cf) = claude_field else {
                continue;
            };
            // pseudo field skips
            if cf.starts_with('\u{FF08}') {
                continue;
            }
            fm.insert(cf.to_string(), value.value.clone());
        }
        let _ = idx;

        // 本文
        let body_raw = ir.body.as_ref().map(|b| b.raw.as_str()).unwrap_or("");
        let body_out = if opts.rewrite_body {
            if let Some(body_seg) = &ir.body {
                rewrite_body(body_raw, &body_seg.findings)
            } else {
                body_raw.to_string()
            }
        } else {
            body_raw.to_string()
        };

        let skill_md_path = format!("{}/.claude/skills/{}/SKILL.md", out_root, skill_name);

        let fm_yaml = if fm.is_empty() {
            String::new()
        } else {
            let yaml_val = Value::Object(fm);
            serde_saphyr::to_string(&yaml_val)
                .with_context(|| "Failed to serialize frontmatter as YAML")?
        };

        let skill_md_content = if fm_yaml.is_empty() {
            body_out
        } else {
            format!("---\n{}---\n{}", fm_yaml, body_out)
        };

        files.push(EmitFile {
            path: skill_md_path,
            content: skill_md_content,
        });

        Ok(EmitPlan { files, diagnostics })
    }
}

/// source_path からスキル名を抽出するヘルパ。
/// .claude/skills/<name>/SKILL.md → <name>
/// .agents/skills/<name>/SKILL.md → <name>
/// それ以外 → "unknown"
fn extract_skill_name(source_path: &str) -> String {
    let path = Path::new(source_path);
    // SKILL.md の親ディレクトリ名を返す
    if let Some(parent) = path.parent() {
        if let Some(name) = parent.file_name() {
            let n = name.to_str().unwrap_or("unknown");
            if n != "skills" && n != ".claude" && n != ".agents" {
                return n.to_string();
            }
        }
    }
    // フォールバック: skill という文字列
    "skill".to_string()
}

/// agents/openai.yaml から policy.allow_implicit_invocation を読み込む。
/// source_path は SKILL.md の絶対パス。
/// serde-saphyr を使って YAML をパースする。
fn load_openai_yaml(source_path: &str) -> Option<bool> {
    let skill_dir = Path::new(source_path).parent()?;
    let openai_yaml = skill_dir.join("agents").join("openai.yaml");
    if !openai_yaml.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&openai_yaml).ok()?;
    // serde-saphyr で Value にパース
    let parsed: serde_json::Value = serde_saphyr::from_str(&content).ok()?;
    parsed["policy"]["allow_implicit_invocation"].as_bool()
}

/// JSON Value を文字列のリストに変換するヘルパ。
/// Value::String → [string]
/// Value::Array → 各要素の as_str()
fn json_to_string_list(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Array(arr) => arr
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect(),
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mappings::load_mappings;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn make_handler() -> SkillsHandler {
        let maps = load_mappings(Path::new("mappings"));
        SkillsHandler {
            map: maps["skills"].clone(),
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
    fn test_skills_detect() {
        let h = make_handler();
        assert!(h.detect(Path::new("SKILL.md")));
        assert!(!h.detect(Path::new(".mcp.json")));
        assert!(!h.detect(Path::new("README.md")));
    }

    #[test]
    fn test_skills_lift_c2x_basic() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".claude").join("skills").join("deploy");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: deploy\ndescription: Deploy the app\n---\n\nRun deployment steps.\n",
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        assert_eq!(ir.kind, Kind::Skill);
        assert!(ir.fields.contains_key("skills.name"));
        assert!(ir.fields.contains_key("skills.description"));
        let name_field = &ir.fields["skills.name"];
        assert_eq!(name_field.value, Value::String("deploy".to_string()));
        assert_eq!(name_field.loss, Loss::Lossless);
    }

    #[test]
    fn test_skills_lift_c2x_dropped_user_invocable() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".claude").join("skills").join("test-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: test-skill\ndescription: Test\nuser-invocable: true\n---\nBody.\n",
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        // user-invocable は dropped
        let f = ir.fields.get("skills.user-invocable").unwrap();
        assert_eq!(f.loss, Loss::Dropped);
    }

    #[test]
    fn test_skills_lower_c2x_basic() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".claude").join("skills").join("deploy");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: deploy\ndescription: Deploy the app\n---\n\nRun deployment steps.\n",
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        let mut opts = default_opts();
        opts.out = Some(out_dir.to_str().unwrap().to_string());

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // 出力ファイルが生成されているか確認
        let has_skill_md = plan.files.iter().any(|f| f.path.ends_with("SKILL.md"));
        assert!(has_skill_md, "Expected SKILL.md in emit plan");

        let skill_file = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("SKILL.md"))
            .unwrap();
        assert!(skill_file.content.contains("deploy"));
    }

    #[test]
    fn test_skills_lower_c2x_with_allowed_tools() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".claude").join("skills").join("build");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: build\ndescription: Build the project\nallowed-tools:\n  - \"Bash(cargo build)\"\n---\nBuild.\n",
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        let mut opts = default_opts();
        opts.out = Some(out_dir.to_str().unwrap().to_string());

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // .rules ファイルが生成されているか確認
        let has_rules = plan.files.iter().any(|f| f.path.ends_with(".rules"));
        assert!(has_rules, "Expected .rules file for Bash tool degrade");
    }

    #[test]
    fn test_skills_lower_c2x_when_to_use_concat() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".claude").join("skills").join("analyze");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: analyze\ndescription: Analyze code\nwhen_to_use: Use this when you need analysis\n---\nAnalyze.\n",
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        let mut opts = default_opts();
        opts.out = Some(out_dir.to_str().unwrap().to_string());

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        let skill_file = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("SKILL.md"))
            .unwrap();

        // description に when_to_use が連結されているか確認
        assert!(skill_file.content.contains("Analyze code"));
        assert!(skill_file
            .content
            .contains("Use this when you need analysis"));
    }

    #[test]
    fn test_extract_skill_name() {
        assert_eq!(
            extract_skill_name("/home/user/.claude/skills/deploy/SKILL.md"),
            "deploy"
        );
        assert_eq!(extract_skill_name(".agents/skills/build/SKILL.md"), "build");
    }

    #[test]
    fn test_skills_lift_c2x_model_degrade() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".claude").join("skills").join("heavy");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: heavy\ndescription: Heavy task\nmodel: claude-opus-4-8\neffort: max\n---\nDo heavy work.\n",
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        // model / effort → degrade info
        let model_f = ir.fields.get("skills.model").unwrap();
        assert_eq!(model_f.loss, Loss::Lossy);
        assert!(model_f.degrade.is_some());

        let effort_f = ir.fields.get("skills.effort").unwrap();
        assert_eq!(effort_f.loss, Loss::Lossy);
        assert!(effort_f.degrade.is_some());
    }

    #[test]
    fn test_skills_lower_c2x_subagent_degrade() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join(".claude").join("skills").join("heavy");
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(
            &path,
            "---\nname: heavy\ndescription: Heavy task\nmodel: claude-opus-4-8\neffort: max\n---\nDo heavy work.\n",
        )
        .unwrap();

        let out_dir = dir.path().join("out");
        // skill_target=Subagent for test
        let opts = LowerOpts {
            out: Some(out_dir.to_str().unwrap().to_string()),
            scope: crate::handlers::Scope::Project,
            dual_manifest: false,
            hooks_target: crate::handlers::Scope::User,
            skill_target: crate::handlers::SkillTargetMode::Subagent,
            interactive: false,
            rewrite_body: false,
        };

        let h = make_handler();
        let parsed = h.parse(&path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let plan = h.lower(&ir, ConvDir::C2x, &opts).unwrap();

        // .codex/agents/<skill>.toml が生成されているか確認
        let has_agent_toml = plan
            .files
            .iter()
            .any(|f| f.path.contains(".codex/agents/") && f.path.ends_with(".toml"));
        assert!(has_agent_toml, "Expected subagent TOML file");
    }
}
