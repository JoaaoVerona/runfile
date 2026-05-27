use super::*;

// ── :env subcommand parsing ──────────────────────────────────────

#[test]
fn env_has_subcommands() {
	let cmd = Cli::command();
	let env = find_subcommand(&cmd, ":env");
	let names: Vec<&str> = env.get_subcommands().map(|s| s.get_name()).collect();

	assert!(names.contains(&"init"), "missing env init");
	assert!(names.contains(&"inject"), "missing env inject");
	assert!(names.contains(&"secret-keys"), "missing env secret-keys");
	assert!(names.contains(&"get"), "missing env get");
	assert!(names.contains(&"set"), "missing env set");
	assert!(names.contains(&"decrypt"), "missing env decrypt");
	assert!(names.contains(&"encrypt"), "missing env encrypt");
}

#[test]
fn cli_parses_env_inject_default_file() {
	let cli = Cli::try_parse_from(["run", ":env", "inject", "--", "echo", "hello"]).unwrap();
	match cli.subcommand {
		Some(crate::Commands::Env {
			action: crate::EnvAction::Inject { file, command },
		}) => {
			assert!(file.is_empty());
			assert_eq!(command, vec!["echo".to_string(), "hello".to_string()]);
		}
		_ => panic!("expected Env Inject"),
	}
}

#[test]
fn cli_parses_env_inject_with_files() {
	let cli = Cli::try_parse_from(["run", ":env", "inject", ".env", ".env.local", "--", "node", "app.js"]).unwrap();
	match cli.subcommand {
		Some(crate::Commands::Env {
			action: crate::EnvAction::Inject { file, command },
		}) => {
			assert_eq!(file, vec![".env".to_string(), ".env.local".to_string()]);
			assert_eq!(command, vec!["node".to_string(), "app.js".to_string()]);
		}
		_ => panic!("expected Env Inject"),
	}
}

#[test]
fn cli_parses_env_inject_with_command_flags_after_dashdash() {
	// After `--`, hyphen-prefixed args belong to the command, not to runfile
	let cli = Cli::try_parse_from(["run", ":env", "inject", "--", "node", "--version"]).unwrap();
	match cli.subcommand {
		Some(crate::Commands::Env {
			action: crate::EnvAction::Inject { command, .. },
		}) => {
			assert_eq!(command, vec!["node".to_string(), "--version".to_string()]);
		}
		_ => panic!("expected Env Inject"),
	}
}

#[test]
fn cli_rejects_env_inject_without_command() {
	assert!(try_parse(&["run", ":env", "inject"]).is_err());
	// File positional without `--` followed by a command is still incomplete:
	// the parser needs at least one arg after `--`.
	assert!(try_parse(&["run", ":env", "inject", ".env"]).is_err());
}

#[test]
fn cli_parses_env_init_defaults() {
	let cli = Cli::try_parse_from(["run", ":env", "init"]).unwrap();
	match cli.subcommand {
		Some(crate::Commands::Env {
			action: crate::EnvAction::Init { path, plain, key },
		}) => {
			assert_eq!(path, ".env");
			assert!(!plain);
			assert!(key.is_none());
		}
		_ => panic!("expected Env Init"),
	}
}

#[test]
fn cli_parses_env_init_with_path() {
	let cli = Cli::try_parse_from(["run", ":env", "init", ".env.production"]).unwrap();
	match cli.subcommand {
		Some(crate::Commands::Env {
			action: crate::EnvAction::Init { path, .. },
		}) => {
			assert_eq!(path, ".env.production");
		}
		_ => panic!("expected Env Init"),
	}
}

#[test]
fn cli_parses_env_init_plain() {
	let cli = Cli::try_parse_from(["run", ":env", "init", "--plain"]).unwrap();
	match cli.subcommand {
		Some(crate::Commands::Env {
			action: crate::EnvAction::Init { plain, key, .. },
		}) => {
			assert!(plain);
			assert!(key.is_none());
		}
		_ => panic!("expected Env Init"),
	}
}

#[test]
fn cli_parses_env_init_with_key() {
	let cli = Cli::try_parse_from(["run", ":env", "init", "--key", "abc123"]).unwrap();
	match cli.subcommand {
		Some(crate::Commands::Env {
			action: crate::EnvAction::Init { plain, key, .. },
		}) => {
			assert!(!plain);
			assert_eq!(key.as_deref(), Some("abc123"));
		}
		_ => panic!("expected Env Init"),
	}
}

#[test]
fn cli_parses_env_secret_keys_get_private() {
	let cli = Cli::try_parse_from(["run", ":env", "secret-keys", "get-private", "a1b2"]).unwrap();
	match cli.subcommand {
		Some(crate::Commands::Env {
			action: crate::EnvAction::SecretKeys {
				action: crate::SecretKeysAction::GetPrivate { partial },
			},
		}) => {
			assert_eq!(partial, "a1b2");
		}
		_ => panic!("expected Env SecretKeys GetPrivate"),
	}
}

#[test]
fn cli_rejects_env_secret_keys_get_private_without_arg() {
	assert!(try_parse(&["run", ":env", "secret-keys", "get-private"]).is_err());
}

#[test]
fn cli_parses_env_set_plain() {
	let cli = Cli::try_parse_from(["run", ":env", "set", "file", "VAR", "val", "--plain"]).unwrap();
	match cli.subcommand {
		Some(crate::Commands::Env {
			action: crate::EnvAction::Set {
				file,
				var,
				value,
				plain,
			},
		}) => {
			assert_eq!(file, "file");
			assert_eq!(var, "VAR");
			assert_eq!(value.as_deref(), Some("val"));
			assert!(plain);
		}
		_ => panic!("expected Env Set"),
	}
}

#[test]
fn cli_parses_env_set_without_value() {
	// When VALUE is omitted, it parses as None and the runtime reads from stdin.
	let cli = Cli::try_parse_from(["run", ":env", "set", "file", "VAR"]).unwrap();
	match cli.subcommand {
		Some(crate::Commands::Env {
			action: crate::EnvAction::Set {
				file,
				var,
				value,
				plain,
			},
		}) => {
			assert_eq!(file, "file");
			assert_eq!(var, "VAR");
			assert_eq!(value, None);
			assert!(!plain);
		}
		_ => panic!("expected Env Set"),
	}
}
