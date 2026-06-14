/// Typed representations of the two orthogonal Codex permission axes and the
/// single Claude axis, plus the bidirectional conversion matrix.
///
/// Codex separates *what* the sandbox boundary is (`SandboxMode`) from *when* the
/// model must ask for human approval (`ApprovalPolicy`). Claude collapses both into
/// a single `defaultMode`. The matrices here are the single source of truth for
/// that collapse in both directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum SandboxMode {
    ReadOnly,
    #[default]
    WorkspaceWrite,
    DangerFullAccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ApprovalPolicy {
    Untrusted,
    #[default]
    OnRequest,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DefaultMode {
    Default,
    Plan,
    AcceptEdits,
    Auto,
    DontAsk,
    BypassPermissions,
}

impl SandboxMode {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
        }
    }

    pub(super) fn from_config(s: &str) -> Option<Self> {
        match s {
            "read-only" => Some(Self::ReadOnly),
            "workspace-write" => Some(Self::WorkspaceWrite),
            "danger-full-access" => Some(Self::DangerFullAccess),
            _ => None,
        }
    }
}

impl ApprovalPolicy {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Untrusted => "untrusted",
            Self::OnRequest => "on-request",
            Self::Never => "never",
        }
    }

    pub(super) fn from_config(s: &str) -> Option<Self> {
        match s {
            "untrusted" => Some(Self::Untrusted),
            "on-request" => Some(Self::OnRequest),
            "never" => Some(Self::Never),
            _ => None,
        }
    }
}

impl DefaultMode {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Plan => "plan",
            Self::AcceptEdits => "acceptEdits",
            Self::Auto => "auto",
            Self::DontAsk => "dontAsk",
            Self::BypassPermissions => "bypassPermissions",
        }
    }

    pub(super) fn from_config(s: &str) -> Option<Self> {
        match s {
            "default" => Some(Self::Default),
            "plan" => Some(Self::Plan),
            "acceptEdits" => Some(Self::AcceptEdits),
            "auto" => Some(Self::Auto),
            "dontAsk" => Some(Self::DontAsk),
            "bypassPermissions" => Some(Self::BypassPermissions),
            _ => None,
        }
    }

    /// Maps a Claude `defaultMode` to the closest pair of Codex axes (c2x direction).
    ///
    /// Both axes are always produced; neither is ever silently omitted.
    pub(super) fn to_codex(self) -> (ApprovalPolicy, SandboxMode) {
        match self {
            // Standard interactive mode: ask before acting, workspace boundary.
            Self::Default => (ApprovalPolicy::OnRequest, SandboxMode::WorkspaceWrite),
            // Read-only sandbox is the closest approximation; no true plan mode in Codex.
            Self::Plan => (ApprovalPolicy::OnRequest, SandboxMode::ReadOnly),
            // Auto-approves edits but keeps the workspace sandbox boundary.
            Self::AcceptEdits => (ApprovalPolicy::OnRequest, SandboxMode::WorkspaceWrite),
            // Fully automatic within the workspace sandbox boundary.
            Self::Auto => (ApprovalPolicy::OnRequest, SandboxMode::WorkspaceWrite),
            // Suppresses prompts but retains the workspace sandbox; not danger-full-access.
            Self::DontAsk => (ApprovalPolicy::Never, SandboxMode::WorkspaceWrite),
            // Removes both the approval gate and the sandbox entirely.
            Self::BypassPermissions => (ApprovalPolicy::Never, SandboxMode::DangerFullAccess),
        }
    }

    /// Joint reverse mapping from Codex's two axes back to a single Claude `defaultMode` (x2c direction).
    ///
    /// The sandbox axis dominates: read-only and danger-full-access each have an
    /// unambiguous Claude counterpart regardless of what the approval axis says.
    /// Only in the workspace-write case does the approval value break the tie.
    pub(super) fn from_codex(approval: ApprovalPolicy, sandbox: SandboxMode) -> Self {
        match sandbox {
            SandboxMode::ReadOnly => Self::Plan,
            SandboxMode::DangerFullAccess => Self::BypassPermissions,
            SandboxMode::WorkspaceWrite => match approval {
                ApprovalPolicy::Never => Self::DontAsk,
                ApprovalPolicy::Untrusted | ApprovalPolicy::OnRequest => Self::Default,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── to_codex: every DefaultMode variant ─────────────────────────────────

    #[test]
    fn default_to_codex() {
        assert_eq!(
            DefaultMode::Default.to_codex(),
            (ApprovalPolicy::OnRequest, SandboxMode::WorkspaceWrite)
        );
    }

    #[test]
    fn plan_to_codex() {
        assert_eq!(
            DefaultMode::Plan.to_codex(),
            (ApprovalPolicy::OnRequest, SandboxMode::ReadOnly)
        );
    }

    #[test]
    fn accept_edits_to_codex() {
        assert_eq!(
            DefaultMode::AcceptEdits.to_codex(),
            (ApprovalPolicy::OnRequest, SandboxMode::WorkspaceWrite)
        );
    }

    #[test]
    fn auto_to_codex() {
        assert_eq!(
            DefaultMode::Auto.to_codex(),
            (ApprovalPolicy::OnRequest, SandboxMode::WorkspaceWrite)
        );
    }

    #[test]
    fn dont_ask_to_codex() {
        assert_eq!(
            DefaultMode::DontAsk.to_codex(),
            (ApprovalPolicy::Never, SandboxMode::WorkspaceWrite)
        );
    }

    #[test]
    fn bypass_permissions_to_codex() {
        assert_eq!(
            DefaultMode::BypassPermissions.to_codex(),
            (ApprovalPolicy::Never, SandboxMode::DangerFullAccess)
        );
    }

    // ── from_codex: representative cells ────────────────────────────────────

    #[test]
    fn read_only_any_approval_gives_plan() {
        assert_eq!(
            DefaultMode::from_codex(ApprovalPolicy::OnRequest, SandboxMode::ReadOnly),
            DefaultMode::Plan
        );
        assert_eq!(
            DefaultMode::from_codex(ApprovalPolicy::Never, SandboxMode::ReadOnly),
            DefaultMode::Plan
        );
        assert_eq!(
            DefaultMode::from_codex(ApprovalPolicy::Untrusted, SandboxMode::ReadOnly),
            DefaultMode::Plan
        );
    }

    #[test]
    fn danger_full_access_gives_bypass_permissions() {
        assert_eq!(
            DefaultMode::from_codex(ApprovalPolicy::Never, SandboxMode::DangerFullAccess),
            DefaultMode::BypassPermissions
        );
        assert_eq!(
            DefaultMode::from_codex(ApprovalPolicy::OnRequest, SandboxMode::DangerFullAccess),
            DefaultMode::BypassPermissions
        );
    }

    #[test]
    fn workspace_write_never_gives_dont_ask() {
        assert_eq!(
            DefaultMode::from_codex(ApprovalPolicy::Never, SandboxMode::WorkspaceWrite),
            DefaultMode::DontAsk
        );
    }

    #[test]
    fn workspace_write_on_request_gives_default() {
        assert_eq!(
            DefaultMode::from_codex(ApprovalPolicy::OnRequest, SandboxMode::WorkspaceWrite),
            DefaultMode::Default
        );
    }

    #[test]
    fn workspace_write_untrusted_gives_default() {
        assert_eq!(
            DefaultMode::from_codex(ApprovalPolicy::Untrusted, SandboxMode::WorkspaceWrite),
            DefaultMode::Default
        );
    }

    // ── Default trait (Codex-documented defaults) ────────────────────────────

    #[test]
    fn default_approval_policy_is_on_request() {
        assert_eq!(ApprovalPolicy::default(), ApprovalPolicy::OnRequest);
    }

    #[test]
    fn default_sandbox_mode_is_workspace_write() {
        assert_eq!(SandboxMode::default(), SandboxMode::WorkspaceWrite);
    }

    #[test]
    fn default_fill_missing_axes_gives_default_mode() {
        // Both axes missing → use Codex defaults → workspace-write + on-request → Default
        let mode = DefaultMode::from_codex(ApprovalPolicy::default(), SandboxMode::default());
        assert_eq!(mode, DefaultMode::Default);
    }

    // ── as_str / from_config round-trips ────────────────────────────────────

    #[test]
    fn sandbox_mode_round_trip() {
        for v in [
            SandboxMode::ReadOnly,
            SandboxMode::WorkspaceWrite,
            SandboxMode::DangerFullAccess,
        ] {
            assert_eq!(SandboxMode::from_config(v.as_str()), Some(v));
        }
    }

    #[test]
    fn approval_policy_round_trip() {
        for v in [
            ApprovalPolicy::Untrusted,
            ApprovalPolicy::OnRequest,
            ApprovalPolicy::Never,
        ] {
            assert_eq!(ApprovalPolicy::from_config(v.as_str()), Some(v));
        }
    }

    #[test]
    fn default_mode_round_trip() {
        for v in [
            DefaultMode::Default,
            DefaultMode::Plan,
            DefaultMode::AcceptEdits,
            DefaultMode::Auto,
            DefaultMode::DontAsk,
            DefaultMode::BypassPermissions,
        ] {
            assert_eq!(DefaultMode::from_config(v.as_str()), Some(v));
        }
    }

    #[test]
    fn from_config_unknown_returns_none() {
        assert_eq!(SandboxMode::from_config("unknown"), None);
        assert_eq!(ApprovalPolicy::from_config("unknown"), None);
        assert_eq!(DefaultMode::from_config("unknown"), None);
    }
}
