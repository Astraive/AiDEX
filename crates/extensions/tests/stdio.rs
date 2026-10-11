#![cfg(windows)]
use aidex_drive::{DriveLayout, DrivePaths};
use aidex_extensions::*;
use aidex_tools::CancelToken;
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc};

#[test]
fn real_sandbox_stdio_initializes_paginates_and_calls() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DrivePaths::from_layout(dir.path().to_path_buf(), DriveLayout::default());
    let helper = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("aidex-sandbox-exec.exe");
    assert!(
        helper.exists(),
        "build aidex-sandbox-exec before running transport integration tests"
    );
    let connector = RealConnector::new(aidex_sandbox::Sandbox::new(helper), paths.tmp());
    let service =
        ExtensionService::with_connector(paths, Arc::new(NoSecrets), Arc::new(connector)).unwrap();
    // A fixture implementing actual MCP newline JSON-RPC, not a production connector.
    let script = r#"$ErrorActionPreference='Stop';while($null -ne ($line=[Console]::In.ReadLine())){$m=$line|ConvertFrom-Json;$result=$null;switch($m.method){'initialize'{$result=@{protocolVersion='2025-06-18';capabilities=@{tools=@{}};serverInfo=@{name='fixture';version='1'}}};'tools/list'{if($m.params.cursor){$result=@{tools=@(@{name='second';description='Second';inputSchema=@{type='object';properties=@{}}})}}else{$result=@{tools=@(@{name='echo';description='Echo';inputSchema=@{type='object';properties=@{text=@{type='string'}};required=@('text');additionalProperties=$false}});nextCursor='page2'}}};'tools/call'{$result=@{content=@(@{type='text';text=$m.params.arguments.text});isError=$false}}};if($null -ne $m.id){$response=@{jsonrpc='2.0';id=$m.id;result=$result};[Console]::Out.WriteLine(($response|ConvertTo-Json -Compress -Depth 20));[Console]::Out.Flush()}}"#;
    let config = McpServerConfig {
        id: "stdio-fixture".into(),
        name: "Fixture".into(),
        enabled: true,
        trusted: true,
        workspace_ids: vec!["workspace".into()],
        profiles: vec!["full_access".into()],
        transport: McpTransport::Stdio {
            command: "powershell.exe".into(),
            args: vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                script.into(),
            ],
            cwd: None,
        },
        secret_refs: BTreeMap::new(),
        timeout_ms: 10000,
        max_output_bytes: 65536,
    };
    service.upsert_mcp(config.clone()).unwrap();
    let scope = CallScope {
        workspace_id: "workspace".into(),
        workspace_root: dir.path().to_path_buf(),
        profile: "full_access".into(),
        full_access: true,
        network_allowed: true,
        selected_skill_ids: vec![],
    };
    let summary = service
        .connect_mcp(&config.id, &scope, &CancelToken::new())
        .unwrap();
    assert_eq!(summary.tools.len(), 2);
    let secrets: Arc<dyn ExtensionSecrets> = Arc::new(NoSecrets);
    let direct = RealConnector::new(
        aidex_sandbox::Sandbox::new(
            std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("aidex-sandbox-exec.exe"),
        ),
        dir.path().join("scratch"),
    );
    let mut connection = direct
        .connect(&config, &scope, secrets, &CancelToken::new())
        .unwrap();
    connection.initialize(&CancelToken::new()).unwrap();
    let result = connection
        .call_tool(
            "echo",
            json!({"text":"real MCP result"}),
            &CancelToken::new(),
        )
        .unwrap();
    assert_eq!(result["content"][0]["text"], "real MCP result");
    connection.disconnect();
    service.disconnect_mcp(&config.id).unwrap();
    let mut denied = scope;
    denied.full_access = false;
    assert!(
        service
            .connect_mcp(&config.id, &denied, &CancelToken::new())
            .is_err()
    );
}

#[test]
fn relocated_drive_runs_the_registered_script_from_its_new_root() {
    let parent = tempfile::tempdir().unwrap();
    let old_root = parent.path().join("drive_a");
    let new_root = parent.path().join("drive_b");
    std::fs::create_dir(&old_root).unwrap();
    let helper = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("aidex-sandbox-exec.exe");
    assert!(helper.exists());
    let old_paths = DrivePaths::from_layout(old_root.clone(), DriveLayout::default());
    let old_service = ExtensionService::with_connector(
        old_paths.clone(),
        Arc::new(NoSecrets),
        Arc::new(RealConnector::for_drive(
            aidex_sandbox::Sandbox::new(helper.clone()),
            &old_paths,
        )),
    )
    .unwrap();
    let script = r#"while($null -ne ($line=[Console]::In.ReadLine())){$m=$line|ConvertFrom-Json;$r=$null;switch($m.method){'initialize'{$r=@{protocolVersion='2025-06-18';capabilities=@{tools=@{}}}};'tools/list'{$r=@{tools=@(@{name='root';inputSchema=@{type='object';properties=@{}}})}};'tools/call'{$r=@{content=@(@{type='text';text=$PSScriptRoot})}}};if($null -ne $m.id){[Console]::Out.WriteLine((@{jsonrpc='2.0';id=$m.id;result=$r}|ConvertTo-Json -Compress -Depth 10));[Console]::Out.Flush()}}"#;
    let script_path = old_paths.mcp().join("fixture.ps1");
    std::fs::write(&script_path, script).unwrap();
    let config = McpServerConfig {
        id: "relocated".into(),
        name: "Relocation fixture".into(),
        enabled: true,
        trusted: true,
        workspace_ids: vec!["workspace".into()],
        profiles: vec!["full_access".into()],
        transport: McpTransport::Stdio {
            command: "powershell.exe".into(),
            args: vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-File".into(),
                script_path.to_string_lossy().into_owned(),
            ],
            cwd: Some(old_root.to_string_lossy().into_owned()),
        },
        secret_refs: BTreeMap::new(),
        timeout_ms: 10000,
        max_output_bytes: 65536,
    };
    old_service.upsert_mcp(config).unwrap();
    drop(old_service);
    std::fs::rename(&old_root, &new_root).unwrap();
    let new_paths = DrivePaths::from_layout(new_root.clone(), DriveLayout::default());
    let connector = Arc::new(RealConnector::for_drive(
        aidex_sandbox::Sandbox::new(helper),
        &new_paths,
    ));
    let service =
        ExtensionService::with_connector(new_paths.clone(), Arc::new(NoSecrets), connector.clone())
            .unwrap();
    let stored = service.list_mcp().unwrap().remove(0).config;
    let McpTransport::Stdio { args, .. } = &stored.transport else {
        panic!()
    };
    assert_eq!(args[4], "drive:mcp/fixture.ps1");
    let scope = CallScope {
        workspace_id: "workspace".into(),
        workspace_root: new_root,
        profile: "full_access".into(),
        full_access: true,
        network_allowed: true,
        selected_skill_ids: vec![],
    };
    let mut connection = connector
        .connect(&stored, &scope, Arc::new(NoSecrets), &CancelToken::new())
        .unwrap();
    connection.initialize(&CancelToken::new()).unwrap();
    connection.list_tools(&CancelToken::new()).unwrap();
    let result = connection
        .call_tool("root", json!({}), &CancelToken::new())
        .unwrap();
    assert_eq!(
        std::path::PathBuf::from(result["content"][0]["text"].as_str().unwrap()),
        new_paths.mcp()
    );
    connection.disconnect();
}
