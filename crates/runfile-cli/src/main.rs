//! The `run` command.
//!
//! Invocation is exactly `run <target> [args…]` -- one target, trailing args.
//! Concurrency is `.parallel` on a block, so there is no flag for it and no
//! target globs, which also means no wildcard that can match the target doing
//! the fanning out.

mod ci_detect;
mod cmd_env;
mod cmd_update;
mod list;
mod prepare;
mod prompt;

use runfile_discovery::{Catalog, discover};
use runfile_runtime::dispatch::Host;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
run <target> [args...]        run a target
run :list                     list every target
run :env <subcommand>         manage .env files
run :update                   update the runfile binary

  -y, --yes          skip confirmation prompts
      --stdin-args   prompt for inputs a target needs but was not given
      --dry-run      print what would run, without running it
      --dir <path>   start discovery here instead of the working directory
";

fn main() -> ExitCode {
	match real_main() {
		Ok(code) => code,
		Err(msg) => {
			eprintln!("error: {msg}");
			ExitCode::FAILURE
		}
	}
}

struct Flags {
	assume_yes: bool,
	stdin_args: bool,
	dry_run: bool,
	dir: Option<PathBuf>,
}

/// Runner flags are recognised only before the target name; everything after
/// it belongs to the target, so `run build --dry-run` passes the flag through.
fn split_flags(argv: Vec<String>) -> (Flags, Vec<String>) {
	let mut f = Flags {
		assume_yes: false,
		stdin_args: false,
		dry_run: false,
		dir: None,
	};
	let mut rest = Vec::new();
	let mut it = argv.into_iter();
	while let Some(a) = it.next() {
		match a.as_str() {
			"-y" | "--yes" => f.assume_yes = true,
			"--stdin-args" => f.stdin_args = true,
			"--dry-run" => f.dry_run = true,
			"--dir" => f.dir = it.next().map(PathBuf::from),
			_ => {
				rest.push(a);
				rest.extend(it);
				break;
			}
		}
	}
	(f, rest)
}

fn real_main() -> Result<ExitCode, String> {
	let (flags, rest) = split_flags(std::env::args().skip(1).collect());
	let Some(first) = rest.first().cloned() else {
		print!("{USAGE}");
		return Ok(ExitCode::SUCCESS);
	};
	let args: Vec<String> = rest[1..].to_vec();

	match first.as_str() {
		":env" => return cmd_env::dispatch(&args),
		":update" => {
			cmd_update::cmd_update(args.first().map(String::as_str));
			return Ok(ExitCode::SUCCESS);
		}
		":list" => {
			let cat = catalog(&flags)?;
			list::print(&cat);
			return Ok(ExitCode::SUCCESS);
		}
		t if t.starts_with(':') => return Err(format!("unknown command `{t}`\n\n{USAGE}")),
		_ => {}
	}

	let cat = catalog(&flags)?;
	let target = cat.resolve(&first).ok_or_else(|| list::unknown(&cat, &first))?;

	// The gate runs before anything else, so a target cannot half-run and then
	// be told its setup was missing.
	prepare::enforce(&cat, target)?;

	let ask = prompt::confirmer();
	let mut host = Host::new(&cat);
	host.assume_yes = flags.assume_yes || ci_detect::is_ci();
	if !host.assume_yes {
		host.prompt = Some(&ask);
	}
	if flags.stdin_args {
		host.ask = Some(prompt::ask_value);
	}
	host.dry_run = flags.dry_run;
	host.keys = runfile_settings::keyring_keys::all_private_keys;

	match host.run(&first, &args) {
		Ok(()) => {
			if flags.dry_run {
				for line in host.trace.lock().expect("trace").iter() {
					println!("{line}");
				}
			}
			prepare::record(&cat, target);
			Ok(ExitCode::SUCCESS)
		}
		Err(e) => Err(e.to_string()),
	}
}

fn catalog(flags: &Flags) -> Result<Catalog, String> {
	let from = match &flags.dir {
		Some(d) => d.clone(),
		None => std::env::current_dir().map_err(|e| e.to_string())?,
	};
	discover(&from, dirs::home_dir().as_deref()).map_err(|e| e.to_string())
}
