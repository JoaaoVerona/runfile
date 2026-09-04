//! `run :completions <shell>` -- hand-written completion scripts.
//!
//! `install` and `uninstall` put the hook into the shell's own profile, or into
//! fish's completions directory, and take it out again -- both idempotent, both
//! leaving everything else in the file alone.
//!
//! Every script asks the binary itself what may follow what (`run :complete`)
//! rather than carrying its own copy of the command tree. That is what makes
//! subcommands complete at any depth: the tree below is the only description of
//! it, so four shell dialects cannot drift from each other or from the CLI.

use std::path::{Path, PathBuf};

const INTRO: &str = "run :completions <command> <shell>   —   shell tab-completion";

const SECTIONS: &[crate::help::Section] = &[
	crate::help::Section(
		"Commands",
		&[
			crate::help::Row(
				"run :completions install <shell>",
				"add the hook to that shell's profile",
			),
			crate::help::Row("run :completions uninstall <shell>", "take it out again"),
			crate::help::Row(
				"run :completions output <shell>",
				"print the script, for `eval` or by hand",
			),
		],
	),
	crate::help::Section("Shells", &[crate::help::Row("bash · zsh · fish · powershell", "")]),
];

pub fn dispatch(args: &[String]) -> Result<std::process::ExitCode, String> {
	let usage = || crate::help::render(INTRO, SECTIONS);
	if args.is_empty() || crate::help::wants_help(args) {
		print!("{}", usage());
		return Ok(std::process::ExitCode::SUCCESS);
	}
	let action = args[0].as_str();
	let shell = args
		.get(1)
		.ok_or_else(|| format!("`{action}` needs a shell\n{}", usage()))?;
	match action {
		"install" => println!("{}", install(shell)?),
		"uninstall" => println!("{}", uninstall(shell)?),
		"output" => print!("{}", script(shell)?),
		other => return Err(format!("unknown command `{other}`\n{}", usage())),
	}
	Ok(std::process::ExitCode::SUCCESS)
}

/// Where an installed hook lives, and what goes in it.
///
/// A profile gets a marked block that calls the binary rather than a copy of
/// the script, so an upgraded `run` is picked up without reinstalling. Fish
/// reads a directory, so it gets a file of its own.
enum Where {
	/// Append a marked block to this profile.
	Profile(PathBuf, String),
	/// Write the whole script to this file.
	File(PathBuf),
}

const MARKER: &str = "# runfile completions";

fn home() -> Result<PathBuf, String> {
	runfile_discovery::home_dir().ok_or_else(|| "no home directory".to_string())
}

fn config_dir() -> Result<PathBuf, String> {
	match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
		Some(d) => Ok(PathBuf::from(d)),
		None => Ok(home()?.join(".config")),
	}
}

/// `~/.local/share`, or wherever XDG says.
fn data_dir() -> Result<PathBuf, String> {
	match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
		Some(d) => Ok(PathBuf::from(d)),
		None => Ok(home()?.join(".local/share")),
	}
}

/// This binary's own path, so an installed hook does not depend on PATH being
/// ready when the shell reads its profile.
fn exe() -> String {
	std::env::current_exe()
		.map(|p| p.display().to_string())
		.unwrap_or_else(|_| "run".to_string())
}

/// Where an older version put the hook, so `uninstall` can still find it.
fn legacy_profile(shell: &str) -> Option<PathBuf> {
	match shell {
		"bash" => home().ok().map(|h| h.join(".bashrc")),
		_ => None,
	}
}

fn destination(shell: &str) -> Result<Where, String> {
	Ok(match shell {
		// A file in bash-completion's own directory rather than a line in
		// `.bashrc`. Ubuntu's `~/.profile` sources `.bashrc` *before* it puts
		// `~/.local/bin` on PATH, so a startup hook that shells out to `run`
		// finds nothing and `eval` registers nothing, silently. A file here is
		// read on demand, by which time PATH is complete.
		"bash" => Where::File(data_dir()?.join("bash-completion/completions/run")),
		// zsh can hit the same ordering, so the line names the binary outright
		// rather than trusting PATH at startup.
		"zsh" => Where::Profile(
			home()?.join(".zshrc"),
			format!(r#"eval "$({} :completions output zsh)""#, exe()),
		),
		"fish" => Where::File(config_dir()?.join("fish/completions/run.fish")),
		"powershell" | "pwsh" => Where::Profile(
			powershell_profile()?,
			format!(
				"& '{}' :completions output powershell | Out-String | Invoke-Expression",
				exe()
			),
		),
		other => return Err(unknown(other)),
	})
}

/// PowerShell's `$PROFILE`, by its documented location rather than by asking
/// PowerShell -- which may not be installed on the machine doing the install.
fn powershell_profile() -> Result<PathBuf, String> {
	if cfg!(windows) {
		Ok(home()?.join("Documents/PowerShell/Microsoft.PowerShell_profile.ps1"))
	} else {
		Ok(config_dir()?.join("powershell/Microsoft.PowerShell_profile.ps1"))
	}
}

/// Add the hook, or say it is already there. Idempotent: the marker is what
/// makes a second install a no-op and an uninstall exact.
pub fn install(shell: &str) -> Result<String, String> {
	match destination(shell)? {
		Where::File(path) => {
			let body = script(shell)?;
			write_new(&path, &body)?;
			Ok(format!("Installed to {}", path.display()))
		}
		Where::Profile(path, line) => {
			let existing = std::fs::read_to_string(&path).unwrap_or_default();
			if existing.contains(MARKER) {
				return Ok(format!("Already installed in {}", path.display()));
			}
			let mut out = existing;
			if !out.is_empty() && !out.ends_with('\n') {
				out.push('\n');
			}
			out.push_str(&format!("\n{MARKER}\n{line}\n"));
			write_new(&path, &out)?;
			Ok(format!(
				"Installed to {}\nRestart your shell to pick it up.",
				path.display()
			))
		}
	}
}

/// Take it out again, leaving anything else in the profile untouched.
pub fn uninstall(shell: &str) -> Result<String, String> {
	// An older version put a line in `.bashrc`; take that out too, so an
	// upgrade does not leave a dead hook behind.
	let mut also = String::new();
	if let Some(profile) = legacy_profile(shell)
		&& let Ok(text) = std::fs::read_to_string(&profile)
		&& let Some(trimmed) = without_block(&text)
	{
		write_new(&profile, &trimmed)?;
		also = format!("\nAlso removed the older hook from {}", profile.display());
	}
	Ok(uninstall_at(shell)? + &also)
}

fn uninstall_at(shell: &str) -> Result<String, String> {
	match destination(shell)? {
		Where::File(path) => match std::fs::remove_file(&path) {
			Ok(()) => Ok(format!("Removed {}", path.display())),
			Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
				Ok(format!("Nothing installed at {}", path.display()))
			}
			Err(e) => Err(format!("{}: {e}", path.display())),
		},
		Where::Profile(path, _) => {
			let Ok(existing) = std::fs::read_to_string(&path) else {
				return Ok(format!("Nothing installed in {}", path.display()));
			};
			let Some(trimmed) = without_block(&existing) else {
				return Ok(format!("Nothing installed in {}", path.display()));
			};
			write_new(&path, &trimmed)?;
			Ok(format!("Removed from {}", path.display()))
		}
	}
}

/// Drop the marker line and the line after it, and nothing else.
fn without_block(text: &str) -> Option<String> {
	let lines: Vec<&str> = text.lines().collect();
	let at = lines.iter().position(|l| l.trim() == MARKER)?;
	let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
	kept.extend(&lines[..at]);
	kept.extend(&lines[(at + 2).min(lines.len())..]);
	// The block was preceded by a blank separator we added; take it back.
	while kept.last().is_some_and(|l| l.trim().is_empty()) {
		kept.pop();
	}
	let mut out = kept.join("\n");
	if !out.is_empty() {
		out.push('\n');
	}
	Some(out)
}

fn write_new(path: &Path, body: &str) -> Result<(), String> {
	if let Some(d) = path.parent() {
		std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
	}
	std::fs::write(path, body).map_err(|e| format!("{}: {e}", path.display()))
}

fn unknown(shell: &str) -> String {
	format!("unknown shell `{shell}`; expected one of: bash, zsh, fish, powershell")
}

// ---------------------------------------------------------------- the tree
//
// One description of what may follow what, which every shell queries through
// `run :complete`. Encoding it here rather than in four shell dialects is what
// lets a subcommand of a subcommand complete: nesting costs a nested `Cmd`,
// not a fifth copy of the walk in a language that cannot share it.

/// What a word in this position completes to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Arg {
	/// Nothing. The position takes no free-form word.
	None,
	/// A path, or a directory. The shell is told to take over, since it does
	/// this far better than a word list can.
	Files,
	Dirs,
	/// A target name, read from the catalog.
	Targets,
}

/// A flag, and what its value completes to if it takes one.
pub struct Flag(pub &'static str, pub Arg);

pub struct Cmd {
	pub name: &'static str,
	pub subs: &'static [Cmd],
	pub flags: &'static [Flag],
	/// What a word that matches no subcommand completes to.
	pub arg: Arg,
}

const fn leaf(name: &'static str) -> Cmd {
	Cmd {
		name,
		subs: &[],
		flags: &[],
		arg: Arg::None,
	}
}

/// A command whose remaining words are paths.
const fn takes_file(name: &'static str, flags: &'static [Flag]) -> Cmd {
	Cmd {
		name,
		subs: &[],
		flags,
		arg: Arg::Files,
	}
}

/// The shells with a script -- also the words `:completions` takes.
const SHELLS: &[Cmd] = &[leaf("bash"), leaf("zsh"), leaf("fish"), leaf("powershell")];

const fn takes_shell(name: &'static str) -> Cmd {
	Cmd {
		name,
		subs: SHELLS,
		flags: &[],
		arg: Arg::None,
	}
}

const GEN_FLAGS: &[Flag] = &[Flag("--include-global", Arg::None), Flag("--stdout", Arg::None)];

const fn editor(name: &'static str) -> Cmd {
	Cmd {
		name,
		subs: &[],
		flags: GEN_FLAGS,
		arg: Arg::None,
	}
}

/// A `--key` names a key file, and may be spelled `--key X` or `--key=X`.
const KEY: Flag = Flag("--key", Arg::Files);

/// The whole command line, from `run` down.
pub const ROOT: Cmd = Cmd {
	name: "run",
	subs: &[
		Cmd {
			name: ":list",
			subs: &[],
			flags: &[Flag("--names", Arg::None), Flag("--json", Arg::None)],
			arg: Arg::None,
		},
		leaf(":init"),
		Cmd {
			name: ":env",
			subs: &[
				takes_file("init", &[Flag("--plain", Arg::None), KEY]),
				takes_file("get", &[]),
				takes_file("set", &[Flag("--plain", Arg::None)]),
				takes_file("encrypt", &[]),
				takes_file("decrypt", &[]),
				takes_file("rotate", &[Flag("--delete-current-key", Arg::None)]),
				takes_file("inject", &[]),
				Cmd {
					name: "secret-keys",
					subs: &[
						Cmd {
							name: "add",
							subs: &[],
							flags: &[KEY],
							arg: Arg::None,
						},
						leaf("list"),
						leaf("get-private"),
						leaf("remove"),
					],
					flags: &[],
					arg: Arg::None,
				},
			],
			flags: &[],
			arg: Arg::None,
		},
		Cmd {
			name: ":completions",
			subs: &[takes_shell("install"), takes_shell("uninstall"), takes_shell("output")],
			flags: &[],
			arg: Arg::None,
		},
		Cmd {
			name: ":generate",
			subs: &[editor("zed"), editor("jetbrains"), editor("vscode")],
			flags: &[],
			arg: Arg::None,
		},
		leaf(":update"),
	],
	flags: &[
		Flag("-y", Arg::None),
		Flag("--yes", Arg::None),
		Flag("--stdin-args", Arg::None),
		Flag("--dry-run", Arg::None),
		Flag("--dir", Arg::Dirs),
		Flag("-h", Arg::None),
		Flag("--help", Arg::None),
		Flag("-v", Arg::None),
		Flag("--version", Arg::None),
	],
	arg: Arg::Targets,
};

/// Asks the shell to complete paths itself. A word list cannot do it well:
/// only the shell knows to append a `/` and not a space.
pub const FILES: &str = "<files>";
pub const DIRS: &str = "<dirs>";

/// Candidates for the word at `cword`, given the words typed so far.
///
/// `words[0]` is the program name. The word being completed is `words[cword]`
/// when the shell passes it (bash does, fish does not), and may be a partial
/// one -- filtering by it is the shell's job, except where it decides *which*
/// kind of candidate applies at all.
pub fn complete(words: &[String], cword: usize, targets: &dyn Fn() -> Vec<String>) -> Vec<String> {
	let cur = words.get(cword).map(String::as_str).unwrap_or("");
	let mut node = &ROOT;
	let mut root = true;
	let mut i = 1;
	while i < cword.min(words.len()) {
		let w = words[i].as_str();
		if let Some(f) = node.flags.iter().find(|f| f.0 == w) {
			// `--dir /some/path`: the value is a word of its own, and if the
			// cursor is on it, that is what we are completing.
			if f.1 != Arg::None {
				if i + 1 == cword {
					return vec![marker(f.1)];
				}
				i += 1;
			}
			i += 1;
			continue;
		}
		if w.starts_with('-') {
			i += 1;
			continue;
		}
		match node.subs.iter().find(|c| c.name == w) {
			Some(next) => {
				node = next;
				root = false;
			}
			// An unrecognised word at the top is a target name, and everything
			// after it belongs to the target rather than to us.
			None if root => return Vec::new(),
			// Below the top it is a free-form argument, so the node keeps
			// offering whatever it offers.
			None => break,
		}
		i += 1;
	}

	// At the top, a bare Tab lists the commands *and* the targets together --
	// both are things a person types there, and hiding the commands behind a
	// `:` makes them undiscoverable. Only flags wait to be asked for.
	if root {
		if cur.starts_with('-') {
			return node.flags.iter().map(|f| f.0.to_string()).collect();
		}
		let mut out: Vec<String> = node.subs.iter().map(|c| c.name.to_string()).collect();
		out.extend(targets());
		return out;
	}

	let mut out: Vec<String> = node.subs.iter().map(|c| c.name.to_string()).collect();
	// Flags only once one is being typed: they are noise beside a file list.
	if cur.starts_with('-') {
		out.extend(node.flags.iter().map(|f| f.0.to_string()));
	} else if node.arg != Arg::None {
		out.push(marker(node.arg));
	}
	out
}

fn marker(arg: Arg) -> String {
	match arg {
		Arg::Dirs => DIRS.to_string(),
		_ => FILES.to_string(),
	}
}

pub fn script(shell: &str) -> Result<String, String> {
	match shell {
		"bash" => Ok(BASH.to_string()),
		"zsh" => Ok(ZSH.to_string()),
		"fish" => Ok(FISH.to_string()),
		"powershell" | "pwsh" => Ok(POWERSHELL.to_string()),
		other => Err(unknown(other)),
	}
}

const BASH: &str = r#"# run(1) completion. Install: run :completions install bash
#
# The binary owns the command tree. This asks it what may follow what, and it
# answers with words, or with <files>/<dirs> when only the shell can do it.

# Readline breaks a word at every character in COMP_WORDBREAKS, and `:` is one
# of them by default -- which splits both `:env` and a namespaced target like
# `vscode:test` into pieces before this function ever sees them, and makes
# readline insert a candidate's own colon after the one already typed. Taking
# `:` out is the fix npm and nvm use, and it has to happen at load: readline
# reads the variable after the function returns, not before.
COMP_WORDBREAKS="${COMP_WORDBREAKS//:/}"

_run() {
	local cur out
	cur="${COMP_WORDS[COMP_CWORD]}"
	out="$(run :complete "$COMP_CWORD" "${COMP_WORDS[@]}" 2>/dev/null)"
	COMPREPLY=()
	case "$out" in
		*"<dirs>"*) COMPREPLY=( $(compgen -d -- "$cur") ); out="${out/<dirs>/}" ;;
		*"<files>"*) COMPREPLY=( $(compgen -f -- "$cur") ); out="${out/<files>/}" ;;
	esac
	COMPREPLY+=( $(compgen -W "$out" -- "$cur") )
}
complete -F _run run
"#;

const ZSH: &str = r#"# run(1) completion. Install: run :completions install zsh
_run() {
	local -a out
	# `${(@)words}` keeps an empty final word, which is what a fresh Tab is.
	# The markers are escaped: `<...>` is a numeric-range glob to zsh.
	out=( ${(f)"$(run :complete $(( CURRENT - 1 )) "${(@)words}" 2>/dev/null)"} )
	if (( ${out[(I)\<dirs\>]} )); then
		out=( ${out:#\<dirs\>} )
		_files -/
	elif (( ${out[(I)\<files\>]} )); then
		out=( ${out:#\<files\>} )
		_files
	fi
	(( ${#out} )) && compadd -- $out
}
compdef _run run
"#;

const FISH: &str = r#"# run(1) completion. Install: run :completions install fish
#
# `-opc` gives the finished words only, so the count of them is the index of
# the one being completed. `-ct` is that word, which the binary needs too: a
# leading `:` or `-` is what tells it to offer commands or flags over targets.
function __run_complete
	set -l toks (commandline -opc)
	run :complete (count $toks) $toks (commandline -ct) 2>/dev/null
end

function __run_words
	__run_complete | string match -v -- '<files>' | string match -v -- '<dirs>'
end

function __run_wants_files
	__run_complete | string match -q -- '<files>'
end

function __run_wants_dirs
	__run_complete | string match -q -- '<dirs>'
end

complete -c run -f -a "(__run_words)"
complete -c run -F -n __run_wants_files
complete -c run -f -n __run_wants_dirs -a "(__fish_complete_directories)"
"#;

const POWERSHELL: &str = r#"# run(1) completion. Install: run :completions install powershell
Register-ArgumentCompleter -Native -CommandName run -ScriptBlock {
	param($wordToComplete, $commandAst, $cursorPosition)

	$words = @($commandAst.CommandElements | ForEach-Object { $_.ToString() })
	# The trailing word is only in the AST once it has a character; on an empty
	# one the position being completed is the next index along.
	$cword = if ($wordToComplete) { $words.Count - 1 } else { $words.Count }
	$out = @(& run :complete $cword @words 2>$null)

	if ($out -contains '<files>' -or $out -contains '<dirs>') {
		return [System.Management.Automation.CompletionCompleters]::CompleteFilename($wordToComplete)
	}
	$out | Where-Object { $_ -like "$wordToComplete*" } | ForEach-Object {
		[System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)
	}
}
"#;

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn every_supported_shell_produces_a_script() {
		for sh in ["bash", "zsh", "fish", "powershell", "pwsh"] {
			let s = script(sh).unwrap_or_else(|e| panic!("{sh}: {e}"));
			// Every script must ask the binary rather than carry its own copy of
			// the tree -- that is the whole point of there being one tree.
			assert!(s.contains("run :complete"), "{sh} must ask the binary");
			// zsh writes the markers escaped, since `<...>` is a glob to it.
			let plain = s.replace('\\', "");
			for m in [FILES, DIRS] {
				assert!(plain.contains(m), "{sh} ignores the {m} marker");
			}
		}
	}

	#[test]
	fn an_unknown_shell_lists_the_supported_ones() {
		let e = script("nushell").unwrap_err();
		assert!(e.contains("bash, zsh, fish, powershell"), "{e}");
	}

	#[test]
	fn every_advertised_command_is_offered_by_completion() {
		// The tree is written by hand, so it can drift from what the help
		// offers. This is the check that it has not.
		let usage = crate::usage();
		for c in ROOT.subs {
			assert!(usage.contains(c.name), "`{}` is completed but not in the help", c.name);
		}
		// And the other way, which is the direction a new command drifts in:
		// added to the help, forgotten in the tree.
		for word in usage.split_whitespace() {
			let word = word.trim_end_matches(|c: char| !c.is_alphanumeric());
			if word.starts_with(':') && word.len() > 1 {
				assert!(
					ROOT.subs.iter().any(|c| c.name == word),
					"`{word}` is in the help but completes to nothing"
				);
			}
		}
	}

	#[test]
	fn subcommands_complete_at_every_depth() {
		let none = || Vec::new();
		let at = |line: &str| {
			let words: Vec<String> = line.split(' ').map(String::from).collect();
			complete(&words, words.len() - 1, &none)
		};
		// One level, two levels, three -- the last is the one that used to fall
		// through to file names.
		assert!(at("run :env ").contains(&"secret-keys".to_string()));
		assert!(at("run :env secret-keys ").contains(&"get-private".to_string()));
		assert!(at("run :completions install ").contains(&"fish".to_string()));
		assert!(at("run :generate ").contains(&"jetbrains".to_string()));
	}

	#[test]
	fn a_flags_value_completes_as_its_own_word() {
		let none = || Vec::new();
		let words: Vec<String> = ["run", "--dir", ""].iter().map(|s| s.to_string()).collect();
		assert_eq!(complete(&words, 2, &none), vec![DIRS.to_string()]);
		// ...and the flag does not derail the walk that follows it.
		let words: Vec<String> = ["run", "--dir", "/tmp", ":env", ""]
			.iter()
			.map(|s| s.to_string())
			.collect();
		assert!(complete(&words, 4, &none).contains(&"inject".to_string()));
	}

	#[test]
	fn a_targets_own_arguments_are_left_alone() {
		// Past a target name the words belong to the target, and guessing at
		// them would offer confident nonsense.
		let targets = || vec!["build".to_string()];
		let words: Vec<String> = ["run", "build", ""].iter().map(|s| s.to_string()).collect();
		assert!(complete(&words, 2, &targets).is_empty());
	}

	#[test]
	fn the_first_word_offers_commands_and_targets_together() {
		let targets = || vec!["build".to_string()];
		let at = |cur: &str| {
			let words = vec!["run".to_string(), cur.to_string()];
			complete(&words, 1, &targets)
		};
		// A bare Tab shows both, as it always has. A command a person cannot
		// see without first guessing its `:` is a command they never find.
		let bare = at("");
		assert!(bare.contains(&":list".to_string()), "{bare:?}");
		assert!(bare.contains(&"build".to_string()), "{bare:?}");
		assert!(at(":").contains(&":list".to_string()));
		// Flags are the one list that waits to be asked for.
		assert!(at("--").contains(&"--dry-run".to_string()));
		assert!(!at("--").contains(&"build".to_string()));
	}
}
