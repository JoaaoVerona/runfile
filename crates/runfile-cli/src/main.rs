//! The `run` command.
//!
//! Invocation is exactly `run <target> [args…]` -- one target, trailing args.
//! Concurrency is `.parallel` on a block, so there is no flag for it and no
//! target globs, which also means no wildcard that can match the target doing
//! the fanning out.

mod ci_detect;
mod cmd_env;
mod cmd_format;
mod cmd_generate;
mod cmd_update;
mod completions;
mod help;
mod init;
mod list;
mod prepare;
mod prompt;
mod stdin_args;
mod target_help;
mod watch;

use runfile_discovery::{Catalog, discover};
use runfile_runtime::dispatch::Host;
use std::path::PathBuf;
use std::process::ExitCode;

use help::{Row, Section};

const INTRO: &str = "run <target> [args...]   —   a cross-platform command runner";

const SECTIONS: &[Section] = &[
	Section(
		"Commands",
		&[
			Row("run <target> [args...]", "run a target"),
			Row("run :list", "list every target"),
			Row("run :init", "create runfiles/ with an example target"),
			Row("run :format", "format every runfile in this project"),
			Row("run :env <command>", "manage .env files"),
			Row("run :completions <command>", "shell tab-completion"),
			Row(
				"run :generate <editor>",
				"write task files for zed, jetbrains or vscode",
			),
			Row("run :update", "update the runfile binary"),
		],
	),
	Section(
		"Options",
		&[
			Row("-y, --yes", "skip confirmation prompts"),
			Row("    --stdin-args", "prompt for inputs a target needs but was not given"),
			Row("    --dry-run", "print what would run, without running it"),
			Row(
				"    --dir <path>",
				"start discovery here instead of the working directory",
			),
			Row("-h, --help", "show this"),
			Row("-v, --version", "print the version"),
		],
	),
];

pub(crate) fn usage() -> String {
	help::render(INTRO, SECTIONS)
}

/// `run :list --names | head` should end quietly, not with a backtrace.
fn runtime_pipe_default() {
	runfile_runtime::interrupt::ignore_broken_pipe();
}

fn main() -> ExitCode {
	runfile_runtime::interrupt::install();
	runtime_pipe_default();
	match real_main() {
		Ok(code) => code,
		Err(msg) => {
			// 130 is what a shell reports for SIGINT, so a caller can tell an
			// interrupt from a failure. `run` prints nothing extra: the
			// terminal already echoed `^C`.
			if runfile_runtime::interrupt::interrupted() {
				return ExitCode::from(runfile_runtime::interrupt::EXIT_CODE as u8);
			}
			eprintln!("{} error: {msg}", runfile_runtime::exec::tag());
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
		print!("{}", usage());
		return Ok(ExitCode::SUCCESS);
	};
	let mut args: Vec<String> = rest[1..].to_vec();

	match first.as_str() {
		":env" => return cmd_env::dispatch(&args),
		":generate" => {
			// Asking what it does must not require a project to be there.
			if args.is_empty() || help::wants_help(&args) {
				print!("{}", cmd_generate::usage());
				return Ok(ExitCode::SUCCESS);
			}
			let cat = catalog(&flags)?;
			return cmd_generate::dispatch(&cat, &args);
		}
		":update" => {
			cmd_update::cmd_update(args.first().map(String::as_str));
			return Ok(ExitCode::SUCCESS);
		}
		"--version" | "-v" | "-V" => {
			println!("run {}", env!("CARGO_PKG_VERSION"));
			return Ok(ExitCode::SUCCESS);
		}
		"--help" | "-h" => {
			print!("{}", usage());
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
		":format" => {
			if help::wants_help(&args) {
				print!("{}", cmd_format::usage());
				return Ok(ExitCode::SUCCESS);
			}
			let flag = |n: &str| args.iter().any(|a| a == n);
			let paths: Vec<String> = args.iter().filter(|a| !a.starts_with('-')).cloned().collect();
			let files = if paths.is_empty() {
				cmd_format::project_files(&catalog(&flags)?, flag("--include-global"))
			} else {
				cmd_format::from_paths(&paths)?
			};
			return cmd_format::format_files(&files, flag("--check"), flag("--stdout"));
		}
		":completions" => return completions::dispatch(&args),
		// Hidden: the shells' one question, "what may follow what". Kept out of
		// the help and out of the tree, since nobody types it. It must never fail
		// loudly either -- a Tab in a directory with no runfiles offers nothing,
		// it does not print an error over the prompt.
		":complete" => {
			let cword: usize = args.first().and_then(|a| a.parse().ok()).unwrap_or(0);
			let words = args.get(1..).unwrap_or(&[]);
			let cat = catalog(&flags).ok();
			let targets = || cat.as_ref().map(list::names).unwrap_or_default();
			for c in completions::complete(words, cword, &targets) {
				println!("{c}");
			}
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
		t if t.starts_with(':') => return Err(format!("unknown command `{t}`\n{}", usage())),
		_ => {}
	}

	let cat = catalog(&flags)?;
	let target = cat.resolve(&first).ok_or_else(|| list::unknown(&cat, &first))?;

	// Before anything else, and before the prepare gate: asking what a target
	// does must not require the project to be set up, and must never be the
	// thing that runs it. `run deploy --help` used to deploy.
	if target_help::wants_help(&args) {
		print!("{}", target_help::render(&cat, target));
		return Ok(ExitCode::SUCCESS);
	}

	// The gate runs before anything else, so a target cannot half-run and then
	// be told its setup was missing. A preview is exempt: it changes nothing,
	// and reading what a target would do is a reasonable thing to want before
	// deciding to set the project up at all.
	if !flags.dry_run {
		prepare::enforce(&cat, target)?;
	}

	let warn = |m: &str| eprintln!("{} warning: {m}", runfile_runtime::exec::tag());
	let interrupted = || runfile_runtime::interrupt::interrupted();
	let mut host = Host::new(&cat);
	host.warn = Some(&warn);
	host.interrupted = Some(&interrupted);
	host.assume_yes = flags.assume_yes || ci_detect::is_ci();
	host.confirm = Some(prompt::confirm);
	if flags.stdin_args {
		// Asked before anything runs, from the list the tree gives. The lazy
		// prompt stays as a backstop for a value the walk cannot see -- there
		// should be none, and a missing one must still be askable rather than
		// fatal.
		stdin_args::collect(&cat, target, &mut args);
		host.ask = Some(prompt::ask_value);
	}
	host.dry_run = flags.dry_run;
	host.keys = runfile_state::keyring_keys::all_private_keys;

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
		return watch::watch(&anchor, &watching, || {
			match host.run(&first, &args) {
				Ok(()) => prepare::record(&cat, target),
				Err(e) => eprintln!("{} {e}", runfile_runtime::exec::tag()),
			}
			// Each iteration starts clean; Ctrl+C ends the session rather than
			// poisoning every run after it.
			runfile_runtime::interrupt::clear();
			// Between iterations, not just at the end: watch mode never reaches
			// one, and each run makes its own temp files.
			host.cleanup_temps();
		})
		.map(|()| ExitCode::SUCCESS);
	}

	let outcome = host.run(&first, &args);
	// However it ended. A target that fails half-way is exactly when a decoded
	// credential must not be left in the temp directory.
	host.cleanup_temps();
	match outcome {
		Ok(()) => {
			if flags.dry_run {
				// Which shell `$` resolved to, so "bash here, sh there" is
				// visible rather than discovered.
				match runfile_runtime::shell::default_shell() {
					Some(p) => println!("# $ runs {}", p.display()),
					None => println!("# $ has no shell available"),
				}
				for line in host.trace.lock().expect("trace").iter() {
					println!("{line}");
				}
			}
			prepare::record(&cat, target);
			Ok(ExitCode::SUCCESS)
		}
		// `exit(code)` is not a failure: it is the status the target asked for,
		// so it is returned rather than printed as an error.
		Err(e) => match e.exit_code() {
			Some(code) => {
				prepare::record(&cat, target);
				Ok(ExitCode::from(code as u8))
			}
			None => Err(e.to_string()),
		},
	}
}

fn catalog(flags: &Flags) -> Result<Catalog, String> {
	let from = match &flags.dir {
		Some(d) => d.clone(),
		None => std::env::current_dir().map_err(|e| e.to_string())?,
	};
	discover(&from, runfile_discovery::home_dir().as_deref()).map_err(|e| e.to_string())
}
