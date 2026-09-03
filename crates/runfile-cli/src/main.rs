//! The `run` command.
//!
//! Invocation is exactly `run <target> [args…]` -- one target, trailing args.
//! Concurrency is `.parallel` on a block, so there is no flag for it and no
//! target globs, which also means no wildcard that can match the target doing
//! the fanning out.

mod ci_detect;
mod cmd_env;
mod cmd_update;
mod completions;
mod init;
mod list;
mod prepare;
mod prompt;
mod watch;

use runfile_discovery::{Catalog, discover};
use runfile_runtime::dispatch::Host;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
run <target> [args...]        run a target
run :list                     list every target
run :init                     create runfiles/ with an example target
run :env <subcommand>         manage .env files
run :completions <shell>      print a completion script
run :update                   update the runfile binary
run :version                  print the version

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
		":version" | "--version" | "-V" => {
			println!("run {}", env!("CARGO_PKG_VERSION"));
			return Ok(ExitCode::SUCCESS);
		}
		":list" => {
			let cat = catalog(&flags)?;
			// Two machine-readable forms: bare names for completion scripts,
			// JSON for tooling that needs paths and descriptions too.
			if args.iter().any(|a| a == "--names") {
				list::print_names(&cat);
			} else if args.iter().any(|a| a == "--json") {
				list::print_json(&cat);
			} else {
				list::print(&cat);
			}
			return Ok(ExitCode::SUCCESS);
		}
		":completions" => {
			let shell = args
				.first()
				.ok_or("usage: run :completions <bash|zsh|fish|powershell>")?;
			print!("{}", completions::script(shell)?);
			return Ok(ExitCode::SUCCESS);
		}
		":init" => {
			let dir = match &flags.dir {
				Some(d) => d.clone(),
				None => std::env::current_dir().map_err(|e| e.to_string())?,
			};
			print!("{}", init::init(&dir)?);
			return Ok(ExitCode::SUCCESS);
		}
		t if t.starts_with(':') => return Err(format!("unknown command `{t}`\n\n{USAGE}")),
		_ => {}
	}

	let cat = catalog(&flags)?;
	let target = cat.resolve(&first).ok_or_else(|| list::unknown(&cat, &first))?;

	// The gate runs before anything else, so a target cannot half-run and then
	// be told its setup was missing. A preview is exempt: it changes nothing,
	// and reading what a target would do is a reasonable thing to want before
	// deciding to set the project up at all.
	if !flags.dry_run {
		prepare::enforce(&cat, target)?;
	}

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

	// A target that declares `.watch` enters watch mode with no flag: the file
	// already said what it wants. `--dry-run` opts out, since printing the same
	// commands forever is not a preview.
	let watching = if flags.dry_run {
		Vec::new()
	} else {
		host.header_props(target, &args).map_err(|e| e.to_string())?.watch
	};
	if !watching.is_empty() {
		let anchor = target.anchor.clone();
		return watch::watch(&anchor, &watching, || match host.run(&first, &args) {
			Ok(()) => prepare::record(&cat, target),
			Err(e) => eprintln!("[runfile] {e}"),
		})
		.map(|()| ExitCode::SUCCESS);
	}

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
