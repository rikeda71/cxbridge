mod common;
use common::*;

use std::path::Path;

use cxbridge::core::{
    detect::detect, ir::Kind, mappings::load_mappings, report::build_report, transforms::ConvDir,
};
use cxbridge::handlers::{pick_handler, LowerOpts, Scope, SkillTargetMode};

/// Convert .mcp.json via c2x and verify that basic conversion works correctly.
#[test]
fn test_mcp_c2x_basic() {
    let mcp_path = "tests/fixtures/claude/.mcp.json";
    assert!(
        Path::new(mcp_path).exists(),
        "Fixture {} must exist",
        mcp_path
    );

    let maps = load_mappings();
    let kind = detect(mcp_path).expect("detect should succeed");
    assert_eq!(kind, cxbridge::core::ir::Kind::Mcp);

    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(mcp_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    assert_eq!(ir.children.len(), 4, "Expected 4 MCP server children");

    // Verify timeout conversion for the filesystem server
    let fs_server = ir.children.iter().find(|c| c.source_path == "filesystem");
    assert!(fs_server.is_some(), "Expected 'filesystem' server");
    let fs = fs_server.unwrap();
    let timeout = fs.fields.get("mcp.timeout");
    assert!(timeout.is_some(), "Expected timeout field");
    // 30000ms → 30.0 sec
    assert_eq!(
        timeout.unwrap().value.as_f64().unwrap(),
        30.0,
        "Expected timeout converted to 30.0 sec"
    );

    // Verify Bearer token extraction for api-server
    let api_server = ir.children.iter().find(|c| c.source_path == "api-server");
    assert!(api_server.is_some(), "Expected 'api-server'");
    let api = api_server.unwrap();
    let bearer = api.fields.get("mcp.bearer");
    assert!(bearer.is_some(), "Expected bearer field");
    assert_eq!(
        bearer.unwrap().value.as_str().unwrap(),
        "API_TOKEN",
        "Expected bearer_token_env_var=API_TOKEN"
    );
}

/// Dropped/lossy fields are enumerated in the report after .mcp.json c2x conversion.
#[test]
fn test_mcp_c2x_report_dropped() {
    let mcp_path = "tests/fixtures/claude/.mcp.json";

    let maps = load_mappings();
    let kind = detect(mcp_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(mcp_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let report = build_report(&ir, &empty_plan());

    // alwaysLoad is Claude-specific and should be dropped (unknown field or dropped)
    // alwaysLoad on disabled-server produces a Drop diagnostic as an unknown field
    let total_drops = report.dropped.len();
    assert!(
        total_drops >= 1,
        "Expected at least 1 dropped entry, got {}",
        total_drops
    );
}

/// Files are generated after .mcp.json c2x lower.
#[test]
fn test_mcp_c2x_lower_generates_files() {
    let mcp_path = "tests/fixtures/claude/.mcp.json";

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings();
    let kind = detect(mcp_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(mcp_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let opts = default_lower_opts_subagent(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    assert!(
        !plan.files.is_empty(),
        "Expected at least one generated file"
    );
    let mcp_file = plan.files.iter().find(|f| f.path.ends_with(".mcp.json"));
    assert!(mcp_file.is_some(), "Expected .mcp.json in output");
    let content: serde_json::Value = serde_json::from_str(&mcp_file.unwrap().content).unwrap();
    assert!(
        content["mcpServers"].is_object(),
        "Expected mcpServers object"
    );
}

/// x2c conversion test for Codex config.toml.
#[test]
fn test_mcp_x2c_from_codex_config() {
    let config_path = "tests/fixtures/codex/config.toml";
    assert!(
        Path::new(config_path).exists(),
        "Fixture {} must exist",
        config_path
    );

    let maps = load_mappings();
    let kind = detect(config_path).expect("detect should succeed");
    assert_eq!(
        kind,
        cxbridge::core::ir::Kind::Mcp,
        "config.toml with mcp_servers should be Kind::Mcp"
    );

    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(config_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    // filesystem and api-server are converted (disabled-server has enabled=false)
    assert!(ir.children.len() >= 2, "Expected at least 2 children");

    // filesystem server: timeout is converted
    let fs = ir.children.iter().find(|c| c.source_path == "filesystem");
    assert!(fs.is_some(), "Expected filesystem server");
    let fs = fs.unwrap();
    // tool_timeout_sec=30.0 → timeout=30000
    if let Some(timeout) = fs.fields.get("mcp.timeout") {
        assert_eq!(
            timeout.value.as_i64().unwrap_or(0),
            30000,
            "Expected timeout=30000ms"
        );
    }

    // Check whether disabled-server has its disabled flag set
    let disabled = ir
        .children
        .iter()
        .find(|c| c.source_path == "disabled-server");
    if let Some(d) = disabled {
        let has_disabled_flag = d.fields.contains_key("__disabled")
            || d.diagnostics
                .iter()
                .any(|diag| diag.message.contains("enabled=false"));
        assert!(
            has_disabled_flag,
            "Expected disabled-server to be marked disabled"
        );
    }
}

/// .mcp.json is generated after x2c conversion.
#[test]
fn test_mcp_x2c_lower_generates_claude_mcp_json() {
    let config_path = "tests/fixtures/codex/config.toml";

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings();
    let kind = detect(config_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(config_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    let opts = LowerOpts {
        out: Some(out_dir.path().to_str().unwrap().to_string()),
        only: vec![],
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: false,
    };
    let plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .expect("lower should succeed");

    let mcp_file = plan.files.iter().find(|f| f.path.ends_with(".mcp.json"));
    assert!(mcp_file.is_some(), "Expected .mcp.json in output");

    let content: serde_json::Value = serde_json::from_str(&mcp_file.unwrap().content).unwrap();
    let servers = content["mcpServers"]
        .as_object()
        .expect("mcpServers should be object");

    assert!(
        servers.contains_key("filesystem"),
        "Expected filesystem server in .mcp.json"
    );

    // Servers with enabled=false must not appear in output.
    assert!(
        !servers.contains_key("disabled-server"),
        "disabled-server should be excluded from .mcp.json"
    );
}

/// c2x: .mcp.json with oauth sub-object must produce IR fields for
/// mcp.oauth.client_id (lossless), mcp.oauth.scopes (lossless, split by space),
/// mcp.oauth.callback_port (lossy, warn), and mcp.oauth.auth_server_metadata_url
/// (dropped + warn). The oauth-server fixture already contains clientId and scopes.
#[test]
fn test_mcp_c2x_oauth_fields_in_ir() {
    let mcp_path = "tests/fixtures/claude/.mcp.json";

    let maps = load_mappings();
    let kind = detect(mcp_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler.parse(Path::new(mcp_path)).expect("parse ok");
    let ir = handler.lift(&parsed, ConvDir::C2x).expect("lift ok");

    let oauth_child = ir
        .children
        .iter()
        .find(|c| c.source_path == "oauth-server")
        .expect("Expected 'oauth-server' child");

    // mcp.oauth.client_id must be present and lossless
    let client_id = oauth_child
        .fields
        .get("mcp.oauth.client_id")
        .expect("Expected mcp.oauth.client_id in IR");
    assert_eq!(
        client_id.value,
        serde_json::Value::String("my-client-id".to_string()),
        "client_id value mismatch"
    );
    assert_eq!(
        client_id.loss,
        cxbridge::core::ir::Loss::Lossless,
        "mcp.oauth.client_id must be lossless"
    );

    // mcp.oauth.scopes must be present, lossless, and split into an array
    let scopes = oauth_child
        .fields
        .get("mcp.oauth.scopes")
        .expect("Expected mcp.oauth.scopes in IR");
    assert_eq!(
        scopes.loss,
        cxbridge::core::ir::Loss::Lossless,
        "mcp.oauth.scopes must be lossless"
    );
    let scopes_arr = scopes
        .value
        .as_array()
        .expect("mcp.oauth.scopes must be array after str_to_list:space");
    assert_eq!(
        scopes_arr,
        &vec![
            serde_json::Value::String("read".to_string()),
            serde_json::Value::String("write".to_string()),
            serde_json::Value::String("admin".to_string()),
        ],
        "scopes must be split by whitespace"
    );

    // No unknown-field diagnostic for oauth
    let has_unknown_oauth_diag = oauth_child
        .diagnostics
        .iter()
        .any(|d| d.message.contains("unknown MCP server field: oauth"));
    assert!(
        !has_unknown_oauth_diag,
        "oauth object must NOT produce 'unknown MCP server field: oauth' diagnostic"
    );
}

/// c2x lower: oauth-server must produce a Codex .mcp.json with oauth.client_id
/// and scopes array present.
#[test]
fn test_mcp_c2x_oauth_lower_output() {
    let mcp_path = "tests/fixtures/claude/.mcp.json";
    let out_dir = tempfile::TempDir::new().unwrap();

    let maps = load_mappings();
    let kind = detect(mcp_path).expect("detect ok");
    let handler = pick_handler(&kind, maps);
    let parsed = handler.parse(Path::new(mcp_path)).expect("parse ok");
    let ir = handler.lift(&parsed, ConvDir::C2x).expect("lift ok");

    let opts = default_lower_opts_subagent(out_dir.path().to_str().unwrap());
    let plan = handler.lower(&ir, ConvDir::C2x, &opts).expect("lower ok");

    let mcp_file = plan
        .files
        .iter()
        .find(|f| f.path.ends_with(".mcp.json"))
        .expect("Expected .mcp.json output");
    let content: serde_json::Value =
        serde_json::from_str(&mcp_file.content).expect("output must be valid JSON");

    let oauth_server = &content["mcpServers"]["oauth-server"];
    assert!(
        oauth_server.is_object(),
        "Expected oauth-server in mcpServers"
    );

    // oauth.client_id must be present (renamed from clientId)
    let client_id = &oauth_server["oauth"]["client_id"];
    assert_eq!(
        client_id,
        &serde_json::Value::String("my-client-id".to_string()),
        "oauth.client_id must be present in Codex output"
    );

    // oauth.scopes must be an array
    let scopes = &oauth_server["oauth"]["scopes"];
    assert!(
        scopes.is_array(),
        "oauth.scopes must be array in Codex output"
    );
    let scopes_arr = scopes.as_array().unwrap();
    assert_eq!(scopes_arr.len(), 3, "Expected 3 scopes");
}

/// x2c: Codex config.toml with [mcp_servers.oauth-server.oauth] must produce IR
/// fields for mcp.oauth.client_id (lossless) and mcp.oauth.scopes (lossless,
/// joined to space-separated string).
#[test]
fn test_mcp_x2c_oauth_fields_in_ir() {
    let config_path = "tests/fixtures/codex/config.toml";

    let maps = load_mappings();
    let kind = detect(config_path).expect("detect ok");
    let handler = pick_handler(&kind, maps);
    let parsed = handler.parse(Path::new(config_path)).expect("parse ok");
    let ir = handler.lift(&parsed, ConvDir::X2c).expect("lift ok");

    let oauth_child = ir
        .children
        .iter()
        .find(|c| c.source_path == "oauth-server")
        .expect("Expected 'oauth-server' child in x2c IR");

    // mcp.oauth.client_id must be lossless
    let client_id = oauth_child
        .fields
        .get("mcp.oauth.client_id")
        .expect("Expected mcp.oauth.client_id in x2c IR");
    assert_eq!(
        client_id.value,
        serde_json::Value::String("my-client-id".to_string()),
        "client_id value mismatch in x2c"
    );
    assert_eq!(
        client_id.loss,
        cxbridge::core::ir::Loss::Lossless,
        "mcp.oauth.client_id must be lossless in x2c"
    );

    // mcp.oauth.scopes must be lossless and joined to a space-separated string
    let scopes = oauth_child
        .fields
        .get("mcp.oauth.scopes")
        .expect("Expected mcp.oauth.scopes in x2c IR");
    assert_eq!(
        scopes.loss,
        cxbridge::core::ir::Loss::Lossless,
        "mcp.oauth.scopes must be lossless in x2c"
    );
    assert_eq!(
        scopes.value,
        serde_json::Value::String("read write admin".to_string()),
        "scopes must be joined to space-separated string in x2c"
    );
}

/// x2c lower: oauth-server must produce a Claude .mcp.json with
/// oauth.clientId and oauth.scopes (space-separated string).
#[test]
fn test_mcp_x2c_oauth_lower_output() {
    let config_path = "tests/fixtures/codex/config.toml";
    let out_dir = tempfile::TempDir::new().unwrap();

    let maps = load_mappings();
    let kind = detect(config_path).expect("detect ok");
    let handler = pick_handler(&kind, maps);
    let parsed = handler.parse(Path::new(config_path)).expect("parse ok");
    let ir = handler.lift(&parsed, ConvDir::X2c).expect("lift ok");

    let opts = LowerOpts {
        out: Some(out_dir.path().to_str().unwrap().to_string()),
        only: vec![],
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: false,
    };
    let plan = handler.lower(&ir, ConvDir::X2c, &opts).expect("lower ok");

    let mcp_file = plan
        .files
        .iter()
        .find(|f| f.path.ends_with(".mcp.json"))
        .expect("Expected .mcp.json output in x2c");
    let content: serde_json::Value =
        serde_json::from_str(&mcp_file.content).expect("output must be valid JSON");

    let oauth_server = &content["mcpServers"]["oauth-server"];
    assert!(
        oauth_server.is_object(),
        "Expected oauth-server in mcpServers (x2c)"
    );

    // oauth.clientId must be present (renamed from client_id)
    let client_id = &oauth_server["oauth"]["clientId"];
    assert_eq!(
        client_id,
        &serde_json::Value::String("my-client-id".to_string()),
        "oauth.clientId must be present in Claude output"
    );

    // oauth.scopes must be a space-separated string
    let scopes = &oauth_server["oauth"]["scopes"];
    assert_eq!(
        scopes,
        &serde_json::Value::String("read write admin".to_string()),
        "oauth.scopes must be space-separated string in Claude output"
    );
}

/// c2x lift: headers with ${VAR} form must produce env_http_headers with bare
/// variable name (no '$' prefix).
#[test]
fn test_mcp_c2x_env_http_headers_bare_var_name_in_ir() {
    // Use only non-Bearer env-var headers to test the env_http_headers path
    let mcp_json = serde_json::json!({
        "frontmatter": {
            "mcpServers": {
                "s": {
                    "type": "http",
                    "url": "https://example.com/mcp",
                    "headers": {
                        "X-Api-Key": "${API_KEY}",
                        "X-Tenant": "${TENANT_ID}"
                    }
                }
            }
        },
        "body": ""
    });

    let maps = load_mappings();
    let handler = cxbridge::handlers::mcp::McpHandler {
        map: maps["mcp"].clone(),
    };

    use cxbridge::handlers::Handler;
    let ir = handler.lift(&mcp_json, ConvDir::C2x).unwrap();

    let server = ir.children.iter().find(|c| c.source_path == "s").unwrap();

    // X-Api-Key: "${API_KEY}" must be in env_http_headers with bare var name
    let env_hdr = server
        .fields
        .get("mcp.env_http_headers")
        .expect("mcp.env_http_headers must be present");
    let hdr_obj = env_hdr
        .value
        .as_object()
        .expect("env_http_headers must be an object");
    let api_key_val = hdr_obj.get("X-Api-Key").expect("X-Api-Key must be present");
    assert_eq!(
        api_key_val,
        &serde_json::Value::String("API_KEY".to_string()),
        "env_http_headers value must be bare var name 'API_KEY', not '$API_KEY'"
    );
    let tenant_val = hdr_obj.get("X-Tenant").expect("X-Tenant must be present");
    assert_eq!(
        tenant_val,
        &serde_json::Value::String("TENANT_ID".to_string()),
        "env_http_headers value must be bare var name 'TENANT_ID', not '$TENANT_ID'"
    );
}

/// c2x lower: headers with ${VAR} form must produce env_http_headers with bare
/// variable name (no '$' prefix) in the emitted Codex .mcp.json.
#[test]
fn test_mcp_c2x_env_http_headers_bare_var_name_in_output() {
    let mcp_path = "tests/fixtures/claude/env_http_headers_project/.mcp.json";
    assert!(
        Path::new(mcp_path).exists(),
        "Fixture {} must exist",
        mcp_path
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings();
    let kind = detect(mcp_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler.parse(Path::new(mcp_path)).expect("parse ok");
    let ir = handler.lift(&parsed, ConvDir::C2x).expect("lift ok");

    let opts = default_lower_opts_subagent(out_dir.path().to_str().unwrap());
    let plan = handler.lower(&ir, ConvDir::C2x, &opts).expect("lower ok");

    let mcp_file = plan
        .files
        .iter()
        .find(|f| f.path.ends_with(".mcp.json"))
        .expect("Expected .mcp.json output");
    let content: serde_json::Value =
        serde_json::from_str(&mcp_file.content).expect("output must be valid JSON");

    let server = &content["mcpServers"]["env-header-server"];
    let env_http = &server["env_http_headers"];
    assert!(env_http.is_object(), "Expected env_http_headers object");

    let x_api_key = &env_http["X-Api-Key"];
    assert_eq!(
        x_api_key,
        &serde_json::Value::String("API_KEY".to_string()),
        "env_http_headers['X-Api-Key'] must be bare 'API_KEY', not '$API_KEY'"
    );
}

/// c2x lift: http transport env with ${VAR} form must produce env_http_headers
/// with bare variable name (no '$' prefix).
#[test]
fn test_mcp_c2x_env_to_env_http_headers_bare_var_name() {
    let mcp_json = serde_json::json!({
        "frontmatter": {
            "mcpServers": {
                "http-env-server": {
                    "type": "http",
                    "url": "https://example.com/mcp",
                    "env": {
                        "X-Service-Key": "${SERVICE_KEY}"
                    }
                }
            }
        },
        "body": ""
    });

    let maps = load_mappings();
    let handler = cxbridge::handlers::mcp::McpHandler {
        map: maps["mcp"].clone(),
    };

    use cxbridge::handlers::Handler;
    let ir = handler.lift(&mcp_json, ConvDir::C2x).unwrap();

    let server = ir
        .children
        .iter()
        .find(|c| c.source_path == "http-env-server")
        .unwrap();

    let env_hdr = server
        .fields
        .get("mcp.env_http_headers")
        .expect("mcp.env_http_headers must be present for http transport env");
    let hdr_obj = env_hdr
        .value
        .as_object()
        .expect("env_http_headers must be object");
    let val = hdr_obj
        .get("X-Service-Key")
        .expect("X-Service-Key must be present");
    assert_eq!(
        val,
        &serde_json::Value::String("SERVICE_KEY".to_string()),
        "env_http_headers value must be bare 'SERVICE_KEY', not '$SERVICE_KEY'"
    );
}

/// x2c lift: env_http_headers with bare var name must produce Claude headers
/// with ${VAR} form.
#[test]
fn test_mcp_x2c_env_http_headers_becomes_dollar_brace_in_headers() {
    // Codex parsed structure: env_http_headers values are bare var names
    let parsed = serde_json::json!({
        "frontmatter": {
            "mcp_servers": {
                "env-header-server": {
                    "url": "https://api.example.com/mcp",
                    "env_http_headers": {
                        "X-Api-Key": "API_KEY",
                        "X-Tenant": "TENANT_ID"
                    }
                }
            }
        },
        "body": ""
    });

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings();
    let handler = cxbridge::handlers::mcp::McpHandler {
        map: maps["mcp"].clone(),
    };

    use cxbridge::handlers::Handler;
    let ir = handler.lift(&parsed, ConvDir::X2c).expect("lift ok");

    let server = ir
        .children
        .iter()
        .find(|c| c.source_path == "env-header-server")
        .expect("Expected 'env-header-server' child");

    let env_hdr = server
        .fields
        .get("mcp.env_http_headers")
        .expect("mcp.env_http_headers must be present in x2c IR");
    let hdr_obj = env_hdr
        .value
        .as_object()
        .expect("env_http_headers must be object");
    let x_api_key = hdr_obj.get("X-Api-Key").expect("X-Api-Key must be present");
    assert_eq!(
        x_api_key,
        &serde_json::Value::String("API_KEY".to_string()),
        "x2c IR must preserve bare var name in env_http_headers"
    );

    // Lower to Claude and check headers become ${VAR} form
    let opts = LowerOpts {
        out: Some(out_dir.path().to_str().unwrap().to_string()),
        only: vec![],
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: false,
    };
    let plan = handler.lower(&ir, ConvDir::X2c, &opts).expect("lower ok");

    let mcp_file = plan
        .files
        .iter()
        .find(|f| f.path.ends_with(".mcp.json"))
        .expect("Expected .mcp.json output");
    let content: serde_json::Value =
        serde_json::from_str(&mcp_file.content).expect("output must be valid JSON");

    let server_cfg = &content["mcpServers"]["env-header-server"];
    let headers = &server_cfg["headers"];
    assert!(
        headers.is_object(),
        "Expected headers object in Claude .mcp.json"
    );

    let x_api_key_header = &headers["X-Api-Key"];
    assert_eq!(
        x_api_key_header,
        &serde_json::Value::String("${API_KEY}".to_string()),
        "x2c lower must convert bare 'API_KEY' to '${{API_KEY}}' in Claude headers"
    );
}

/// gap 7/42: x2c e2e — Codex config.toml with env_http_headers must produce
/// Claude .mcp.json headers with ${VAR} form (not bare var names).
///
/// Drives the full pipeline: parse from fixture file → lift → lower → assert
/// output file content.
#[test]
fn test_mcp_x2c_env_http_headers_e2e_dollar_brace_wrapping() {
    let config_path = "tests/fixtures/codex/env_http_headers_project/config.toml";
    assert!(
        Path::new(config_path).exists(),
        "Fixture {} must exist",
        config_path
    );

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings();
    let kind = detect(config_path).expect("detect should succeed");
    let handler = pick_handler(&kind, maps);
    let parsed = handler
        .parse(Path::new(config_path))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    let opts = LowerOpts {
        out: Some(out_dir.path().to_str().unwrap().to_string()),
        only: vec![],
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: false,
    };
    let plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .expect("lower should succeed");

    let mcp_file = plan
        .files
        .iter()
        .find(|f| f.path.ends_with(".mcp.json"))
        .expect("Expected .mcp.json output");
    let content: serde_json::Value =
        serde_json::from_str(&mcp_file.content).expect("output must be valid JSON");

    let headers = &content["mcpServers"]["auth-server"]["headers"];
    assert!(
        headers.is_object(),
        "Expected headers object in Claude .mcp.json, got: {headers}"
    );

    let authorization = &headers["Authorization"];
    assert_eq!(
        authorization,
        &serde_json::Value::String("${MY_AUTH_TOKEN}".to_string()),
        "env_http_headers 'MY_AUTH_TOKEN' must become '${{MY_AUTH_TOKEN}}' in Claude headers, got: {authorization}"
    );

    let x_custom = &headers["X-Custom"];
    assert_eq!(
        x_custom,
        &serde_json::Value::String("${MY_API_KEY}".to_string()),
        "env_http_headers 'MY_API_KEY' must become '${{MY_API_KEY}}' in Claude headers, got: {x_custom}"
    );
}

/// c2x lift: when an http server has both headers (with ${VAR}) and env (with
/// ${VAR}), env_http_headers in the IR must contain entries from BOTH sources.
/// The headers-derived entry must not be silently overwritten.
#[test]
fn test_mcp_c2x_env_http_headers_merged_when_both_headers_and_env() {
    let mcp_json = serde_json::json!({
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

    let maps = load_mappings();
    let handler = cxbridge::handlers::mcp::McpHandler {
        map: maps["mcp"].clone(),
    };

    use cxbridge::handlers::Handler;
    let ir = handler.lift(&mcp_json, ConvDir::C2x).unwrap();
    let server = ir.children.iter().find(|c| c.source_path == "s").unwrap();

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
        "X-From-Headers (from headers) must be in merged env_http_headers, got: {:?}",
        hdr_obj
    );
    assert_eq!(
        hdr_obj["X-From-Headers"],
        serde_json::Value::String("FROM_HEADERS".to_string()),
        "X-From-Headers value must be bare var name"
    );
    assert!(
        hdr_obj.contains_key("API_KEY"),
        "API_KEY (from env) must be in merged env_http_headers, got: {:?}",
        hdr_obj
    );
    assert_eq!(
        hdr_obj["API_KEY"],
        serde_json::Value::String("API_KEY".to_string()),
        "API_KEY value must be bare var name"
    );
}

/// c2x lower: when an http server has both headers and env, the emitted
/// env_http_headers must contain entries from both sources (no silent drop).
#[test]
fn test_mcp_c2x_env_http_headers_merged_in_output() {
    let mcp_json = serde_json::json!({
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

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings();
    let handler = cxbridge::handlers::mcp::McpHandler {
        map: maps["mcp"].clone(),
    };

    use cxbridge::handlers::Handler;
    let ir = handler.lift(&mcp_json, ConvDir::C2x).unwrap();

    let opts = LowerOpts {
        out: Some(out_dir.path().to_str().unwrap().to_string()),
        only: vec![],
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: false,
    };
    let plan = handler.lower(&ir, ConvDir::C2x, &opts).unwrap();

    let mcp_file = plan
        .files
        .iter()
        .find(|f| f.path.ends_with(".mcp.json"))
        .expect("Expected .mcp.json output");
    let content: serde_json::Value = serde_json::from_str(&mcp_file.content).unwrap();

    let env_http = &content["mcpServers"]["s"]["env_http_headers"];
    assert!(
        env_http.is_object(),
        "Expected env_http_headers object in output"
    );
    assert!(
        env_http["X-From-Headers"] == serde_json::Value::String("FROM_HEADERS".to_string()),
        "X-From-Headers must be present in merged output, got: {:?}",
        env_http
    );
    assert!(
        env_http["API_KEY"] == serde_json::Value::String("API_KEY".to_string()),
        "API_KEY must be present in merged output, got: {:?}",
        env_http
    );

    // Report must reflect 0 unexpected drops
    let report = build_report(&ir, &plan);
    let drop_ids: Vec<_> = report
        .dropped
        .iter()
        .filter_map(|d| d.id.as_deref())
        .collect();
    assert!(
        !drop_ids.contains(&"mcp.env_http_headers"),
        "mcp.env_http_headers must not appear as dropped, got: {:?}",
        drop_ids
    );
}

/// c2x lift: when Authorization is "Bearer ${TOKEN}", the bearer env var must be
/// extracted, non-Authorization headers with ${VAR} values must go to
/// env_http_headers, and literal-value headers must go to http_headers with a
/// Warn diagnostic.
#[test]
fn test_mcp_c2x_bearer_auth_remaining_var_headers_routed_to_env_http_headers() {
    let mcp_json = serde_json::json!({
        "frontmatter": {
            "mcpServers": {
                "s": {
                    "type": "http",
                    "url": "https://x.com",
                    "headers": {
                        "Authorization": "Bearer ${TOKEN}",
                        "X-Api-Key": "${API_KEY}",
                        "X-Static": "literal"
                    }
                }
            }
        },
        "body": ""
    });

    let maps = load_mappings();
    let handler = cxbridge::handlers::mcp::McpHandler {
        map: maps["mcp"].clone(),
    };

    use cxbridge::handlers::Handler;
    let ir = handler.lift(&mcp_json, ConvDir::C2x).unwrap();
    let server = ir.children.iter().find(|c| c.source_path == "s").unwrap();

    // bearer_token_env_var must be extracted as "TOKEN"
    let bearer = server
        .fields
        .get("mcp.bearer")
        .expect("mcp.bearer must be present when Authorization is Bearer ${TOKEN}");
    assert_eq!(
        bearer.value,
        serde_json::Value::String("TOKEN".to_string()),
        "bearer_token_env_var must be bare var name 'TOKEN'"
    );

    // X-Api-Key: "${API_KEY}" must be in env_http_headers with bare var name "API_KEY"
    let env_hdr = server
        .fields
        .get("mcp.env_http_headers")
        .expect("mcp.env_http_headers must be present for ${VAR} headers alongside Bearer auth");
    let hdr_obj = env_hdr
        .value
        .as_object()
        .expect("mcp.env_http_headers must be an object");
    assert!(
        hdr_obj.contains_key("X-Api-Key"),
        "X-Api-Key (${{VAR}} value) must be in env_http_headers, not http_headers. got: {:?}",
        hdr_obj
    );
    assert_eq!(
        hdr_obj["X-Api-Key"],
        serde_json::Value::String("API_KEY".to_string()),
        "env_http_headers['X-Api-Key'] must be bare var name 'API_KEY'"
    );

    // X-Static: "literal" must NOT be in env_http_headers
    assert!(
        !hdr_obj.contains_key("X-Static"),
        "X-Static (literal value) must not be in env_http_headers, got: {:?}",
        hdr_obj
    );

    // X-Static must be in mcp.headers (http_headers)
    let http_hdr = server
        .fields
        .get("mcp.headers")
        .expect("mcp.headers must be present for literal-value headers alongside Bearer auth");
    let http_obj = http_hdr
        .value
        .as_object()
        .expect("mcp.headers must be an object");
    assert!(
        http_obj.contains_key("X-Static"),
        "X-Static (literal value) must be in mcp.headers (http_headers), got: {:?}",
        http_obj
    );

    // There must be a Warn diagnostic for X-Static
    let has_static_warn = server
        .diagnostics
        .iter()
        .any(|d| d.level == cxbridge::core::ir::DiagLevel::Warn && d.message.contains("X-Static"));
    assert!(
        has_static_warn,
        "Expected a Warn diagnostic for literal-value header X-Static alongside Bearer auth, got: {:?}",
        server.diagnostics
    );
}

/// c2x lower: when Authorization is "Bearer ${TOKEN}", the Codex .mcp.json must
/// have bearer_token_env_var="TOKEN", env_http_headers={"X-Api-Key": "API_KEY"},
/// and http_headers={"X-Static": "literal"}.
#[test]
fn test_mcp_c2x_bearer_auth_remaining_var_headers_in_output() {
    let mcp_json = serde_json::json!({
        "frontmatter": {
            "mcpServers": {
                "s": {
                    "type": "http",
                    "url": "https://x.com",
                    "headers": {
                        "Authorization": "Bearer ${TOKEN}",
                        "X-Api-Key": "${API_KEY}",
                        "X-Static": "literal"
                    }
                }
            }
        },
        "body": ""
    });

    let out_dir = tempfile::TempDir::new().unwrap();
    let maps = load_mappings();
    let handler = cxbridge::handlers::mcp::McpHandler {
        map: maps["mcp"].clone(),
    };

    use cxbridge::handlers::Handler;
    let ir = handler.lift(&mcp_json, ConvDir::C2x).unwrap();

    let opts = LowerOpts {
        out: Some(out_dir.path().to_str().unwrap().to_string()),
        only: vec![],
        scope: Scope::Project,
        dual_manifest: false,
        hooks_target: Scope::User,
        skill_target: SkillTargetMode::Skill,
        interactive: false,
        rewrite_body: false,
        keep_claude_frontmatter: false,
    };
    let plan = handler.lower(&ir, ConvDir::C2x, &opts).unwrap();

    let mcp_file = plan
        .files
        .iter()
        .find(|f| f.path.ends_with(".mcp.json"))
        .expect("Expected .mcp.json output");
    let content: serde_json::Value = serde_json::from_str(&mcp_file.content).unwrap();

    let server = &content["mcpServers"]["s"];

    // bearer_token_env_var must be "TOKEN"
    assert_eq!(
        server["bearer_token_env_var"],
        serde_json::Value::String("TOKEN".to_string()),
        "bearer_token_env_var must be 'TOKEN'"
    );

    // env_http_headers must contain X-Api-Key → API_KEY
    let env_http = &server["env_http_headers"];
    assert!(env_http.is_object(), "Expected env_http_headers in output");
    assert_eq!(
        env_http["X-Api-Key"],
        serde_json::Value::String("API_KEY".to_string()),
        "env_http_headers['X-Api-Key'] must be 'API_KEY'"
    );

    // http_headers must exist and contain X-Static → "literal"
    assert!(
        server["http_headers"].is_object(),
        "http_headers must be present and be an object in output"
    );
    assert_eq!(
        server["http_headers"]["X-Static"],
        serde_json::Value::String("literal".to_string()),
        "http_headers['X-Static'] must be 'literal'"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// From mcp_disabled_sentinel_leak.rs
// ────────────────────────────────────────────────────────────────────────────

const DISABLED_FIXTURE: &str = "tests/fixtures/codex/mcp_disabled_server/config.toml";

/// After lift, the child IRNode for a disabled server must have NO field
/// with id "__disabled".  The only record of the disabled state must be
/// a Drop diagnostic with id "mcp.enabled".
#[test]
fn test_disabled_server_ir_has_no_disabled_sentinel_field() {
    let maps = load_mappings();
    let handler = pick_handler(&Kind::Mcp, maps);

    let parsed = handler
        .parse(Path::new(DISABLED_FIXTURE))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    // The fixture has exactly one server (s) with enabled=false.
    assert_eq!(ir.children.len(), 1, "fixture must have exactly one server");
    let child = &ir.children[0];

    // No IRField with id "__disabled" must be present.
    assert!(
        !child.fields.contains_key("__disabled"),
        "child.fields must NOT contain a '__disabled' sentinel field; \
         found fields: {:?}",
        child.fields.keys().collect::<Vec<_>>()
    );

    // The disabled state must be recorded as a Drop diagnostic with id "mcp.enabled".
    let has_enabled_diag = child.diagnostics.iter().any(|d| {
        d.id.as_deref() == Some("mcp.enabled") && d.level == cxbridge::core::ir::DiagLevel::Drop
    });
    assert!(
        has_enabled_diag,
        "child.diagnostics must contain a Drop diagnostic with id 'mcp.enabled'; \
         found diagnostics: {:?}",
        child
            .diagnostics
            .iter()
            .map(|d| (d.level.clone(), d.id.as_deref().unwrap_or("<none>")))
            .collect::<Vec<_>>()
    );
}

/// The full pipeline report for an enabled=false server must contain exactly
/// one dropped entry with id "mcp.enabled" and zero entries with id "__disabled".
#[test]
fn test_disabled_server_report_shows_mcp_enabled_not_disabled_sentinel() {
    let maps = load_mappings();
    let handler = pick_handler(&Kind::Mcp, maps);

    let parsed = handler
        .parse(Path::new(DISABLED_FIXTURE))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .expect("lower should succeed");

    let report = build_report(&ir, &plan);

    // The "__disabled" sentinel must never appear in the user-facing dropped list.
    let disabled_sentinel_count = report
        .dropped
        .iter()
        .filter(|e| e.id.as_deref() == Some("__disabled"))
        .count();
    assert_eq!(
        disabled_sentinel_count,
        0,
        "'__disabled' is an internal sentinel and must NOT appear in report.dropped; \
         found {} occurrence(s). Full dropped: {:?}",
        disabled_sentinel_count,
        report
            .dropped
            .iter()
            .map(|e| (e.id.as_deref().unwrap_or("<none>"), e.message.as_str()))
            .collect::<Vec<_>>()
    );

    // "mcp.enabled" must appear exactly once.
    let enabled_count = report
        .dropped
        .iter()
        .filter(|e| e.id.as_deref() == Some("mcp.enabled"))
        .count();
    assert_eq!(
        enabled_count,
        1,
        "'mcp.enabled' must appear exactly once in report.dropped; \
         found {} occurrence(s). Full dropped: {:?}",
        enabled_count,
        report
            .dropped
            .iter()
            .map(|e| (e.id.as_deref().unwrap_or("<none>"), e.message.as_str()))
            .collect::<Vec<_>>()
    );
}

/// The total dropped count for a single disabled server must be exactly 1.
/// Before the fix it was 3: one from __disabled IRField, one from ir.diagnostics,
/// one from plan.diagnostics.
#[test]
fn test_disabled_server_total_dropped_exactly_one() {
    let maps = load_mappings();
    let handler = pick_handler(&Kind::Mcp, maps);

    let parsed = handler
        .parse(Path::new(DISABLED_FIXTURE))
        .expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .expect("lower should succeed");

    let report = build_report(&ir, &plan);

    assert_eq!(
        report.dropped.len(),
        1,
        "A single disabled server must produce exactly 1 dropped entry total; \
         found {}. Full dropped: {:?}",
        report.dropped.len(),
        report
            .dropped
            .iter()
            .map(|e| (e.id.as_deref().unwrap_or("<none>"), e.message.as_str()))
            .collect::<Vec<_>>()
    );
}

// ────────────────────────────────────────────────────────────────────────────
// From mcp_dropped_warn_dedup.rs
// ────────────────────────────────────────────────────────────────────────────

const DROPPED_WARN_FIXTURE: &str = "tests/fixtures/claude/mcp_dropped_warn_fields/.mcp.json";
const X2C_DISABLED_FIXTURE: &str = "tests/fixtures/codex/mcp_disabled_server/config.toml";

/// alwaysLoad (loss:dropped + warn:true) must appear exactly once in
/// report.dropped and must not appear in report.lossy.
#[test]
fn test_mcp_c2x_always_load_dropped_once_not_in_lossy() {
    let fixture = Path::new(DROPPED_WARN_FIXTURE);
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings();
    let handler = pick_handler(&Kind::Mcp, maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let report = build_report(&ir, &plan);

    // alwaysLoad must appear exactly once in dropped.
    let always_load_count = report
        .dropped
        .iter()
        .filter(|e| e.id.as_deref() == Some("mcp.alwaysLoad"))
        .count();
    assert_eq!(
        always_load_count,
        1,
        "mcp.alwaysLoad must appear exactly once in report.dropped, found {} times. \
         Full dropped: {:?}",
        always_load_count,
        report
            .dropped
            .iter()
            .map(|e| (e.id.as_deref().unwrap_or("<none>"), e.message.as_str()))
            .collect::<Vec<_>>()
    );

    // alwaysLoad must not appear in lossy.
    let in_lossy = report
        .lossy
        .iter()
        .any(|e| e.id.as_deref() == Some("mcp.alwaysLoad"));
    assert!(
        !in_lossy,
        "mcp.alwaysLoad must NOT appear in report.lossy; lossy: {:?}",
        report
            .lossy
            .iter()
            .map(|e| e.id.as_deref().unwrap_or("<none>"))
            .collect::<Vec<_>>()
    );
}

/// headersHelper (loss:dropped + warn:true) must appear exactly once in
/// report.dropped and must not appear in report.lossy.
#[test]
fn test_mcp_c2x_headers_helper_dropped_once_not_in_lossy() {
    let fixture = Path::new(DROPPED_WARN_FIXTURE);
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings();
    let handler = pick_handler(&Kind::Mcp, maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let report = build_report(&ir, &plan);

    // headersHelper must appear exactly once in dropped.
    let headers_helper_count = report
        .dropped
        .iter()
        .filter(|e| e.id.as_deref() == Some("mcp.headersHelper"))
        .count();
    assert_eq!(
        headers_helper_count,
        1,
        "mcp.headersHelper must appear exactly once in report.dropped, found {} times. \
         Full dropped: {:?}",
        headers_helper_count,
        report
            .dropped
            .iter()
            .map(|e| (e.id.as_deref().unwrap_or("<none>"), e.message.as_str()))
            .collect::<Vec<_>>()
    );

    // headersHelper must not appear in lossy.
    let in_lossy = report
        .lossy
        .iter()
        .any(|e| e.id.as_deref() == Some("mcp.headersHelper"));
    assert!(
        !in_lossy,
        "mcp.headersHelper must NOT appear in report.lossy; lossy: {:?}",
        report
            .lossy
            .iter()
            .map(|e| e.id.as_deref().unwrap_or("<none>"))
            .collect::<Vec<_>>()
    );
}

/// The full report summary for the minimal fixture (one server with
/// alwaysLoad + headersHelper) must show exactly 2 dropped entries —
/// one per field, each appearing once.
#[test]
fn test_mcp_c2x_summary_counts_two_dropped_not_six() {
    let fixture = Path::new(DROPPED_WARN_FIXTURE);
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings();
    let handler = pick_handler(&Kind::Mcp, maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::C2x)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::C2x, &opts)
        .expect("lower should succeed");

    let report = build_report(&ir, &plan);

    // The fixture has exactly 2 dropped fields: mcp.alwaysLoad and mcp.headersHelper.
    // The known dropped field IDs (ignoring any transport-type or mcp.format entries).
    let known_dropped_ids = ["mcp.alwaysLoad", "mcp.headersHelper"];
    let known_dropped_count = report
        .dropped
        .iter()
        .filter(|e| {
            e.id.as_deref()
                .map(|id| known_dropped_ids.contains(&id))
                .unwrap_or(false)
        })
        .count();

    assert_eq!(
        known_dropped_count,
        2,
        "Expected exactly 2 dropped entries for the 2 known dropped fields, \
         found {}. Full dropped: {:?}",
        known_dropped_count,
        report
            .dropped
            .iter()
            .map(|e| (e.id.as_deref().unwrap_or("<none>"), e.message.as_str()))
            .collect::<Vec<_>>()
    );
}

/// x2c: a disabled server (enabled=false) must produce exactly one
/// dropped entry for mcp.enabled in the report.  Before the fix, it appeared
/// twice — once from ir.diagnostics (pushed by lift) and once from
/// plan.diagnostics (pushed by lower_x2c) — plus a third __disabled entry
/// from ir.fields, yielding "3 dropped".
#[test]
fn test_mcp_x2c_enabled_false_dropped_once() {
    let fixture = Path::new(X2C_DISABLED_FIXTURE);
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings();
    let handler = pick_handler(&Kind::Mcp, maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .expect("lower should succeed");

    let report = build_report(&ir, &plan);

    // mcp.enabled must appear exactly once in dropped.
    let enabled_count = report
        .dropped
        .iter()
        .filter(|e| e.id.as_deref() == Some("mcp.enabled"))
        .count();
    assert_eq!(
        enabled_count,
        1,
        "mcp.enabled must appear exactly once in report.dropped, found {} times. \
         Full dropped: {:?}",
        enabled_count,
        report
            .dropped
            .iter()
            .map(|e| (e.id.as_deref().unwrap_or("<none>"), e.message.as_str()))
            .collect::<Vec<_>>()
    );

    // __disabled is an internal bookkeeping key and must not surface in the report.
    let disabled_in_dropped = report
        .dropped
        .iter()
        .any(|e| e.id.as_deref() == Some("__disabled"));
    assert!(
        !disabled_in_dropped,
        "__disabled must not appear in report.dropped (it is an internal field); \
         Full dropped: {:?}",
        report
            .dropped
            .iter()
            .map(|e| (e.id.as_deref().unwrap_or("<none>"), e.message.as_str()))
            .collect::<Vec<_>>()
    );
}

/// x2c: the total dropped count for a single disabled server must be exactly 1,
/// not 3 (the pre-fix value).
#[test]
fn test_mcp_x2c_enabled_false_total_dropped_is_one() {
    let fixture = Path::new(X2C_DISABLED_FIXTURE);
    assert!(fixture.exists(), "Fixture {} must exist", fixture.display());

    let maps = load_mappings();
    let handler = pick_handler(&Kind::Mcp, maps);

    let parsed = handler.parse(fixture).expect("parse should succeed");
    let ir = handler
        .lift(&parsed, ConvDir::X2c)
        .expect("lift should succeed");

    let out_dir = tempfile::TempDir::new().unwrap();
    let opts = default_lower_opts(out_dir.path().to_str().unwrap());
    let plan = handler
        .lower(&ir, ConvDir::X2c, &opts)
        .expect("lower should succeed");

    let report = build_report(&ir, &plan);

    assert_eq!(
        report.dropped.len(),
        1,
        "A single disabled server must produce exactly 1 dropped entry, \
         found {}. Full dropped: {:?}",
        report.dropped.len(),
        report
            .dropped
            .iter()
            .map(|e| (e.id.as_deref().unwrap_or("<none>"), e.message.as_str()))
            .collect::<Vec<_>>()
    );
}
