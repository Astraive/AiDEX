//! Provider-native MCP configuration. The run credential is only delivered through environment.
use crate::{Driver, RuntimeConfig, state::error};
use aidex_native_sessions::{RuntimeError, RuntimeRequest};
use aidex_sandbox::SandboxRequest;
use std::path::Path;
use zeroize::Zeroizing;
pub(crate) const AUTH_ENV: &str = "AIDEX_RUNTIME_BRIDGE_AUTHORIZATION";
pub(crate) fn supported(driver: Driver) -> bool {
    matches!(driver, Driver::Codex | Driver::ClaudeCode)
}
pub(crate) fn inject(
    config: &RuntimeConfig,
    r: &RuntimeRequest,
    scratch: &Path,
    request: &mut SandboxRequest,
    args: &mut Vec<String>,
) -> Result<Vec<Zeroizing<String>>, RuntimeError> {
    let Some(bridge) = &r.bridge else {
        return Ok(vec![]);
    };
    if !supported(config.driver) {
        return Err(error(
            "bridge_unsupported",
            "This adapter has no verified native MCP bridge configuration",
        ));
    }
    let header = Zeroizing::new(bridge.authorization_header());
    if !header.starts_with("Bearer ") || header.len() > 8192 || header.chars().any(char::is_control)
    {
        return Err(error(
            "invalid_request",
            "Invalid runtime bridge credential",
        ));
    }
    request.env.push((AUTH_ENV.into(), header.to_string()));
    match config.driver {
        Driver::Codex => {
            let url = serde_json::to_string(bridge.endpoint())
                .map_err(|_| error("invalid_request", "Invalid runtime bridge endpoint"))?;
            args.extend(["-c".into(),format!("mcp_servers.aidex_tools.url={url}"),"-c".into(),format!("mcp_servers.aidex_tools.env_http_headers.Authorization=\"{AUTH_ENV}\""),"-c".into(),"mcp_servers.aidex_tools.required=true".into(),"-c".into(),"mcp_servers.aidex_tools.tool_timeout_sec=180".into(),"-c".into(),format!("shell_environment_policy.exclude=[\"{AUTH_ENV}\",\"ACCESS_TOKEN\",\"OPENAI_API_KEY\",\"CODEX_API_KEY\"]")]);
        }
        Driver::ClaudeCode => {
            // https://code.claude.com/docs/en/mcp (retrieved 2026-10-11, lines 499-504):
            // each server's `timeout` is milliseconds and overrides MCP_TOOL_TIMEOUT.
            // Outside the native home collected for encrypted recovery. It contains only an env reference.
            let path = scratch.join("aidex-mcp.json");
            let value = serde_json::json!({"mcpServers":{"aidex_tools":{"type":"http","url":bridge.endpoint(),"headers":{"Authorization":format!("${{{AUTH_ENV}}}")},"timeout":180000}}});
            std::fs::write(
                &path,
                serde_json::to_vec(&value).map_err(|_| {
                    error(
                        "state_unavailable",
                        "Bridge configuration cannot be encoded",
                    )
                })?,
            )
            .map_err(|_| {
                error(
                    "state_unavailable",
                    "Bridge configuration cannot be written",
                )
            })?;
            args.extend([
                "--strict-mcp-config".into(),
                "--mcp-config".into(),
                path.to_string_lossy().into_owned(),
            ]);
        }
        _ => unreachable!(),
    }
    Ok(vec![
        Zeroizing::new(header.strip_prefix("Bearer ").unwrap().to_owned()),
        header,
    ])
}
