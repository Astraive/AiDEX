//! The per-server configuration files and the `.mcp.json` importer.

mod support;

use std::fs;

use aidex_drive::DrivePaths;
use aidex_mcp::{
    MAX_MCP_JSON_BYTES, McpConfigStore, McpErrorKind, McpSource, McpSourceKind, TransportKind,
};
use support::stdio_config;

#[test]
fn servers_are_saved_one_file_each_and_validated_on_load() {
    let root = tempfile::tempdir().unwrap();
    aidex_drive::init_with(
        root.path(),
        aidex_drive::InitOptions {
            require_removable: false,
        },
    )
    .unwrap();
    let paths = DrivePaths::open(root.path()).unwrap();
    let store = McpConfigStore::open(&paths).unwrap();
    assert_eq!(store.dir(), paths.mcp());
    assert!(store.list().unwrap().servers.is_empty());

    let config = stdio_config("fixture", "normal");
    store.add(&config).unwrap();
    assert_eq!(store.add(&config).unwrap_err().kind, McpErrorKind::Conflict);
    let file = paths.mcp().join("fixture.toml");
    let text = fs::read_to_string(&file).unwrap();
    assert!(text.starts_with("version = 1\n"), "{text}");
    assert_eq!(store.get("fixture").unwrap(), config);

    let mut changed = config.clone();
    changed.name = "Renamed".into();
    store.update(&changed).unwrap();
    assert_eq!(store.get("fixture").unwrap().name, "Renamed");
    let mut absent = changed.clone();
    absent.id = "absent".into();
    assert_eq!(
        store.update(&absent).unwrap_err().kind,
        McpErrorKind::NotFound
    );
    assert!(!store.set_enabled("fixture", false).unwrap().enabled);
    assert!(store.set_enabled("fixture", true).unwrap().enabled);
    let mut invalid = changed.clone();
    invalid.command = None;
    assert_eq!(
        store.update(&invalid).unwrap_err().kind,
        McpErrorKind::InvalidConfig
    );

    // Files that fail to load are listed as invalid and left untouched.
    fs::write(paths.mcp().join("broken.toml"), "version = 1\nid = ").unwrap();
    fs::write(
        paths.mcp().join("renamed.toml"),
        fs::read_to_string(&file).unwrap(),
    )
    .unwrap();
    fs::write(
        paths.mcp().join("extra.toml"),
        "version = 1\nid = \"extra\"\nname = \"x\"\ntransport = \"stdio\"\ncommand = \"x\"\napi_key = \"value\"\n",
    )
    .unwrap();
    fs::write(paths.mcp().join("notes.txt"), "ignored").unwrap();
    let listing = store.list().unwrap();
    assert_eq!(listing.servers.len(), 1);
    let invalid: Vec<&str> = listing.invalid.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(invalid, ["broken", "extra", "renamed"]);
    assert!(listing.invalid[1].error.contains("api_key"));
    assert!(!listing.invalid[1].error.contains("value"));
    assert_eq!(
        store.get("renamed").unwrap_err().kind,
        McpErrorKind::InvalidConfig
    );
    assert_eq!(
        store.get("../escape").unwrap_err().kind,
        McpErrorKind::NotFound
    );

    assert!(store.remove("fixture").unwrap());
    assert!(!store.remove("fixture").unwrap());
    assert!(!store.remove("../escape").unwrap());
    assert!(!store.exists("fixture"));
}

#[test]
fn mcp_json_files_import_as_disabled_entries_without_values() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".mcp.json");
    let secret = support::sentinel();
    fs::write(
        &path,
        serde_json::json!({"mcpServers": {
            "filesystem": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "."]},
            "Linear Remote": {"type": "http", "url": "https://mcp.linear.app/mcp", "headers": {"Authorization": format!("Bearer {secret}")}},
            "tokens": {"command": "uvx", "args": ["tool"], "env": {"API_TOKEN": secret}},
            "bad": {"type": "websocket", "url": "wss://x.test"}
        }})
        .to_string(),
    )
    .unwrap();
    let imported =
        McpConfigStore::import_mcp_json(&path, McpSource::plugin("example-plugin")).unwrap();
    assert_eq!(imported.len(), 4);
    let text = serde_json::to_string(&imported).unwrap();
    assert!(!text.contains(&secret));
    for server in imported.iter().filter_map(|s| s.config.as_ref()) {
        assert!(!server.enabled);
        assert_eq!(server.source.kind, McpSourceKind::Plugin);
        assert_eq!(server.source.owner.as_deref(), Some("example-plugin"));
        server.validate().unwrap();
    }
    let linear = imported
        .iter()
        .find(|s| s.original_name == "Linear Remote")
        .unwrap();
    let config = linear.config.as_ref().unwrap();
    assert_eq!(config.id, "linear-remote");
    assert_eq!(config.transport, TransportKind::Http);
    assert_eq!(
        config.headers_from_secrets[0].secret_name,
        "mcp-linear-remote-authorization"
    );
    let tokens = imported
        .iter()
        .find(|s| s.original_name == "tokens")
        .unwrap();
    assert_eq!(tokens.dropped_env, ["API_TOKEN"]);
    assert!(
        imported
            .iter()
            .find(|s| s.original_name == "bad")
            .unwrap()
            .error
            .is_some()
    );
    // Nothing was written next to the file.
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);

    let missing =
        McpConfigStore::import_mcp_json(&dir.path().join("absent.json"), McpSource::drive())
            .unwrap_err();
    assert_eq!(missing.kind, McpErrorKind::NotFound);
    let large = dir.path().join("large.json");
    fs::write(&large, vec![b' '; MAX_MCP_JSON_BYTES as usize + 1]).unwrap();
    assert_eq!(
        McpConfigStore::import_mcp_json(&large, McpSource::drive())
            .unwrap_err()
            .kind,
        McpErrorKind::InvalidConfig
    );
    assert_eq!(
        McpConfigStore::import_mcp_json(dir.path(), McpSource::drive())
            .unwrap_err()
            .kind,
        McpErrorKind::InvalidConfig
    );
}
