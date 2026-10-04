//! The `run` command.
//!
//! Invocation is exactly `run <target> [args…]` -- one target, trailing args.
//! Concurrency is `parallel do` and `parallel for` in a file, so there is no flag for it and no
//! target globs, which also means no wildcard that can match the target doing
//! the fanning out.

mod ci_detect;
mod cmd_env;
mod cmd_generate;
mod cmd_lint;
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
			Row("run :lint", "format every runfile and check it for errors"),
			Row("run :env <command>", "manage .env files"),
			Row("run :completions <command>", "shell tab-completion"),
			Row(
				"run :generate <editor>",
				"write task files for zed, jetbrains or vscode",
			),
			Row("run :lsp", "start the language server on stdin/stdout"),
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
			if help::wants_help(&args) {
				print!("{}", cmd_update::usage());
				return Ok(ExitCode::SUCCESS);
			}
			return cmd_update::cmd_update(&args);
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
		":lint" => {
			if help::wants_help(&args) {
				print!("{}", cmd_lint::usage());
				return Ok(ExitCode::SUCCESS);
			}
			if let Some(bad) = args
				.iter()
				.find(|a| a.starts_with('-') && !cmd_lint::FLAGS.contains(&a.as_str()))
			{
				return Err(format!("unknown flag `{bad}` for `:lint`\n{}", cmd_lint::usage()));
			}
			let flag = |n: &str| args.iter().any(|a| a == n);
			let paths: Vec<String> = args.iter().filter(|a| !a.starts_with('-')).cloned().collect();
			if !paths.is_empty() {
				let files = cmd_lint::from_paths(&paths)?;
				return Ok(cmd_lint::lint(&files, None, flag("--check"), flag("--stdout")));
			}
			let from = start(&flags)?;
			let cat = cmd_lint::catalog(&from).map_err(|e| e.to_string())?;
			let files = cmd_lint::project_files(&cat, &from, flag("--include-global"));
			return Ok(cmd_lint::lint(&files, Some(&cat), flag("--check"), flag("--stdout")));
		}
		// Became `:lint`. Said apart from an unknown command, since it is not a
		// typo: it is a script or a habit written for an older runner, and the
		// person reading this needs the new name rather than the list of commands.
		":format" => return Err(cmd_lint::FORMAT_IS_LINT.to_string()),
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
		// The language server, on stdin/stdout. It answers here -- before the
		// catalog is built and before anything else can reach stdout -- because
		// LSP framing owns that stream: one stray line of ours desynchronises
		// the client for the rest of the session. Errors go to stderr, which
		// is where `main` already writes them and where an editor logs them.
		":lsp" => {
			runfile_lsp::serve().map_err(|e| e.to_string())?;
			return Ok(ExitCode::SUCCESS);
		}
		":init" => {
			print!("{}", init::init(&start(&flags)?)?);
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

	let interrupted = || runfile_runtime::interrupt::interrupted();
	let mut host = Host::new(&cat);
	host.interrupted = Some(&interrupted);
	host.assume_yes = flags.assume_yes || ci_detect::is_ci();
	host.confirm = Some(prompt::confirm);
	if flags.stdin_args {
		// A file that names something nothing defines is refused before anyone
		// is asked about it: the answers would only be thrown away.
		host.check(target).map_err(|e| e.to_string())?;
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
		match host.header_props(target, &args) {
			Ok(props) => props.watch,
			// An `exit()` in the header, or in a `_shared.run` `let`, ends the
			// target before anything it declares applies. The run meets it too,
			// and ends the way it says; reported from here it was `error: exit
			// 0`, status 1.
			Err(e) if e.exit_code().is_some() => Vec::new(),
			Err(e) => return Err(e.to_string()),
		}
	};
	if !watching.is_empty() {
		let anchor = target.anchor.clone();
		return watch::watch(&anchor, &watching, || {
			// Cleans even if a run panics; the explicit call below is kept
			// because cleanup is idempotent and the intent is clearer with it.
			let _temps = host.temp_guard();
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

	// The guard is what cleans up if the run panics -- an unwind would skip the
	// explicit call below, leaving a decrypted temp file on disk.
	let temps = host.temp_guard();
	let outcome = host.run(&first, &args);
	// However it ended. A target that fails half-way is exactly when a decoded
	// credential must not be left in the temp directory.
	drop(temps);
	if flags.dry_run {
		// However it ended, too. A preview that stopped part-way -- at an
		// `exit()`, or at a failure -- would still have run everything above
		// the line it stopped at, and printing only a finished one showed
		// nothing at all for a target that exits early. One that stopped before
		// its first command has nothing to show but why.
		let trace = host.trace.lock().expect("trace");
		if outcome.is_ok() || !trace.is_empty() {
			// Which shell `$` resolved to, so "bash here, sh there" is visible
			// rather than discovered.
			match runfile_runtime::shell::default_shell() {
				Some(p) => println!("# $ runs {}", p.display()),
				None => println!("# $ has no shell available"),
			}
			for line in trace.iter() {
				println!("{line}");
			}
		}
	}
	match outcome {
		// A run that ended well tells the gate its setup is done -- unless
		// nothing ran. `record` only writes for a setup target, and a preview of
		// one ran none of its commands: recording it marked the directory
		// prepared, and every target after it walked through the gate.
		// `--dry-run` changes nothing, and that includes what the gate believes.
		Ok(()) => {
			if !flags.dry_run {
				prepare::record(&cat, target);
			}
			Ok(ExitCode::SUCCESS)
		}
		// `exit(code)` is not a failure to report: it is the status the target
		// asked for, so it is returned rather than printed as an error. It is not
		// a run that ended well either -- `exit(0)` ends one as `Ok` and never
		// reaches here -- so a setup that says 1 leaves the gate shut.
		Err(e) => match e.exit_code() {
			Some(code) => Ok(ExitCode::from(code as u8)),
			None => Err(e.to_string()),
		},
	}
}

fn catalog(flags: &Flags) -> Result<Catalog, String> {
	discover(&start(flags)?, discovery_home().as_deref()).map_err(|e| e.to_string())
}

/// Where discovery starts: `--dir`, or the working directory.
fn start(flags: &Flags) -> Result<PathBuf, String> {
	match &flags.dir {
		Some(d) => Ok(d.clone()),
		None => std::env::current_dir().map_err(|e| e.to_string()),
	}
}

/// The home directory discovery folds a machine-wide `runfiles/` in from --
/// and **`None` in CI**.
///
/// A runner's home directory is nobody's: on a hosted one it holds whatever the
/// image happened to ship, and on a self-hosted one it belongs to the machine's
/// owner rather than to this job. Either way a target that no reader of the
/// repository can see must not join the run, and must not shadow one that is
/// checked in. Passing no home is the whole gate -- there is no second place
/// that decides, so the catalog, `:list`, `:lint`, `:generate` and
/// `:complete` are covered by this one call rather than by an `is_ci` each.
///
/// It is also why nothing has to *clean* `$HOME/.runfiles` on a runner any
/// more: a directory that is never read is not a leak, and deleting a
/// self-hosted runner's own would be pure destruction.
pub fn discovery_home() -> Option<std::path::PathBuf> {
	if ci_detect::is_ci() {
		return None;
	}
	runfile_discovery::home_dir()
}
