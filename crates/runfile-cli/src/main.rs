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
mod cmd_packages;
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
use std::sync::Mutex;

use help::{Row, Section};

const INTRO: &str = "run <target> [args...]   —   a cross-platform command runner";

const SECTIONS: &[Section] = &[
	Section(
		"Commands",
		&[
			Row("run <target> [args...]", "run a target"),
			Row(
				"run <package> [args...]",
				"run a package: host/owner/repo@version, or a ./folder",
			),
			Row(
				"run :packages <command>",
				"list, fetch and prune packages and downloads, with their sizes",
			),
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
			Row(
				"    --stdin-args",
				"prompt on stdin for inputs a target was not given, and for its confirm()",
			),
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
		":packages" => return cmd_packages::dispatch(&args),
		// Became `:packages fetch`. Said apart from an unknown command, as
		// `:format` is: it is a script written for the runner before.
		":fetch" => return Err(cmd_packages::FETCH_IS_PACKAGES.to_string()),
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

	// `run github.com/owner/repo@v1 sync`, or `run ./tools/j18n sync`: a
	// package, which needs no project here -- it is run from wherever someone
	// stands, as a program on PATH is.
	let package = runfile_lang::reference::is_reference(&first);
	let cat = if package { Catalog::default() } else { catalog(&flags)? };
	let target = match package {
		true => None,
		false => Some(cat.resolve(&first).ok_or_else(|| list::unknown(&cat, &first))?),
	};

	if let Some(target) = target {
		// Before anything else, and before the prepare gate: asking what a
		// target does must not require the project to be set up, and must never
		// be the thing that runs it. `run deploy --help` used to deploy. Not for
		// a package, which is a program someone runs: `--help` after it is the
		// program's own.
		if target_help::wants_help(&args) {
			print!("{}", target_help::render(&cat, target));
			return Ok(ExitCode::SUCCESS);
		}

		// The gate runs before anything else, so a target cannot half-run and
		// then be told its setup was missing. A preview is exempt: it changes
		// nothing, and reading what a target would do is a reasonable thing to
		// want before deciding to set the project up at all. A package has no
		// gate: nobody set up a cache entry, or works in one.
		if !flags.dry_run {
			prepare::enforce(&cat, target)?;
		}
	}
	let assume_yes = flags.assume_yes || ci_detect::is_ci();
	let cache = runfile_remote::Cache::open_default();
	if package && !flags.dry_run {
		first_fetch(&first, cache.as_ref(), assume_yes, flags.stdin_args)?;
	}

	// Caught from here on, and only here: a target run is what looks at the
	// flag -- between statements, at a prompt, in the watch loop -- and stops
	// in order, cleaning up its temp files. Caught for every command, as it
	// was, it made the rest unstoppable: nothing else looks, so a Ctrl+C at
	// `:env secret-keys add`'s prompt, or the SIGTERM an editor ends `:lsp`
	// with, was a flag nobody read. They are ended the way any program is.
	runfile_runtime::interrupt::install();
	let interrupted = || runfile_runtime::interrupt::interrupted();
	// A target that declares `.watch` enters watch mode with no flag: the file
	// already said what it wants. The run says what it watches as its header is
	// worked out, before its first line, and the watcher starts there.
	// `--dry-run` opts out, since printing the same commands forever is not a
	// preview.
	let watching: Mutex<Option<watch::Watching>> = Mutex::new(None);
	let start_watching = |props: &runfile_runtime::props::Props| -> Result<(), String> {
		let mut w = watching.lock().expect("watch lock");
		if w.is_none() && !props.watch.is_empty() {
			*w = Some(watch::Watching::start(&props.watch)?);
		}
		Ok(())
	};
	let mut host = Host::new(&cat);
	host.interrupted = Some(&interrupted);
	host.assume_yes = assume_yes;
	host.confirm = Some(prompt::confirm);
	host.cache = cache;
	host.dry_run = flags.dry_run;
	if flags.stdin_args {
		// A file that names something nothing defines is refused before anyone
		// is asked about it: the answers would only be thrown away.
		match target {
			Some(target) => {
				host.check(target).map_err(|e| e.to_string())?;
				// Asked before anything runs, from the list the tree gives. The
				// lazy prompt stays as a backstop for a value the walk cannot see
				// -- there should be none, and a missing one must still be
				// askable rather than fatal.
				stdin_args::collect(&cat, target, &mut args);
			}
			None => {
				let pkg = host.package(&first, None).map_err(|e| e.to_string())?;
				host.check_package(&pkg).map_err(|e| e.to_string())?;
				if let Some(entry) = pkg.entry() {
					stdin_args::collect(&pkg.catalog, entry, &mut args);
				}
			}
		}
		// Stopped at a question: nothing has run, and nothing will.
		if interrupted() {
			return Ok(ExitCode::from(runfile_runtime::interrupt::EXIT_CODE as u8));
		}
		host.ask = Some(prompt::ask_value);
		// The flag says somebody answers on stdin, so `confirm()` is asked there
		// too, terminal or not -- the same rule as every other question.
		host.confirm = Some(prompt::confirm_on_stdin);
	}
	host.keys = runfile_state::keyring_keys::all_private_keys;

	if !flags.dry_run {
		host.on_header = Some(&start_watching);
	}

	// The guard is what cleans up if the run panics -- an unwind would skip the
	// explicit call below, leaving a decrypted temp file on disk.
	let temps = host.temp_guard();
	let outcome = host.run(&first, &args);
	// However it ended. A target that fails half-way is exactly when a decoded
	// credential must not be left in the temp directory.
	drop(temps);
	let watcher = watching.lock().expect("watch lock").take();
	if let Some(w) = watcher {
		// The first run was the first iteration, and is reported as every other
		// one is. Nothing starts a second watcher after this one.
		host.on_header = None;
		let report = |outcome: Result<(), runfile_runtime::run::RunError>| {
			match outcome {
				Ok(()) => {
					if let Some(target) = target {
						prepare::record(&cat, target);
					}
				}
				// Said about an interrupt by nobody, as outside watch mode: the
				// terminal already echoed `^C`.
				Err(_) if interrupted() => {}
				Err(e) => eprintln!("{} {e}", runfile_runtime::exec::tag()),
			}
			// Between iterations, not just at the end: watch mode never reaches
			// one, and each run makes its own temp files.
			host.cleanup_temps();
		};
		report(outcome);
		// Never cleared: a signal ends the session, during a run as much as
		// between them, so no later run is there for a set flag to poison. It
		// was cleared after every run, which kept a session that was asked to
		// stop going, the flag forgotten, until a second Ctrl+C.
		w.follow(|| {
			// Cleans even if a run panics; `report` cleans too, which is
			// idempotent, and the intent is clearer with both.
			let _temps = host.temp_guard();
			report(host.run(&first, &args));
		});
		if interrupted() {
			return Ok(ExitCode::from(runfile_runtime::interrupt::EXIT_CODE as u8));
		}
		return Ok(ExitCode::SUCCESS);
	}
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
			if !flags.dry_run
				&& let Some(target) = target
			{
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

/// Ask before a package typed at the terminal is fetched from a repository
/// this machine has never fetched from -- the one place a typo in a name could
/// fetch and run someone else's code. Says which commit it would run.
///
/// A reference written in a file is not asked about: the file is the decision,
/// as its `$ curl … | sh` would be. Nor is one after `-y` or in CI, or one the
/// cache already knows the repository of.
fn first_fetch(
	word: &str,
	cache: Option<&runfile_remote::Cache>,
	assume_yes: bool,
	stdin_args: bool,
) -> Result<(), String> {
	use runfile_lang::reference::Reference;
	let (Ok(Reference::Remote(r)), Some(cache)) = (runfile_lang::reference::parse(word), cache) else {
		// A path needs no fetching, and a reference that cannot be read is the
		// run's to refuse, with the same message.
		return Ok(());
	};
	if assume_yes || cache.knows(&r) {
		return Ok(());
	}
	let resolved = cache.resolve(&r, runfile_remote::Network::Allowed).map_err(|e| e.0)?;
	let question = format!(
		"{} has not been run on this machine; fetch {} ({}) and run it?",
		r.repository(),
		r.shown(),
		runfile_remote::short(&resolved.commit)
	);
	let yes = match stdin_args {
		true => prompt::confirm_on_stdin(&question),
		false => prompt::confirm(&question),
	};
	match yes {
		true => Ok(()),
		false => Err(format!(
			"not fetched: {} runs only once you say yes, or with `-y` -- `run -y {word}`",
			r.repository()
		)),
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
