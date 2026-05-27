use super::*;

// ── Install snippet tests ─────────────────────────────────────────

#[test]
fn config_snippet_without_runfile_path() {
	let snippet = mcp_config_snippet(None);
	let obj = snippet.as_object().unwrap();
	assert_eq!(obj.get("command").unwrap(), "run");
	let args = obj.get("args").unwrap().as_array().unwrap();
	assert_eq!(args.len(), 1);
	assert_eq!(args[0], ":mcp");
}

#[test]
fn config_snippet_with_runfile_path() {
	let snippet = mcp_config_snippet(Some("ci/Runfile.json"));
	let obj = snippet.as_object().unwrap();
	assert_eq!(obj.get("command").unwrap(), "run");
	let args = obj.get("args").unwrap().as_array().unwrap();
	assert_eq!(args.len(), 3);
	assert_eq!(args[0], "-f");
	assert_eq!(args[1], "ci/Runfile.json");
	assert_eq!(args[2], ":mcp");
}

#[test]
fn install_no_agent_returns_instructions() {
	let dir = tempfile::TempDir::new().unwrap();
	let result = install_for_agent(None, None, dir.path());
	assert!(matches!(result, InstallResult::Instructions(_)));
	if let InstallResult::Instructions(text) = result {
		assert!(text.contains("claude-code"));
		assert!(text.contains("cursor"));
	}
}

#[test]
fn install_unknown_agent_returns_instructions() {
	let dir = tempfile::TempDir::new().unwrap();
	let result = install_for_agent(Some("unknown-agent"), None, dir.path());
	assert!(matches!(result, InstallResult::Instructions(_)));
	if let InstallResult::Instructions(text) = result {
		assert!(text.contains("unknown-agent"));
	}
}

#[test]
fn install_claude_desktop_returns_instructions() {
	let dir = tempfile::TempDir::new().unwrap();
	let result = install_for_agent(Some("claude-desktop"), None, dir.path());
	assert!(matches!(result, InstallResult::Instructions(_)));
	if let InstallResult::Instructions(text) = result {
		assert!(text.contains("Claude Desktop"));
	}
}

#[test]
fn install_claude_code_writes_config() {
	let dir = tempfile::TempDir::new().unwrap();
	let result = install_for_agent(Some("claude-code"), None, dir.path());

	assert!(matches!(result, InstallResult::Installed { .. }));
	let config_path = dir.path().join(".claude/settings.local.json");
	assert!(config_path.is_file());
	let contents: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
	assert!(contents.get("mcpServers").unwrap().get("runfile").is_some());
}

#[test]
fn install_cursor_writes_config() {
	let dir = tempfile::TempDir::new().unwrap();
	let result = install_for_agent(Some("cursor"), None, dir.path());

	assert!(matches!(result, InstallResult::Installed { .. }));
	let config_path = dir.path().join(".cursor/mcp.json");
	assert!(config_path.is_file());
	let contents: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
	assert!(contents.get("mcpServers").unwrap().get("runfile").is_some());
}

#[test]
fn install_claude_code_updates_existing() {
	let dir = tempfile::TempDir::new().unwrap();

	// First install
	install_for_agent(Some("claude-code"), None, dir.path());

	// Second install should be an update
	let result = install_for_agent(Some("claude-code"), None, dir.path());
	assert!(matches!(result, InstallResult::Updated { .. }));
}

#[test]
fn install_preserves_existing_config_fields() {
	let dir = tempfile::TempDir::new().unwrap();
	let claude_dir = dir.path().join(".claude");
	std::fs::create_dir_all(&claude_dir).unwrap();
	let config_path = claude_dir.join("settings.local.json");

	// Write existing config with other fields
	let existing = serde_json::json!({
		"otherSetting": true,
		"mcpServers": {
			"other-server": { "command": "other" }
		}
	});
	std::fs::write(&config_path, serde_json::to_string_pretty(&existing).unwrap()).unwrap();

	install_for_agent(Some("claude-code"), None, dir.path());

	let contents: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
	// Our entry was added
	assert!(contents.get("mcpServers").unwrap().get("runfile").is_some());
	// Existing entries preserved
	assert!(contents.get("mcpServers").unwrap().get("other-server").is_some());
	assert_eq!(contents.get("otherSetting").unwrap(), true);
}
