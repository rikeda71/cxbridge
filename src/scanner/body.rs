use once_cell::sync::Lazy;
use regex::Regex;

use crate::core::transforms::ConvDir;

/// The category of a pattern detected in skill/command/prompt body text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindingKind {
    /// Zero-based positional argument: `$ARGUMENTS[0]`, `$ARGUMENTS[1]`, etc.
    ArgIndexed,
    /// Named argument: `$name` form
    ArgNamed,
    /// Environment variable reference: `${CLAUDE_*}`
    EnvVar,
    /// Dynamic inline injection: `!`cmd``
    DynamicInline,
    /// Dynamic block injection: ` ```! ... ``` `
    DynamicBlock,
    /// Slash command invocation without namespace: `/skill-name`
    InvokeSlash,
    /// Namespaced slash invocation: `/namespace:skill`
    InvokeNamespaced,
}

/// scan_body が本文中の検出事項に対して提案するアクション。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// 自動書き換え可能（rewrite_body で置換する）
    Rewrite,
    /// 警告のみ（自動書き換えしない。手動対応を促す）
    Warn,
    /// 出力から除去を提案（自動除去はしない）
    Drop,
}

/// 本文中の検出事項1件。
#[derive(Debug, Clone)]
pub struct BodyFinding {
    pub kind: FindingKind,
    /// マッチしたテキスト
    pub matched: String,
    /// 行番号（1 始まり）
    pub line: usize,
    /// 推奨アクション
    pub action: Action,
    /// action == Rewrite の場合の置換後テキスト
    pub rewrite: Option<String>,
    /// レポート用の説明メッセージ
    pub note: String,
}

static RE_ARG_INDEXED_BRACKET: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\$ARGUMENTS\[(\d+)\]").unwrap());

// Codex positional argument on the x2c side.
static RE_ARG_POSITIONAL: Lazy<Regex> = Lazy::new(|| Regex::new(r"\$([1-9][0-9]*)").unwrap());

// Matches bare $ARGUMENTS; whether `[N]` follows is determined by post-processing.
static RE_ARG_BARE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\$ARGUMENTS").unwrap());

// x2c only: escaped dollar sign.
static RE_DOLLAR_DOLLAR: Lazy<Regex> = Lazy::new(|| Regex::new(r"\$\$").unwrap());

// Lowercase-led variable names, excluding $ARGUMENTS and ${...}.
static RE_ARG_NAMED: Lazy<Regex> = Lazy::new(|| Regex::new(r"\$([a-z][a-z0-9_]*)").unwrap());

static RE_ENV_VAR: Lazy<Regex> = Lazy::new(|| Regex::new(r"\$\{CLAUDE_[A-Z_]+\}").unwrap());

static RE_DYNAMIC_INLINE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(^|\s)!`[^`]+`").unwrap());

static RE_DYNAMIC_BLOCK: Lazy<Regex> = Lazy::new(|| Regex::new(r"```!").unwrap());

// Must be matched before RE_INVOKE_SLASH so the colon form takes priority.
static RE_INVOKE_NAMESPACED: Lazy<Regex> = Lazy::new(|| Regex::new(r"/[\w-]+:[\w-]+").unwrap());

static RE_INVOKE_SLASH: Lazy<Regex> = Lazy::new(|| Regex::new(r"/[\w-]+").unwrap());

/// Scans `body` for patterns that differ between Claude Code and Codex CLI syntaxes.
/// Returns findings without modifying the input; callers apply rewrites via [`rewrite_body`].
///
/// Detection actions per pattern in the `c2x` direction:
///
/// | Pattern | Action |
/// |---|---|
/// | `$ARGUMENTS[N]` (N ≥ 1) | Rewrite → `$N+1` |
/// | `$ARGUMENTS[0]` | Warn + propose `$1` (not auto-rewritten; `$0` is the shell script name) |
/// | bare `$ARGUMENTS` | Warn (Codex only supports positional form in Custom Prompts) |
/// | `$name` named arg | Warn (callers must switch to `KEY=value` invocation) |
/// | `${CLAUDE_*}` env var | Drop (no Codex equivalent) |
/// | `!`cmd`` inline injection | Warn (Codex treats it as a literal string) |
/// | ` ```! ` block injection | Warn |
/// | `/skill-name` slash call | Warn + propose `$skill-name` |
/// | `/namespace:skill` | Drop (no namespace concept in Codex) |
///
/// In the `x2c` direction: `$$` → `$` (Rewrite); `$N` → `$ARGUMENTS[N-1]` (Rewrite).
pub fn scan_body(body: &str, dir: ConvDir) -> Vec<BodyFinding> {
    let mut findings = Vec::new();

    for (line_idx, line) in body.lines().enumerate() {
        let line_no = line_idx + 1;

        match dir {
            ConvDir::C2x => {
                scan_c2x_line(line, line_no, &mut findings);
            }
            ConvDir::X2c => {
                scan_x2c_line(line, line_no, &mut findings);
            }
        }
    }

    findings
}

fn scan_c2x_line(line: &str, line_no: usize, findings: &mut Vec<BodyFinding>) {
    // Namespaced calls must be collected first so their byte ranges can be
    // excluded when scanning for plain slash calls below.
    for cap in RE_INVOKE_NAMESPACED.find_iter(line) {
        findings.push(BodyFinding {
            kind: FindingKind::InvokeNamespaced,
            matched: cap.as_str().to_string(),
            line: line_no,
            action: Action::Drop,
            rewrite: None,
            note: "Codex has no namespace concept for slash commands; manual conversion required."
                .to_string(),
        });
    }

    for cap in RE_ENV_VAR.find_iter(line) {
        findings.push(BodyFinding {
            kind: FindingKind::EnvVar,
            matched: cap.as_str().to_string(),
            line: line_no,
            action: Action::Drop,
            rewrite: None,
            note: format!(
                "{} has no Codex equivalent and must be removed.",
                cap.as_str()
            ),
        });
    }

    // Process $ARGUMENTS[N] before bare $ARGUMENTS so bracket forms are
    // already recorded in processed_positions when the bare scan runs.
    let mut processed_positions: Vec<(usize, usize)> = Vec::new();

    for cap in RE_ARG_INDEXED_BRACKET.captures_iter(line) {
        let full_match = cap.get(0).unwrap();
        let idx_str = &cap[1];
        let idx: usize = idx_str.parse().unwrap_or(0);

        processed_positions.push((full_match.start(), full_match.end()));

        if idx == 0 {
            // $0 is the shell script name, so auto-rewriting $ARGUMENTS[0]→$1
            // would silently introduce a collision. Propose only.
            findings.push(BodyFinding {
                kind: FindingKind::ArgIndexed,
                matched: full_match.as_str().to_string(),
                line: line_no,
                action: Action::Warn,
                rewrite: Some("$1".to_string()),
                note: "Proposing $ARGUMENTS[0] → $1; not auto-rewritten because $0 collides with the shell script name."
                    .to_string(),
            });
        } else {
            findings.push(BodyFinding {
                kind: FindingKind::ArgIndexed,
                matched: full_match.as_str().to_string(),
                line: line_no,
                action: Action::Rewrite,
                rewrite: Some(format!("${}", idx + 1)),
                note: format!("$ARGUMENTS[{}] → ${} (index +1)", idx, idx + 1),
            });
        }
    }

    for cap in RE_ARG_BARE.find_iter(line) {
        let start = cap.start();
        let end = cap.end();
        // If the next character is '[' this is a bracket form already handled above.
        let next_char = line.as_bytes().get(end).copied();
        if next_char == Some(b'[') {
            continue;
        }
        if processed_positions
            .iter()
            .any(|(s, e)| start >= *s && start < *e)
        {
            continue;
        }
        findings.push(BodyFinding {
            kind: FindingKind::ArgIndexed,
            matched: cap.as_str().to_string(),
            line: line_no,
            action: Action::Warn,
            rewrite: None,
            note: "Bare $ARGUMENTS is only supported in Codex Custom Prompts, not in skill bodies."
                .to_string(),
        });
    }

    for cap in RE_ARG_NAMED.captures_iter(line) {
        let full_match = cap.get(0).unwrap();
        let name = &cap[1];
        // RE_ARG_NAMED targets lowercase; guard against false-positive on "arguments".
        if name.starts_with("arguments") {
            continue;
        }
        let start = full_match.start();
        if processed_positions
            .iter()
            .any(|(s, e)| start >= *s && start < *e)
        {
            continue;
        }
        findings.push(BodyFinding {
            kind: FindingKind::ArgNamed,
            matched: full_match.as_str().to_string(),
            line: line_no,
            action: Action::Warn,
            rewrite: None,
            note: format!(
                "${} becomes a KEY=value invocation argument in Codex; verify all references in the body.",
                name
            ),
        });
    }

    for cap in RE_DYNAMIC_INLINE.find_iter(line) {
        findings.push(BodyFinding {
            kind: FindingKind::DynamicInline,
            matched: cap.as_str().trim().to_string(),
            line: line_no,
            action: Action::Warn,
            rewrite: None,
            note: "Inline !`cmd` injection is not supported by Codex; treated as a literal string."
                .to_string(),
        });
    }

    for cap in RE_DYNAMIC_BLOCK.find_iter(line) {
        findings.push(BodyFinding {
            kind: FindingKind::DynamicBlock,
            matched: cap.as_str().to_string(),
            line: line_no,
            action: Action::Warn,
            rewrite: None,
            note: "Block ```! injection is not supported by Codex; manual conversion required."
                .to_string(),
        });
    }

    // Exclude byte ranges already matched by RE_INVOKE_NAMESPACED.
    let ns_positions: Vec<(usize, usize)> = RE_INVOKE_NAMESPACED
        .find_iter(line)
        .map(|m| (m.start(), m.end()))
        .collect();

    for cap in RE_INVOKE_SLASH.find_iter(line) {
        let start = cap.start();
        if ns_positions.iter().any(|(s, e)| start >= *s && start < *e) {
            continue;
        }
        let skill_name = &cap.as_str()[1..]; // strip leading slash
        findings.push(BodyFinding {
            kind: FindingKind::InvokeSlash,
            matched: cap.as_str().to_string(),
            line: line_no,
            action: Action::Warn,
            rewrite: Some(format!("${}", skill_name)),
            note: format!(
                "/{} should become ${} in Codex; verify before applying.",
                skill_name, skill_name
            ),
        });
    }
}

fn scan_x2c_line(line: &str, line_no: usize, findings: &mut Vec<BodyFinding>) {
    for cap in RE_DOLLAR_DOLLAR.find_iter(line) {
        findings.push(BodyFinding {
            kind: FindingKind::ArgIndexed,
            matched: cap.as_str().to_string(),
            line: line_no,
            action: Action::Rewrite,
            rewrite: Some("$".to_string()),
            note: "$$ → $".to_string(),
        });
    }

    for cap in RE_ARG_POSITIONAL.captures_iter(line) {
        let full_match = cap.get(0).unwrap();
        let idx_str = &cap[1];
        let idx: usize = idx_str.parse().unwrap_or(1);
        findings.push(BodyFinding {
            kind: FindingKind::ArgIndexed,
            matched: full_match.as_str().to_string(),
            line: line_no,
            action: Action::Rewrite,
            rewrite: Some(format!("$ARGUMENTS[{}]", idx - 1)),
            note: format!("${} → $ARGUMENTS[{}] (index -1)", idx, idx - 1),
        });
    }
}

/// Applies all `Action::Rewrite` findings to `raw` and returns the updated string.
///
/// Only called when `opts.rewrite_body` is true; otherwise the caller emits the
/// body unchanged.
pub fn rewrite_body(raw: &str, findings: &[BodyFinding]) -> String {
    let rewrites: Vec<&BodyFinding> = findings
        .iter()
        .filter(|f| f.action == Action::Rewrite && f.rewrite.is_some())
        .collect();

    if rewrites.is_empty() {
        return raw.to_string();
    }

    let lines: Vec<&str> = raw.lines().collect();
    let mut result_lines: Vec<String> = lines.iter().map(|l| l.to_string()).collect();

    for finding in &rewrites {
        let line_idx = finding.line - 1; // convert 1-based to 0-based
        if line_idx < result_lines.len() {
            if let Some(rewrite) = &finding.rewrite {
                result_lines[line_idx] =
                    result_lines[line_idx].replacen(&finding.matched, rewrite, 1);
            }
        }
    }

    let mut output = result_lines.join("\n");
    if raw.ends_with('\n') {
        output.push('\n');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_body_arg_indexed_c2x() {
        let body = "Use $ARGUMENTS[0] and $ARGUMENTS[1] here";
        let findings = scan_body(body, ConvDir::C2x);
        let indexed: Vec<_> = findings
            .iter()
            .filter(|f| f.kind == FindingKind::ArgIndexed)
            .collect();
        assert_eq!(indexed.len(), 2);
        // $ARGUMENTS[0] → warn (not auto-rewrite due to $0 conflict)
        let f0 = indexed
            .iter()
            .find(|f| f.matched == "$ARGUMENTS[0]")
            .unwrap();
        assert_eq!(f0.action, Action::Warn);
        // $ARGUMENTS[1] → rewrite to $2
        let f1 = indexed
            .iter()
            .find(|f| f.matched == "$ARGUMENTS[1]")
            .unwrap();
        assert_eq!(f1.action, Action::Rewrite);
        assert_eq!(f1.rewrite, Some("$2".to_string()));
    }

    #[test]
    fn test_scan_body_bare_arguments_c2x() {
        let body = "Pass $ARGUMENTS to the command";
        let findings = scan_body(body, ConvDir::C2x);
        let bare: Vec<_> = findings
            .iter()
            .filter(|f| f.kind == FindingKind::ArgIndexed && f.matched == "$ARGUMENTS")
            .collect();
        assert_eq!(bare.len(), 1);
        assert_eq!(bare[0].action, Action::Warn);
    }

    #[test]
    fn test_scan_body_env_var_c2x() {
        let body = "Session: ${CLAUDE_SESSION_ID}";
        let findings = scan_body(body, ConvDir::C2x);
        let env: Vec<_> = findings
            .iter()
            .filter(|f| f.kind == FindingKind::EnvVar)
            .collect();
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].action, Action::Drop);
    }

    #[test]
    fn test_scan_body_dynamic_inline_c2x() {
        let body = "Run !`git diff` to see changes";
        let findings = scan_body(body, ConvDir::C2x);
        let inline: Vec<_> = findings
            .iter()
            .filter(|f| f.kind == FindingKind::DynamicInline)
            .collect();
        assert_eq!(inline.len(), 1);
        assert_eq!(inline[0].action, Action::Warn);
    }

    #[test]
    fn test_scan_body_namespaced_c2x() {
        let body = "Call /claude:deploy to deploy";
        let findings = scan_body(body, ConvDir::C2x);
        let ns: Vec<_> = findings
            .iter()
            .filter(|f| f.kind == FindingKind::InvokeNamespaced)
            .collect();
        assert_eq!(ns.len(), 1);
        assert_eq!(ns[0].action, Action::Drop);
    }

    #[test]
    fn test_scan_body_slash_c2x() {
        let body = "Use /deploy command";
        let findings = scan_body(body, ConvDir::C2x);
        let slash: Vec<_> = findings
            .iter()
            .filter(|f| f.kind == FindingKind::InvokeSlash)
            .collect();
        assert_eq!(slash.len(), 1);
        assert_eq!(slash[0].action, Action::Warn);
        assert_eq!(slash[0].rewrite, Some("$deploy".to_string()));
    }

    #[test]
    fn test_scan_body_dollar_dollar_x2c() {
        let body = "Escaped $$ dollar sign";
        let findings = scan_body(body, ConvDir::X2c);
        let dd: Vec<_> = findings.iter().filter(|f| f.matched == "$$").collect();
        assert_eq!(dd.len(), 1);
        assert_eq!(dd[0].action, Action::Rewrite);
        assert_eq!(dd[0].rewrite, Some("$".to_string()));
    }

    #[test]
    fn test_scan_body_positional_x2c() {
        let body = "Use $1 and $2 here";
        let findings = scan_body(body, ConvDir::X2c);
        let pos: Vec<_> = findings
            .iter()
            .filter(|f| f.kind == FindingKind::ArgIndexed)
            .collect();
        assert_eq!(pos.len(), 2);
        let f1 = pos.iter().find(|f| f.matched == "$1").unwrap();
        assert_eq!(f1.rewrite, Some("$ARGUMENTS[0]".to_string()));
        let f2 = pos.iter().find(|f| f.matched == "$2").unwrap();
        assert_eq!(f2.rewrite, Some("$ARGUMENTS[1]".to_string()));
    }

    #[test]
    fn test_rewrite_body() {
        let body = "Use $ARGUMENTS[1] here\n";
        let findings = scan_body(body, ConvDir::C2x);
        let result = rewrite_body(body, &findings);
        assert!(result.contains("$2"), "Expected $2 in result: {}", result);
    }

    #[test]
    fn test_rewrite_body_no_rewrites() {
        let body = "No special patterns here\n";
        let findings = scan_body(body, ConvDir::C2x);
        let result = rewrite_body(body, &findings);
        assert_eq!(result, body);
    }

    #[test]
    fn test_scan_body_line_numbers() {
        let body = "line 1\nRun $ARGUMENTS[1]\nline 3\n";
        let findings = scan_body(body, ConvDir::C2x);
        let indexed: Vec<_> = findings
            .iter()
            .filter(|f| f.kind == FindingKind::ArgIndexed)
            .collect();
        assert!(!indexed.is_empty());
        assert_eq!(indexed[0].line, 2);
    }
}
