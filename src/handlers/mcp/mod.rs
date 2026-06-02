use std::path::Path;

use serde_json::Value;

use crate::core::ir::{new_node, Kind, Tool};
use crate::core::mappings::DomainMap;
use crate::core::transforms::ConvDir;
use crate::handlers::{EmitPlan, Handler, LowerOpts};

mod lift;
mod lower;
mod parse;

use parse::parse_toml_mcp_config;

/// Handler for the MCP domain.
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
            // config.toml is parsed as TOML
            parse_toml_mcp_config(path)
        } else {
            // .mcp.json is parsed as JSON
            crate::core::serialize::json::parse_json_file(path)
        }
    }

    fn lift(&self, parsed: &Value, dir: ConvDir) -> anyhow::Result<crate::core::ir::IRNode> {
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

    fn lower(
        &self,
        ir: &crate::core::ir::IRNode,
        dir: ConvDir,
        opts: &LowerOpts,
    ) -> anyhow::Result<EmitPlan> {
        match dir {
            ConvDir::C2x => self.lower_c2x(ir, opts),
            ConvDir::X2c => self.lower_x2c(ir, opts),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ir::{DiagLevel, Loss};
    use crate::core::mappings::load_mappings;
    use crate::core::transforms::ConvDir;
    use parse::{extract_bearer_env_var, extract_env_var_ref};
    use std::path::Path;
    use tempfile::TempDir;

    fn make_handler() -> McpHandler {
        let maps = load_mappings();
        McpHandler {
            map: maps["mcp"].clone(),
        }
    }

    fn default_opts() -> LowerOpts {
        LowerOpts {
            out: None,
            only: vec![],
            scope: crate::handlers::Scope::Project,
            dual_manifest: false,
            hooks_target: crate::handlers::Scope::User,
            skill_target: crate::handlers::SkillTargetMode::Skill,
            interactive: false,
            rewrite_body: false,
            keep_claude_frontmatter: false,
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
    fn test_mcp_lift_c2x_sse_ws_dropped_under_own_ids() {
        let dir = TempDir::new().unwrap();
        let mcp_path = dir.path().join(".mcp.json");
        std::fs::write(
            &mcp_path,
            r#"{
  "mcpServers": {
    "sse-server": {"type": "sse", "url": "https://example.com/sse"},
    "ws-server": {"type": "ws", "url": "wss://example.com/ws"}
  }
}"#,
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&mcp_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();

        let sse = ir
            .children
            .iter()
            .find(|c| c.source_path == "sse-server")
            .unwrap();
        // The drop is attributed to its own mapping id, not mcp.transport_type.
        assert!(sse.fields.contains_key("mcp.transport_sse"));
        assert!(!sse.fields.contains_key("mcp.transport_type"));
        assert_eq!(sse.fields["mcp.transport_sse"].loss, Loss::Dropped);

        let ws = ir
            .children
            .iter()
            .find(|c| c.source_path == "ws-server")
            .unwrap();
        assert!(ws.fields.contains_key("mcp.transport_ws"));
        assert_eq!(ws.fields["mcp.transport_ws"].loss, Loss::Dropped);
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

        // Verify that .mcp.json was generated.
        // c2x converts to Codex, so .mcp.json is the output format.
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

        // alwaysLoad, headersHelper are Claude-specific, so either Drop diagnostics
        // are emitted or they are dropped as unknown fields.
        let child = &ir.children[0];
        let drop_diags: Vec<_> = child
            .diagnostics
            .iter()
            .filter(|d| d.level == DiagLevel::Drop)
            .collect();
        // alwaysLoad and headersHelper are either unknown MCP fields or dropped
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

    // gap 5/42: OAuth nested fields silently dropped

    /// c2x: oauth sub-object must produce mcp.oauth.client_id (lossless),
    /// mcp.oauth.scopes (lossless, array), mcp.oauth.callback_port (lossy),
    /// and mcp.oauth.auth_server_metadata_url (dropped+warn).
    #[test]
    fn test_mcp_lift_c2x_oauth_roundtrip() {
        let dir = TempDir::new().unwrap();
        let mcp_path = dir.path().join(".mcp.json");
        std::fs::write(
            &mcp_path,
            r#"{
  "mcpServers": {
    "s": {
      "type": "http",
      "url": "https://x.com",
      "oauth": {
        "clientId": "id",
        "scopes": "a:read b:write",
        "callbackPort": 9876,
        "authServerMetadataUrl": "https://auth.example.com"
      }
    }
  }
}"#,
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&mcp_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let child = &ir.children[0];

        // mcp.oauth.client_id: lossless
        let cid = child
            .fields
            .get("mcp.oauth.client_id")
            .expect("mcp.oauth.client_id must be in IR");
        assert_eq!(cid.value, Value::String("id".to_string()));
        assert!(matches!(cid.loss, Loss::Lossless));

        // mcp.oauth.scopes: lossless, array after str_to_list:space
        let scopes = child
            .fields
            .get("mcp.oauth.scopes")
            .expect("mcp.oauth.scopes must be in IR");
        assert!(matches!(scopes.loss, Loss::Lossless));
        let arr = scopes.value.as_array().expect("scopes must be array");
        assert_eq!(
            arr,
            &vec![
                Value::String("a:read".to_string()),
                Value::String("b:write".to_string()),
            ]
        );

        // mcp.oauth.callback_port: lossy
        let cp = child
            .fields
            .get("mcp.oauth.callback_port")
            .expect("mcp.oauth.callback_port must be in IR");
        assert!(matches!(cp.loss, Loss::Lossy));
        assert_eq!(cp.value, Value::Number(serde_json::Number::from(9876)));

        // mcp.oauth.auth_server_metadata_url: dropped
        // The field is represented via IRField.loss == Dropped; build_report reads
        // it from ir.fields.  No additional Diagnostic is pushed (doing so would
        // cause each dropped field to be counted multiple times in the summary).
        let asm = child
            .fields
            .get("mcp.oauth.auth_server_metadata_url")
            .expect("mcp.oauth.auth_server_metadata_url must be in IR");
        assert!(matches!(asm.loss, Loss::Dropped));
        // No spurious Diagnostic must be pushed for this dropped field.
        let has_spurious_diag = child
            .diagnostics
            .iter()
            .any(|d| d.id.as_deref() == Some("mcp.oauth.auth_server_metadata_url"));
        assert!(
            !has_spurious_diag,
            "mcp.oauth.auth_server_metadata_url must NOT push a redundant Diagnostic \
             (the IRField.dropped entry is the canonical source); diagnostics: {:?}",
            child.diagnostics
        );

        // no unknown-field diagnostic for oauth
        let has_unknown = child
            .diagnostics
            .iter()
            .any(|d| d.message.contains("unknown MCP server field: oauth"));
        assert!(
            !has_unknown,
            "oauth must not produce unknown-field diagnostic"
        );
    }

    /// c2x: server with both headers (${VAR}) and env (${VAR}) must merge both
    /// into a single env_http_headers IRField — no silent overwrite.
    #[test]
    fn test_lift_c2x_merges_headers_and_env_into_env_http_headers() {
        let parsed = serde_json::json!({
            "frontmatter": {
                "mcpServers": {
                    "s": {
                        "type": "http",
                        "url": "https://x.com",
                        "headers": { "X-From-Headers": "${FROM_HEADERS}" },
                        "env":     { "API_KEY": "${API_KEY}" }
                    }
                }
            },
            "body": ""
        });

        let h = make_handler();
        let ir = h.lift(&parsed, ConvDir::C2x).unwrap();
        let server = ir.children.iter().find(|c| c.source_path == "s").unwrap();

        // Only one env_http_headers field must exist (merged)
        let env_hdr = server
            .fields
            .get("mcp.env_http_headers")
            .expect("mcp.env_http_headers must be present");
        let hdr_obj = env_hdr
            .value
            .as_object()
            .expect("env_http_headers must be an object");

        assert!(
            hdr_obj.contains_key("X-From-Headers"),
            "headers-derived entry must survive merge: {:?}",
            hdr_obj
        );
        assert_eq!(
            hdr_obj["X-From-Headers"],
            Value::String("FROM_HEADERS".to_string())
        );
        assert!(
            hdr_obj.contains_key("API_KEY"),
            "env-derived entry must survive merge: {:?}",
            hdr_obj
        );
        assert_eq!(hdr_obj["API_KEY"], Value::String("API_KEY".to_string()));

        // mcp.env must NOT remain as a separate Lossless IRField for http transport
        // (it was fully consumed by env_http_headers, so it should not show lossless)
        if let Some(env_field) = server.fields.get("mcp.env") {
            assert!(
                !matches!(env_field.loss, Loss::Lossless),
                "mcp.env must not be Lossless for http transport (it was transformed)"
            );
        }
    }

    /// x2c: Codex config.toml with [oauth] sub-table must produce
    /// mcp.oauth.client_id (lossless) and mcp.oauth.scopes (lossless, joined string).
    #[test]
    fn test_mcp_lift_x2c_oauth() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"[mcp_servers.s]
url = "https://x.com"

[mcp_servers.s.oauth]
client_id = "id"
scopes = ["a:read", "b:write"]
"#,
        )
        .unwrap();

        let h = make_handler();
        let parsed = h.parse(&config_path).unwrap();
        let ir = h.lift(&parsed, ConvDir::X2c).unwrap();
        let child = &ir.children[0];

        // mcp.oauth.client_id: lossless
        let cid = child
            .fields
            .get("mcp.oauth.client_id")
            .expect("mcp.oauth.client_id must be in x2c IR");
        assert_eq!(cid.value, Value::String("id".to_string()));
        assert!(matches!(cid.loss, Loss::Lossless));

        // mcp.oauth.scopes: lossless, joined by space
        let scopes = child
            .fields
            .get("mcp.oauth.scopes")
            .expect("mcp.oauth.scopes must be in x2c IR");
        assert!(matches!(scopes.loss, Loss::Lossless));
        assert_eq!(
            scopes.value,
            Value::String("a:read b:write".to_string()),
            "scopes must be joined by space in x2c"
        );
    }
}
