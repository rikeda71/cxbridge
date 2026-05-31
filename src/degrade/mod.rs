// Each degradation yields SideArtifact (generated file) + Diagnostic (degradation record)
// because Codex lacks the concept of per-skill scope.

pub mod hooks_scope;
pub mod rules;
pub mod subagent;
