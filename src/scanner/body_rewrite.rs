use super::body::{Action, BodyFinding};

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

    let mut result_lines: Vec<String> = raw.lines().map(str::to_string).collect();

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
    use crate::core::transforms::ConvDir;
    use crate::scanner::body::{scan_body, BodyContext};

    #[test]
    fn test_rewrite_body() {
        let body = "Use $ARGUMENTS[1] here\n";
        let findings = scan_body(body, ConvDir::C2x, BodyContext::SkillBody);
        let result = rewrite_body(body, &findings);
        assert!(result.contains("$2"), "Expected $2 in result: {}", result);
    }

    #[test]
    fn test_rewrite_body_no_rewrites() {
        let body = "No special patterns here\n";
        let findings = scan_body(body, ConvDir::C2x, BodyContext::SkillBody);
        let result = rewrite_body(body, &findings);
        assert_eq!(result, body);
    }

    #[test]
    fn test_rewrite_body_dollar_dollar_does_not_overlap_positional_x2c() {
        let body = "Escaped $$1 here";
        let findings = scan_body(body, ConvDir::X2c, BodyContext::SkillBody);
        assert_eq!(rewrite_body(body, &findings), "Escaped $1 here");
    }
}
