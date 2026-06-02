use std::path::Path;

use anyhow::Context;

use crate::handlers::EmitFile;

/// Walks `skill_dir` and collects all non-`.md` files (excluding
/// `agents/openai.yaml`) as `EmitFile` values with paths remapped under
/// `out_skill_dir`.
///
/// Content is read as UTF-8; binary files are silently skipped.
pub(super) fn collect_aux_files(
    skill_dir: &Path,
    out_skill_dir: &str,
) -> anyhow::Result<Vec<EmitFile>> {
    let mut result = Vec::new();
    collect_aux_files_recursive(skill_dir, skill_dir, out_skill_dir, &mut result)?;
    Ok(result)
}

fn collect_aux_files_recursive(
    base_dir: &Path,
    current_dir: &Path,
    out_skill_dir: &str,
    result: &mut Vec<EmitFile>,
) -> anyhow::Result<()> {
    let entries = match std::fs::read_dir(current_dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    for entry in entries {
        let entry = entry.with_context(|| {
            format!(
                "Failed to read directory entry in {}",
                current_dir.display()
            )
        })?;
        let path = entry.path();
        if path.is_dir() {
            collect_aux_files_recursive(base_dir, &path, out_skill_dir, result)?;
            continue;
        }
        // Skip .md files (SKILL.md is handled separately)
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            continue;
        }
        // Compute relative path from base_dir
        let rel = path.strip_prefix(base_dir).with_context(|| {
            format!(
                "Path {} is not under {}",
                path.display(),
                base_dir.display()
            )
        })?;
        // Skip agents/openai.yaml (handled separately as SideArtifact or via lift)
        let rel_str = rel.to_str().unwrap_or("");
        if rel_str == "agents/openai.yaml" || rel_str == "agents\\openai.yaml" {
            continue;
        }
        // Read content as UTF-8; skip silently if not valid UTF-8 (binary)
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let out_path = format!("{}/{}", out_skill_dir, rel_str.replace('\\', "/"));
        result.push(EmitFile {
            path: out_path,
            content,
        });
    }
    Ok(())
}

/// Extracts the skill name from source_path.
/// .claude/skills/<name>/SKILL.md → <name>
/// .agents/skills/<name>/SKILL.md → <name>
/// Anything else → "skill"
pub(super) fn extract_skill_name(source_path: &str) -> String {
    let path = Path::new(source_path);
    // Return the name of the parent directory of SKILL.md
    if let Some(parent) = path.parent() {
        if let Some(name) = parent.file_name() {
            let n = name.to_str().unwrap_or("unknown");
            if n != "skills" && n != ".claude" && n != ".agents" {
                return n.to_string();
            }
        }
    }
    // Fallback: the string "skill"
    "skill".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // ── extract_skill_name ────────────────────────────────────────────────

    #[test]
    fn extract_skill_name_claude_path() {
        assert_eq!(
            extract_skill_name("/home/user/.claude/skills/deploy/SKILL.md"),
            "deploy"
        );
    }

    #[test]
    fn extract_skill_name_agents_path() {
        assert_eq!(extract_skill_name(".agents/skills/build/SKILL.md"), "build");
    }

    #[test]
    fn extract_skill_name_parent_is_skills_falls_back() {
        // Path where the parent dir IS "skills" — no valid skill name above it
        assert_eq!(extract_skill_name("skills/SKILL.md"), "skill");
    }

    #[test]
    fn extract_skill_name_bare_filename_falls_back() {
        assert_eq!(extract_skill_name("SKILL.md"), "skill");
    }

    #[test]
    fn extract_skill_name_parent_is_dot_claude_falls_back() {
        assert_eq!(extract_skill_name(".claude/SKILL.md"), "skill");
    }

    // ── collect_aux_files ─────────────────────────────────────────────────

    #[test]
    fn collect_aux_files_returns_non_md_files() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join("my-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("README.txt"), "readme content\n").unwrap();
        fs::write(skill_dir.join("SKILL.md"), "---\nname: my-skill\n---\n").unwrap();

        let files = collect_aux_files(&skill_dir, ".agents/skills/my-skill").unwrap();

        let readme = files.iter().find(|f| f.path.ends_with("README.txt"));
        assert!(
            readme.is_some(),
            "README.txt must be collected as aux file; got: {:?}",
            files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );
        assert_eq!(readme.unwrap().content.trim(), "readme content");

        // .md files must be excluded
        let has_md = files.iter().any(|f| f.path.ends_with(".md"));
        assert!(!has_md, "SKILL.md must not appear in aux files");
    }

    #[test]
    fn collect_aux_files_recurses_into_subdirectories() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join("sk");
        let scripts_dir = skill_dir.join("scripts");
        let refs_dir = skill_dir.join("references");
        fs::create_dir_all(&scripts_dir).unwrap();
        fs::create_dir_all(&refs_dir).unwrap();
        fs::write(scripts_dir.join("run.sh"), "#!/bin/bash\necho hi\n").unwrap();
        fs::write(refs_dir.join("spec.txt"), "spec content\n").unwrap();

        let files = collect_aux_files(&skill_dir, ".agents/skills/sk").unwrap();

        let run_sh = files
            .iter()
            .find(|f| f.path == ".agents/skills/sk/scripts/run.sh");
        assert!(
            run_sh.is_some(),
            "scripts/run.sh must be collected with full remapped path"
        );
        assert_eq!(run_sh.unwrap().content, "#!/bin/bash\necho hi\n");

        let spec = files
            .iter()
            .find(|f| f.path == ".agents/skills/sk/references/spec.txt");
        assert!(
            spec.is_some(),
            "references/spec.txt must be collected with full remapped path"
        );
    }

    #[test]
    fn collect_aux_files_excludes_agents_openai_yaml() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join("sk");
        let agents_dir = skill_dir.join("agents");
        fs::create_dir_all(&agents_dir).unwrap();
        fs::write(
            agents_dir.join("openai.yaml"),
            "policy:\n  allow_implicit_invocation: true\n",
        )
        .unwrap();
        // A different yaml in agents/ should be collected normally
        fs::write(agents_dir.join("other.yaml"), "key: value\n").unwrap();

        let files = collect_aux_files(&skill_dir, ".agents/skills/sk").unwrap();

        let has_openai = files.iter().any(|f| f.path.ends_with("agents/openai.yaml"));
        assert!(
            !has_openai,
            "agents/openai.yaml must be excluded from aux file collection"
        );

        let other = files.iter().find(|f| f.path.ends_with("agents/other.yaml"));
        assert!(other.is_some(), "agents/other.yaml must still be collected");
    }

    #[test]
    fn collect_aux_files_remaps_output_prefix() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join("sk");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("data.txt"), "data\n").unwrap();

        let out_prefix = "custom/output/prefix";
        let files = collect_aux_files(&skill_dir, out_prefix).unwrap();

        let data = files
            .iter()
            .find(|f| f.path == "custom/output/prefix/data.txt");
        assert!(
            data.is_some(),
            "output path must use the supplied out_skill_dir prefix; got: {:?}",
            files.iter().map(|f| &f.path).collect::<Vec<_>>()
        );
    }

    #[test]
    fn collect_aux_files_empty_dir_returns_empty_vec() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join("empty-skill");
        fs::create_dir_all(&skill_dir).unwrap();

        let files = collect_aux_files(&skill_dir, ".agents/skills/empty-skill").unwrap();
        assert!(files.is_empty(), "empty skill dir must yield no aux files");
    }

    #[test]
    fn collect_aux_files_nonexistent_dir_returns_empty_vec() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir.path().join("does-not-exist");

        // Nonexistent directory is silently treated as empty
        let files = collect_aux_files(&skill_dir, ".agents/skills/x").unwrap();
        assert!(
            files.is_empty(),
            "nonexistent skill dir must yield no aux files (silently ok)"
        );
    }
}
