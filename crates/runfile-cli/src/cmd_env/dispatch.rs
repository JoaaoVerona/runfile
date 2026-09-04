//! `run :env <subcommand>` argument routing.

use std::process::ExitCode;

use crate::help::{Row, Section};

const INTRO: &str = "run :env <command>   —   .env files, with per-value encryption";

const SECTIONS: &[Section] = &[Section(
	"Commands",
	&[
		Row("run :env init [path]", "create a .env, encrypted unless --plain"),
		Row("run :env get <file> <var>", "read one value, decrypting if needed"),
		Row(
			"run :env set <file> <var> [value]",
			"write one value, encrypting by default",
		),
		Row("run :env encrypt <src> <dst> <key>", "encrypt a plain file"),
		Row("run :env decrypt [src] [dst]", "decrypt to a plain file"),
		Row("run :env rotate <file>", "re-key every encrypted value"),
		Row(
			"run :env inject <file...> -- <cmd>",
			"run a command with the file's values",
		),
		Row("run :env secret-keys <command>", "add · list · get-private · remove"),
	],
)];

fn usage() -> String {
	crate::help::render(INTRO, SECTIONS)
}

pub fn dispatch(args: &[String]) -> Result<ExitCode, String> {
	let Some(sub) = args
		.first()
		.map(String::as_str)
		.filter(|_| !crate::help::wants_help(args))
	else {
		print!("{}", usage());
		return Ok(ExitCode::SUCCESS);
	};
	let rest = &args[1..];
	let flag = |n: &str| rest.iter().any(|a| a == n);
	let positional: Vec<&str> = rest
		.iter()
		.filter(|a| !a.starts_with("--"))
		.map(String::as_str)
		.collect();

	match sub {
		"init" => super::cmd_init(
			positional.first().copied().unwrap_or(".env"),
			flag("--plain"),
			value_of(rest, "--key"),
		),
		"get" => {
			let (f, v) = two(&positional, "get <file> <var>")?;
			super::cmd_get(f, v);
		}
		"set" => {
			let (f, v) = two(&positional, "set <file> <var> [value]")?;
			super::cmd_set(f, v, positional.get(2).copied(), flag("--plain"));
		}
		"encrypt" => {
			let (s, d) = two(&positional, "encrypt <src> <dst> <key-prefix>")?;
			let k = positional.get(2).copied().ok_or("encrypt needs a key prefix")?;
			super::cmd_encrypt_file(s, d, k);
		}
		"decrypt" => super::cmd_decrypt_file(positional.first().copied(), positional.get(1).copied()),
		"rotate" => {
			let f = positional.first().copied().ok_or("rotate needs a file")?;
			super::cmd_rotate(f, flag("--delete-current-key"));
		}
		"inject" => {
			// Everything after `--` is the command; before it, the files.
			let split = rest
				.iter()
				.position(|a| a == "--")
				.ok_or("inject needs `-- <command>`")?;
			let files: Vec<String> = rest[..split].to_vec();
			super::cmd_inject(&files, &rest[split + 1..]);
		}
		"secret-keys" => return secret_keys(rest),
		other => return Err(format!("unknown `:env` command `{other}`\n{}", usage())),
	}
	Ok(ExitCode::SUCCESS)
}

fn secret_keys(rest: &[String]) -> Result<ExitCode, String> {
	let sub = rest
		.first()
		.map(String::as_str)
		.ok_or("secret-keys needs a subcommand")?;
	let positional: Vec<&str> = rest[1..]
		.iter()
		.filter(|a| !a.starts_with("--"))
		.map(String::as_str)
		.collect();
	match sub {
		"add" => super::cmd_secret_keys_add(value_of(rest, "--key")),
		"list" => super::cmd_secret_keys_list(),
		"get-private" => super::cmd_get_private_key(positional.first().copied().ok_or("needs a key prefix")?),
		"remove" => super::cmd_secret_keys_remove(positional.first().copied().ok_or("needs a key prefix")?),
		other => return Err(format!("unknown `secret-keys` subcommand `{other}`")),
	}
	Ok(ExitCode::SUCCESS)
}

fn two<'a>(p: &[&'a str], usage: &str) -> Result<(&'a str, &'a str), String> {
	match (p.first(), p.get(1)) {
		(Some(a), Some(b)) => Ok((a, b)),
		_ => Err(format!("`:env {usage}`")),
	}
}

/// `--key=VALUE` or `--key VALUE`.
fn value_of<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
	let mut it = args.iter();
	while let Some(a) = it.next() {
		if let Some(v) = a.strip_prefix(name).and_then(|r| r.strip_prefix('=')) {
			return Some(v);
		}
		if a == name {
			return it.next().map(String::as_str);
		}
	}
	None
}
