use std::path::Path;

use serde_json::Value;
use toml_edit::{Array, DocumentMut, Item, Table};

use crate::core::ir::{
    new_node, DiagLevel, Diagnostic, DroppedInfo, IRField, IRNode, Kind, Loss, Tool,
};
use crate::core::mappings::DomainMap;
use crate::core::transforms::ConvDir;
use crate::handlers::{EmitFile, EmitPlan, Handler, LowerOpts, Scope};

/// 共通 10 イベント（both / lossless）のリスト。
const COMMON_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PreCompact",
    "PostCompact",
    "SubagentStart",
    "SubagentStop",
    "Stop",
];

/// Claude 固有イベント（claude_to_codex / dropped）のリスト。
const CLAUDE_ONLY_EVENTS: &[&str] = &[
    "Setup",
    "UserPromptExpansion",
    "PermissionDenied",
    "PostToolUseFailure",
    "PostToolBatch",
    "Notification",
    "MessageDisplay",
    "TaskCreated",
    "TaskCompleted",
    "StopFailure",
    "TeammateIdle",
    "InstructionsLoaded",
    "ConfigChange",
    "CwdChanged",
    "FileChanged",
    "WorktreeCreate",
    "WorktreeRemove",
    "Elicitation",
    "ElicitationResult",
    "SessionEnd",
];

/// c2x で dropped される hook フィールド（args/shell/if/once/asyncRewake）。
const DROPPED_C2X_HOOK_FIELDS: &[&str] = &["args", "shell", "if", "once", "asyncRewake"];

/// hooks ドメインのハンドラ。
pub struct HooksHandler {
    pub map: DomainMap,
}

impl Handler for HooksHandler {
    fn kind(&self) -> Kind {
        Kind::Hooks
    }

    fn detect(&self, path: &Path) -> bool {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        // hooks.json (Claude) または hooks セクション含む config.toml (Codex)
        name == "hooks.json" || name == "settings.json"
    }

    fn parse(&self, path: &Path) -> anyhow::Result<Value> {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.ends_with(".toml") {
            parse_codex_hooks_toml(path)
        } else {
            crate::core::serialize::json::parse_json_file(path)
        }
    }

    fn lift(&self, parsed: &Value, dir: ConvDir) -> anyhow::Result<IRNode> {
        match dir {
            ConvDir::C2x => self.lift_c2x(parsed),
            ConvDir::X2c => self.lift_x2c(parsed),
        }
    }

    fn lower(&self, ir: &IRNode, dir: ConvDir, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        match dir {
            ConvDir::C2x => self.lower_c2x(ir, opts),
            ConvDir::X2c => self.lower_x2c(ir, opts),
        }
    }
}

impl HooksHandler {
    /// Claude JSON hooks → IR（c2x）
    fn lift_c2x(&self, parsed: &Value) -> anyhow::Result<IRNode> {
        let source_path = parsed["path"].as_str().unwrap_or("").to_string();
        let mut node = new_node(Kind::Hooks, Tool::Claude, &source_path);

        // parsed は hooks.json の raw JSON または settings.json の "hooks" キー以下
        // 形式: {"hooks": {"EventName": [{matcher, hooks:[{type,...}]}]}}
        // あるいは直接 {"EventName": [...]}
        let hooks_obj = if let Some(h) = parsed.get("hooks").and_then(|v| v.as_object()) {
            h.clone()
        } else if let Some(fm) = parsed.get("frontmatter").and_then(|v| v.as_object()) {
            if let Some(h) = fm.get("hooks").and_then(|v| v.as_object()) {
                h.clone()
            } else {
                // frontmatter 全体を hooks として扱う
                fm.clone()
            }
        } else if let Some(obj) = parsed.as_object() {
            obj.clone()
        } else {
            return Ok(node);
        };

        for (event_name, event_entries) in &hooks_obj {
            // meta フィールド（path 等）はスキップ
            if event_name == "path" || event_name == "body" || event_name == "frontmatter" {
                continue;
            }

            if CLAUDE_ONLY_EVENTS.contains(&event_name.as_str()) {
                // dropped
                node.fields.insert(
                    format!("hooks.event.{}", event_name),
                    IRField {
                        id: format!("hooks.event.{}", event_name),
                        value: event_entries.clone(),
                        loss: Loss::Dropped,
                        transforms_applied: vec![],
                        degrade: None,
                        warning: Some(format!(
                            "Event '{}' is Claude-specific and has no Codex equivalent (dropped)",
                            event_name
                        )),
                        dropped: Some(DroppedInfo {
                            reason: format!("Claude-only event: {}", event_name),
                        }),
                    },
                );
                node.diagnostics.push(Diagnostic {
                    level: DiagLevel::Drop,
                    id: Some(format!("hooks.event.{}", event_name)),
                    message: format!(
                        "Event '{}' is Claude-specific and will be dropped in c2x conversion",
                        event_name
                    ),
                });
                continue;
            }

            if COMMON_EVENTS.contains(&event_name.as_str()) {
                // Process each hook entry in the event array
                let processed = process_hook_entries_c2x(event_name, event_entries, &mut node);
                node.fields.insert(
                    format!("hooks.event.{}", event_name),
                    IRField {
                        id: format!("hooks.event.{}", event_name),
                        value: processed,
                        loss: Loss::Lossless,
                        transforms_applied: vec!["format:json_to_toml".to_string()],
                        degrade: None,
                        warning: None,
                        dropped: None,
                    },
                );
            } else {
                // unknown event → dropped
                node.fields.insert(
                    format!("hooks.event.{}", event_name),
                    IRField {
                        id: format!("hooks.event.{}", event_name),
                        value: event_entries.clone(),
                        loss: Loss::Dropped,
                        transforms_applied: vec![],
                        degrade: None,
                        warning: Some(format!("Unknown event '{}' dropped", event_name)),
                        dropped: Some(DroppedInfo {
                            reason: format!("Unknown event: {}", event_name),
                        }),
                    },
                );
                node.diagnostics.push(Diagnostic {
                    level: DiagLevel::Drop,
                    id: None,
                    message: format!("Unknown hook event '{}' dropped", event_name),
                });
            }
        }

        Ok(node)
    }

    /// Codex TOML hooks → IR（x2c）
    fn lift_x2c(&self, parsed: &Value) -> anyhow::Result<IRNode> {
        let source_path = parsed["path"].as_str().unwrap_or("").to_string();
        let mut node = new_node(Kind::Hooks, Tool::Codex, &source_path);

        // parsed はすでに {"hooks": {"EventName": [...]}} 構造
        let hooks_obj = if let Some(h) = parsed.get("hooks").and_then(|v| v.as_object()) {
            h.clone()
        } else if let Some(obj) = parsed.as_object() {
            obj.clone()
        } else {
            return Ok(node);
        };

        for (event_name, event_entries) in &hooks_obj {
            if event_name == "path" || event_name == "body" || event_name == "frontmatter" {
                continue;
            }

            if COMMON_EVENTS.contains(&event_name.as_str()) {
                let processed = process_hook_entries_x2c(event_name, event_entries, &mut node);
                node.fields.insert(
                    format!("hooks.event.{}", event_name),
                    IRField {
                        id: format!("hooks.event.{}", event_name),
                        value: processed,
                        loss: Loss::Lossless,
                        transforms_applied: vec!["format:toml_to_json".to_string()],
                        degrade: None,
                        warning: None,
                        dropped: None,
                    },
                );
            } else {
                // Codex-only or unknown event dropped
                node.fields.insert(
                    format!("hooks.event.{}", event_name),
                    IRField {
                        id: format!("hooks.event.{}", event_name),
                        value: event_entries.clone(),
                        loss: Loss::Dropped,
                        transforms_applied: vec![],
                        degrade: None,
                        warning: Some(format!("Event '{}' not in common set, dropped", event_name)),
                        dropped: Some(DroppedInfo {
                            reason: format!("Non-common event: {}", event_name),
                        }),
                    },
                );
            }
        }

        Ok(node)
    }

    /// c2x: IR → Codex TOML hooks
    fn lower_c2x(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let mut diagnostics = ir.diagnostics.clone();
        let out_root = opts.out.as_deref().unwrap_or(".");

        // #16430 warn: plugin-bundled hooks are not loaded by Codex
        diagnostics.push(Diagnostic {
            level: DiagLevel::Warn,
            id: Some("hooks.plugin_bundled".to_string()),
            message: "Warning (#16430): Plugin-bundled hooks are not loaded by Codex. \
                      Use --hooks-target=user|project to write hooks to ~/.codex/hooks.json \
                      or .codex/config.toml [hooks] instead."
                .to_string(),
        });

        // Collect common event hooks
        let mut hooks_entries: Vec<(String, Value)> = Vec::new();
        for (id, field) in &ir.fields {
            if field.loss == Loss::Dropped {
                continue;
            }
            if let Some(event_name) = id.strip_prefix("hooks.event.") {
                if COMMON_EVENTS.contains(&event_name) {
                    hooks_entries.push((event_name.to_string(), field.value.clone()));
                }
            }
        }

        let files = match opts.hooks_target {
            Scope::User => {
                // Write to ~/.codex/hooks.json (JSON format)
                let mut hooks_json = serde_json::Map::new();
                for (event_name, entries) in &hooks_entries {
                    hooks_json.insert(event_name.clone(), entries.clone());
                }
                let content = serde_json::to_string_pretty(&Value::Object(hooks_json))
                    .unwrap_or_else(|_| "{}".to_string());
                vec![EmitFile {
                    path: format!("{}/hooks.json", out_root),
                    content,
                }]
            }
            Scope::Project => {
                // Write to .codex/config.toml [hooks] section (TOML format)
                let toml_str = build_hooks_toml(&hooks_entries)?;
                vec![EmitFile {
                    path: format!("{}/.codex/config.toml", out_root),
                    content: toml_str,
                }]
            }
        };

        Ok(EmitPlan { files, diagnostics })
    }

    /// x2c: IR → Claude JSON hooks
    fn lower_x2c(&self, ir: &IRNode, opts: &LowerOpts) -> anyhow::Result<EmitPlan> {
        let diagnostics = ir.diagnostics.clone();
        let out_root = opts.out.as_deref().unwrap_or(".");

        let mut hooks_obj = serde_json::Map::new();
        for (id, field) in &ir.fields {
            if field.loss == Loss::Dropped {
                continue;
            }
            if let Some(event_name) = id.strip_prefix("hooks.event.") {
                if COMMON_EVENTS.contains(&event_name) {
                    hooks_obj.insert(event_name.to_string(), field.value.clone());
                }
            }
        }

        let hooks_wrapper = serde_json::json!({ "hooks": hooks_obj });
        let content =
            serde_json::to_string_pretty(&hooks_wrapper).unwrap_or_else(|_| "{}".to_string());

        let files = vec![EmitFile {
            path: format!("{}/hooks.json", out_root),
            content,
        }];

        Ok(EmitPlan { files, diagnostics })
    }
}

/// hooks の各エントリを c2x 方向で処理する（matcher 正規化、dropped フィールドの除外）。
/// 副作用として node.diagnostics に警告を追加する。
fn process_hook_entries_c2x(event_name: &str, entries: &Value, node: &mut IRNode) -> Value {
    let arr = match entries.as_array() {
        Some(a) => a,
        None => return entries.clone(),
    };

    let processed: Vec<Value> = arr
        .iter()
        .filter_map(|entry| {
            let obj = entry.as_object()?;
            let mut new_obj = serde_json::Map::new();

            // matcher 正規化
            if let Some(matcher) = obj.get("matcher").and_then(|v| v.as_str()) {
                let (normalized, lossy) = normalize_matcher_c2x(matcher);
                new_obj.insert("matcher".to_string(), Value::String(normalized.clone()));
                if lossy {
                    node.diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some("hooks.matcher.exact".to_string()),
                        message: format!(
                            "Event '{}' matcher '{}' normalized to '{}' (lossy: Codex uses regex evaluation)",
                            event_name, matcher, normalized
                        ),
                    });
                } else {
                    // regex passthrough: Codex evaluates matchers as regexes; preserve and warn
                    node.diagnostics.push(Diagnostic {
                        level: DiagLevel::Warn,
                        id: Some("hooks.matcher.regex".to_string()),
                        message: format!(
                            "Event '{}' matcher '{}' passed through as-is (contains regex characters; Codex evaluates it as a regex)",
                            event_name, matcher
                        ),
                    });
                }
            }

            // hooks 配列
            if let Some(hooks_arr) = obj.get("hooks").and_then(|v| v.as_array()) {
                let processed_hooks: Vec<Value> = hooks_arr
                    .iter()
                    .filter_map(|h| process_single_hook_c2x(h, event_name, node))
                    .collect();
                new_obj.insert("hooks".to_string(), Value::Array(processed_hooks));
            }

            Some(Value::Object(new_obj))
        })
        .collect();

    Value::Array(processed)
}

/// hooks の各エントリを x2c 方向で処理する（commandWindows → shell 変換等）。
fn process_hook_entries_x2c(_event_name: &str, entries: &Value, node: &mut IRNode) -> Value {
    let arr = match entries.as_array() {
        Some(a) => a,
        None => return entries.clone(),
    };

    let processed: Vec<Value> = arr
        .iter()
        .filter_map(|entry| {
            let obj = entry.as_object()?;
            let mut new_obj = serde_json::Map::new();

            // matcher: Codex は常に regex なので そのまま転写
            if let Some(matcher) = obj.get("matcher") {
                new_obj.insert("matcher".to_string(), matcher.clone());
            }

            // hooks 配列
            if let Some(hooks_arr) = obj.get("hooks").and_then(|v| v.as_array()) {
                let processed_hooks: Vec<Value> = hooks_arr
                    .iter()
                    .filter_map(|h| process_single_hook_x2c(h, node))
                    .collect();
                new_obj.insert("hooks".to_string(), Value::Array(processed_hooks));
            }

            Some(Value::Object(new_obj))
        })
        .collect();

    Value::Array(processed)
}

/// 単一 hook エントリを c2x 変換する。
/// dropped フィールド（args/shell/if/once/asyncRewake）を除外し、
/// args がある場合は command に合成、http/mcp_tool タイプは None（除外）を返す。
fn process_single_hook_c2x(hook: &Value, event_name: &str, node: &mut IRNode) -> Option<Value> {
    let obj = hook.as_object()?;
    let hook_type = obj
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("command");

    // http/mcp_tool タイプは dropped
    if hook_type == "http" || hook_type == "mcp_tool" {
        node.diagnostics.push(Diagnostic {
            level: DiagLevel::Drop,
            id: Some(format!("hooks.type.{}", hook_type)),
            message: format!(
                "Event '{}': hook type '{}' has no Codex equivalent (dropped)",
                event_name, hook_type
            ),
        });
        return None;
    }

    // Codex は prompt/agent タイプを parse するが実行しないため dropped にする
    if hook_type == "prompt" || hook_type == "agent" {
        node.diagnostics.push(Diagnostic {
            level: DiagLevel::Drop,
            id: Some(format!("hooks.type.{}", hook_type)),
            message: format!(
                "Event '{}': hook type '{}' has no Codex equivalent (dropped; loss:dropped per mappings)",
                event_name, hook_type
            ),
        });
        return None;
    }

    let mut new_obj = serde_json::Map::new();

    // type をそのまま転写
    new_obj.insert("type".to_string(), Value::String(hook_type.to_string()));

    // command フィールドの処理（args がある場合は合成）
    let command = obj.get("command").and_then(|v| v.as_str());
    let args = obj.get("args").and_then(|v| v.as_array());

    if let Some(args_arr) = args {
        // args は c2x で dropped だが、command に合成する
        let synthesized = synthesize_command(command, args_arr);
        new_obj.insert("command".to_string(), Value::String(synthesized));
        node.diagnostics.push(Diagnostic {
            level: DiagLevel::Warn,
            id: Some("hooks.command.args".to_string()),
            message: format!(
                "Event '{}': 'args' field dropped (synthesized into 'command' with shell escaping)",
                event_name
            ),
        });
    } else if let Some(cmd) = command {
        new_obj.insert("command".to_string(), Value::String(cmd.to_string()));
    }

    // timeout, statusMessage, async → lossless
    for field_name in &["timeout", "statusMessage", "async"] {
        if let Some(v) = obj.get(*field_name) {
            new_obj.insert(field_name.to_string(), v.clone());
        }
    }

    // dropped フィールドの警告
    for dropped_field in DROPPED_C2X_HOOK_FIELDS {
        if obj.contains_key(*dropped_field) && *dropped_field != "args" {
            // args はすでに処理済み
            node.diagnostics.push(Diagnostic {
                level: DiagLevel::Drop,
                id: Some(format!("hooks.command.{}", dropped_field)),
                message: format!(
                    "Event '{}': hook field '{}' has no Codex equivalent (dropped)",
                    event_name, dropped_field
                ),
            });
        }
    }

    Some(Value::Object(new_obj))
}

/// 単一 hook エントリを x2c 変換する（commandWindows → Claude shell:powershell warn）。
fn process_single_hook_x2c(hook: &Value, node: &mut IRNode) -> Option<Value> {
    let obj = hook.as_object()?;
    let mut new_obj = serde_json::Map::new();

    for (k, v) in obj {
        match k.as_str() {
            "commandWindows" | "command_windows" => {
                // x2c: commandWindows は Claude 側に直接対応なし → warn
                node.diagnostics.push(Diagnostic {
                    level: DiagLevel::Warn,
                    id: Some("hooks.command.commandWindows".to_string()),
                    message:
                        "commandWindows has no direct Claude equivalent; consider shell:powershell"
                            .to_string(),
                });
                // 出力に含めない（lossy）
            }
            _ => {
                new_obj.insert(k.clone(), v.clone());
            }
        }
    }

    Some(Value::Object(new_obj))
}

/// matcher 正規化（c2x）。
/// - exact（英数字・_・| のみ）→ "^Bash$" / "^(Edit|Write)$"
/// - wildcard（"*" または ""）→ "" (全マッチ)
/// - regex 的文字を含む → そのまま（warn）
///
/// Returns (normalized_matcher, is_lossy)
fn normalize_matcher_c2x(matcher: &str) -> (String, bool) {
    if matcher.is_empty() || matcher == "*" {
        // wildcard → "" (全マッチ)
        return ("".to_string(), true);
    }

    // 英数字・_・| のみかチェック
    let is_exact = matcher
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '|');

    if is_exact {
        // exact or alternation
        if matcher.contains('|') {
            // alternation → "^(Edit|Write)$"
            (format!("^({})$", matcher), true)
        } else {
            // single exact → "^Bash$"
            (format!("^{}$", matcher), true)
        }
    } else {
        // regex 的文字を含む → そのまま（warn）
        (matcher.to_string(), false)
    }
}

/// command + args 配列を shell form に合成する（shlex::quote でエスケープ）。
fn synthesize_command(command: Option<&str>, args: &[Value]) -> String {
    let mut parts: Vec<String> = Vec::new();

    if let Some(cmd) = command {
        parts.push(cmd.to_string());
    }

    for arg in args {
        if let Some(s) = arg.as_str() {
            // shlex::try_quote でシェル特殊文字をエスケープ
            let quoted =
                shlex::try_quote(s).unwrap_or_else(|_| std::borrow::Cow::Owned(format!("'{}'", s)));
            parts.push(quoted.to_string());
        }
    }

    parts.join(" ")
}

/// hooks エントリを Codex TOML の [[hooks.EventName]] 形式に変換する。
fn build_hooks_toml(hooks_entries: &[(String, Value)]) -> anyhow::Result<String> {
    let mut doc = DocumentMut::new();

    // [hooks] テーブルを構築
    let hooks_item = doc.entry("hooks").or_insert(Item::Table(Table::new()));
    let hooks_tbl = hooks_item
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("[hooks] is not a table"))?;

    for (event_name, entries) in hooks_entries {
        let arr = match entries.as_array() {
            Some(a) => a,
            None => continue,
        };

        // [[hooks.EventName]] array-of-tables
        let aot_item = hooks_tbl
            .entry(event_name)
            .or_insert(Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
        let aot = aot_item
            .as_array_of_tables_mut()
            .ok_or_else(|| anyhow::anyhow!("hooks.{} is not array-of-tables", event_name))?;

        for entry_val in arr {
            let entry_obj = match entry_val.as_object() {
                Some(o) => o,
                None => continue,
            };
            let mut tbl = Table::new();

            // matcher
            if let Some(m) = entry_obj.get("matcher").and_then(|v| v.as_str()) {
                tbl.insert("matcher", toml_edit::value(m));
            }

            // hooks array-of-tables inside the entry
            if let Some(hooks_arr) = entry_obj.get("hooks").and_then(|v| v.as_array()) {
                let mut inner_aot = toml_edit::ArrayOfTables::new();
                for h in hooks_arr {
                    let h_obj = match h.as_object() {
                        Some(o) => o,
                        None => continue,
                    };
                    let mut h_tbl = Table::new();
                    for (k, v) in h_obj {
                        json_value_to_toml_item(v).map(|item| h_tbl.insert(k, item));
                    }
                    inner_aot.push(h_tbl);
                }
                tbl.insert("hooks", Item::ArrayOfTables(inner_aot));
            }

            aot.push(tbl);
        }
    }

    Ok(doc.to_string())
}

/// JSON Value を toml_edit Item に変換する（ベストエフォート）。
fn json_value_to_toml_item(v: &Value) -> Option<Item> {
    match v {
        Value::String(s) => Some(toml_edit::value(s.as_str())),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(toml_edit::value(i))
            } else {
                n.as_f64().map(toml_edit::value)
            }
        }
        Value::Bool(b) => Some(toml_edit::value(*b)),
        Value::Array(arr) => {
            let mut toml_arr = Array::new();
            for item in arr {
                match item {
                    Value::String(s) => toml_arr.push(s.as_str()),
                    Value::Number(n) => {
                        if let Some(i) = n.as_i64() {
                            toml_arr.push(i);
                        }
                    }
                    Value::Bool(b) => toml_arr.push(*b),
                    _ => {}
                }
            }
            Some(toml_edit::value(toml_arr))
        }
        Value::Null => None,
        Value::Object(_) => None, // nested objects not supported for simple values
    }
}

/// Codex config.toml の [hooks] セクションを読み込み、JSON Value として返す。
/// 形式: {"path": "...", "hooks": {"EventName": [{matcher, hooks:[{type,...}]}]}}
fn parse_codex_hooks_toml(path: &Path) -> anyhow::Result<Value> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", path.display(), e))?;
    let doc: DocumentMut = content
        .parse()
        .map_err(|e| anyhow::anyhow!("Failed to parse TOML {}: {}", path.display(), e))?;

    let mut hooks_map = serde_json::Map::new();

    if let Some(hooks_item) = doc.get("hooks") {
        if let Some(hooks_tbl) = hooks_item.as_table() {
            for (event_name, event_val) in hooks_tbl {
                if let Some(aot) = event_val.as_array_of_tables() {
                    let mut entries_arr = Vec::new();
                    for entry_tbl in aot {
                        let mut entry_obj = serde_json::Map::new();

                        // matcher
                        if let Some(m) = entry_tbl.get("matcher").and_then(|v| v.as_str()) {
                            entry_obj.insert("matcher".to_string(), Value::String(m.to_string()));
                        }

                        // hooks array-of-tables
                        if let Some(hooks_aot_item) = entry_tbl.get("hooks") {
                            if let Some(hooks_aot) = hooks_aot_item.as_array_of_tables() {
                                let mut hooks_json = Vec::new();
                                for h_tbl in hooks_aot {
                                    let hook_obj = toml_table_to_json(h_tbl);
                                    hooks_json.push(Value::Object(hook_obj));
                                }
                                entry_obj.insert("hooks".to_string(), Value::Array(hooks_json));
                            }
                        }

                        entries_arr.push(Value::Object(entry_obj));
                    }
                    hooks_map.insert(event_name.to_string(), Value::Array(entries_arr));
                }
            }
        }
    }

    Ok(Value::Object({
        let mut root = serde_json::Map::new();
        root.insert(
            "path".to_string(),
            Value::String(path.to_str().unwrap_or("").to_string()),
        );
        root.insert("hooks".to_string(), Value::Object(hooks_map));
        root
    }))
}

/// toml_edit Table を serde_json Map に変換するヘルパ。
fn toml_table_to_json(tbl: &Table) -> serde_json::Map<String, Value> {
    let mut map = serde_json::Map::new();
    for (k, v) in tbl {
        if let Some(jv) = toml_item_to_json(v) {
            map.insert(k.to_string(), jv);
        }
    }
    map
}

/// toml_edit Item を serde_json Value に変換するヘルパ。
fn toml_item_to_json(item: &Item) -> Option<Value> {
    match item {
        Item::Value(v) => toml_value_to_json(v),
        Item::Table(tbl) => {
            let obj = toml_table_to_json(tbl);
            Some(Value::Object(obj))
        }
        Item::ArrayOfTables(aot) => {
            let arr: Vec<Value> = aot
                .iter()
                .map(|t| Value::Object(toml_table_to_json(t)))
                .collect();
            Some(Value::Array(arr))
        }
        Item::None => None,
    }
}

/// toml_edit::Value を serde_json::Value に変換するヘルパ。
fn toml_value_to_json(tv: &toml_edit::Value) -> Option<Value> {
    use toml_edit::Value as TV;
    match tv {
        TV::String(s) => Some(Value::String(s.value().to_string())),
        TV::Integer(i) => Some(Value::Number(serde_json::Number::from(*i.value()))),
        TV::Float(f) => serde_json::Number::from_f64(*f.value()).map(Value::Number),
        TV::Boolean(b) => Some(Value::Bool(*b.value())),
        TV::Array(arr) => {
            let items: Vec<Value> = arr.iter().filter_map(toml_value_to_json).collect();
            Some(Value::Array(items))
        }
        TV::InlineTable(tbl) => {
            let mut map = serde_json::Map::new();
            for (k, v) in tbl {
                if let Some(jv) = toml_value_to_json(v) {
                    map.insert(k.to_string(), jv);
                }
            }
            Some(Value::Object(map))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mappings::load_mappings;
    use std::path::Path;
    use tempfile::TempDir;

    fn make_handler() -> HooksHandler {
        let maps = load_mappings(Path::new("mappings"));
        HooksHandler {
            map: maps["hooks"].clone(),
        }
    }

    fn default_opts(out_dir: &str) -> LowerOpts {
        LowerOpts {
            out: Some(out_dir.to_string()),
            scope: Scope::Project,
            dual_manifest: false,
            hooks_target: Scope::User,
            skill_target: crate::handlers::SkillTargetMode::Skill,
            interactive: false,
            rewrite_body: false,
        }
    }

    #[test]
    fn test_hooks_detect() {
        let h = make_handler();
        assert!(h.detect(Path::new("hooks.json")));
        assert!(h.detect(Path::new("settings.json")));
        assert!(!h.detect(Path::new(".mcp.json")));
        assert!(!h.detect(Path::new("SKILL.md")));
    }

    #[test]
    fn test_normalize_matcher_exact() {
        let (norm, lossy) = normalize_matcher_c2x("Bash");
        assert_eq!(norm, "^Bash$");
        assert!(lossy);
    }

    #[test]
    fn test_normalize_matcher_alternation() {
        let (norm, lossy) = normalize_matcher_c2x("Edit|Write");
        assert_eq!(norm, "^(Edit|Write)$");
        assert!(lossy);
    }

    #[test]
    fn test_normalize_matcher_wildcard_star() {
        let (norm, lossy) = normalize_matcher_c2x("*");
        assert_eq!(norm, "");
        assert!(lossy);
    }

    #[test]
    fn test_normalize_matcher_wildcard_empty() {
        let (norm, lossy) = normalize_matcher_c2x("");
        assert_eq!(norm, "");
        assert!(lossy);
    }

    #[test]
    fn test_normalize_matcher_regex_passthrough() {
        let (norm, lossy) = normalize_matcher_c2x("^Bash.*");
        assert_eq!(norm, "^Bash.*");
        assert!(!lossy);
    }

    #[test]
    fn test_hooks_lift_c2x_common_event() {
        let hooks_json = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [
                            { "type": "command", "command": "echo pre-tool" }
                        ]
                    }
                ]
            }
        });

        let h = make_handler();
        let ir = h.lift_c2x(&hooks_json).unwrap();

        let field = ir.fields.get("hooks.event.PreToolUse");
        assert!(field.is_some(), "Expected PreToolUse field");
        let f = field.unwrap();
        assert_eq!(f.loss, Loss::Lossless);
        // matcher should be normalized
        let entries = f.value.as_array().unwrap();
        let first = &entries[0];
        let matcher = first.get("matcher").and_then(|v| v.as_str()).unwrap();
        assert_eq!(matcher, "^Bash$", "Expected normalized matcher");
    }

    #[test]
    fn test_hooks_lift_c2x_claude_only_event_dropped() {
        let hooks_json = serde_json::json!({
            "hooks": {
                "Setup": [
                    {
                        "matcher": "",
                        "hooks": [{ "type": "command", "command": "echo setup" }]
                    }
                ]
            }
        });

        let h = make_handler();
        let ir = h.lift_c2x(&hooks_json).unwrap();

        let field = ir.fields.get("hooks.event.Setup");
        assert!(field.is_some(), "Expected Setup field");
        assert_eq!(field.unwrap().loss, Loss::Dropped);
        // diagnostic
        let has_drop_diag = ir
            .diagnostics
            .iter()
            .any(|d| d.level == DiagLevel::Drop && d.message.contains("Setup"));
        assert!(has_drop_diag, "Expected Drop diagnostic for Setup event");
    }

    #[test]
    fn test_hooks_lift_c2x_http_type_dropped() {
        let hooks_json = serde_json::json!({
            "hooks": {
                "PostToolUse": [
                    {
                        "matcher": "",
                        "hooks": [
                            { "type": "http", "url": "https://example.com" }
                        ]
                    }
                ]
            }
        });

        let h = make_handler();
        let ir = h.lift_c2x(&hooks_json).unwrap();

        // The event field is stored (not dropped), but http hook is filtered out
        // Check that there's a Drop diagnostic about http type
        let has_http_drop = ir
            .diagnostics
            .iter()
            .any(|d| d.message.contains("http") && d.level == DiagLevel::Drop);
        assert!(has_http_drop, "Expected Drop diagnostic for http type");
    }

    #[test]
    fn test_hooks_lower_c2x_user_scope_json() {
        let hooks_json = serde_json::json!({
            "hooks": {
                "Stop": [
                    {
                        "matcher": "",
                        "hooks": [
                            { "type": "command", "command": "echo stop" }
                        ]
                    }
                ]
            }
        });

        let h = make_handler();
        let ir = h.lift_c2x(&hooks_json).unwrap();

        let dir = TempDir::new().unwrap();
        let opts = default_opts(dir.path().to_str().unwrap());
        let plan = h.lower_c2x(&ir, &opts).unwrap();

        // Should produce hooks.json
        let has_hooks_json = plan.files.iter().any(|f| f.path.ends_with("hooks.json"));
        assert!(has_hooks_json, "Expected hooks.json output for user scope");

        let hj = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("hooks.json"))
            .unwrap();
        let parsed: Value = serde_json::from_str(&hj.content).unwrap();
        assert!(
            parsed.get("Stop").is_some(),
            "Expected Stop event in hooks.json"
        );
    }

    #[test]
    fn test_hooks_lower_c2x_project_scope_toml() {
        let hooks_json = serde_json::json!({
            "hooks": {
                "Stop": [
                    {
                        "matcher": "",
                        "hooks": [
                            { "type": "command", "command": "echo stop" }
                        ]
                    }
                ]
            }
        });

        let h = make_handler();
        let ir = h.lift_c2x(&hooks_json).unwrap();

        let dir = TempDir::new().unwrap();
        let mut opts = default_opts(dir.path().to_str().unwrap());
        opts.hooks_target = Scope::Project;
        let plan = h.lower_c2x(&ir, &opts).unwrap();

        let has_config_toml = plan.files.iter().any(|f| f.path.ends_with("config.toml"));
        assert!(
            has_config_toml,
            "Expected config.toml output for project scope"
        );

        let ct = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("config.toml"))
            .unwrap();
        assert!(
            ct.content.contains("[[hooks.Stop]]"),
            "Expected [[hooks.Stop]] in config.toml, got: {}",
            ct.content
        );
    }

    #[test]
    fn test_hooks_lift_c2x_args_synthesized() {
        let hooks_json = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [
                            {
                                "type": "command",
                                "command": "my-script",
                                "args": ["--flag", "value with spaces"]
                            }
                        ]
                    }
                ]
            }
        });

        let h = make_handler();
        let ir = h.lift_c2x(&hooks_json).unwrap();

        let field = ir.fields.get("hooks.event.PreToolUse").unwrap();
        let entries = field.value.as_array().unwrap();
        let hook = &entries[0]["hooks"][0];
        let cmd = hook["command"].as_str().unwrap();
        // Should contain the synthesized command with quoted args
        assert!(cmd.contains("my-script"), "Expected my-script in command");
        assert!(
            cmd.contains("--flag"),
            "Expected --flag in synthesized command"
        );
        // "value with spaces" should be quoted
        assert!(
            cmd.contains("value with spaces") || cmd.contains("'value with spaces'"),
            "Expected quoted arg in: {}",
            cmd
        );
    }

    #[test]
    fn test_hooks_lift_x2c_toml_roundtrip() {
        // Simulate parsing Codex TOML hooks structure
        let codex_parsed = serde_json::json!({
            "path": ".codex/config.toml",
            "hooks": {
                "Stop": [
                    {
                        "matcher": "",
                        "hooks": [
                            { "type": "command", "command": "echo done" }
                        ]
                    }
                ]
            }
        });

        let h = make_handler();
        let ir = h.lift_x2c(&codex_parsed).unwrap();

        let field = ir.fields.get("hooks.event.Stop");
        assert!(field.is_some(), "Expected Stop event");
        assert_eq!(field.unwrap().loss, Loss::Lossless);
    }

    #[test]
    fn test_hooks_lower_x2c() {
        let codex_parsed = serde_json::json!({
            "path": ".codex/config.toml",
            "hooks": {
                "PostToolUse": [
                    {
                        "matcher": "^Bash$",
                        "hooks": [
                            { "type": "command", "command": "echo post" }
                        ]
                    }
                ]
            }
        });

        let h = make_handler();
        let ir = h.lift_x2c(&codex_parsed).unwrap();

        let dir = TempDir::new().unwrap();
        let opts = default_opts(dir.path().to_str().unwrap());
        let plan = h.lower_x2c(&ir, &opts).unwrap();

        let hj = plan
            .files
            .iter()
            .find(|f| f.path.ends_with("hooks.json"))
            .unwrap();
        let parsed: Value = serde_json::from_str(&hj.content).unwrap();
        assert!(
            parsed["hooks"]["PostToolUse"].is_array(),
            "Expected PostToolUse in output hooks.json"
        );
    }

    #[test]
    fn test_hooks_lift_c2x_16430_warn() {
        let hooks_json = serde_json::json!({
            "hooks": {
                "Stop": [{
                    "matcher": "",
                    "hooks": [{ "type": "command", "command": "echo ok" }]
                }]
            }
        });

        let h = make_handler();
        let ir = h.lift_c2x(&hooks_json).unwrap();
        let dir = TempDir::new().unwrap();
        let opts = default_opts(dir.path().to_str().unwrap());
        let plan = h.lower_c2x(&ir, &opts).unwrap();

        // #16430 warning should be present
        let has_16430_warn = plan
            .diagnostics
            .iter()
            .any(|d| d.message.contains("#16430"));
        assert!(has_16430_warn, "Expected #16430 warning in diagnostics");
    }
}
