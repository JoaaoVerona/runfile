//! `run :completions <shell>` -- hand-written completion scripts.
//!
//! `install` and `uninstall` put the hook into the shell's own profile, or into
//! fish's completions directory, and take it out again -- both idempotent, both
//! leaving everything else in the file alone.
//!
//! Every script asks the binary itself for target names (`run :list --names`)
//! rather than parsing files, so completion never has to know the language and
//! cannot drift from it. Names are the only dynamic input: subcommands and
//! flags are fixed, so they are baked into each script.

use std::path::{Path, PathBuf};

/// Subcommands offered when the current word starts with `:`.
pub const COMMANDS: &[&str] = &[":list", ":env", ":update", ":completions", ":init", ":version"];
/// Flags offered before the target name.
pub const FLAGS: &[&str] = &["-y", "--yes", "--stdin-args", "--dry-run", "--dir"];
/// `:env` subcommands, the one nested level that exists.
pub const ENV_SUBS: &[&str] = &[
	"init",
	"get",
	"set",
	"encrypt",
	"decrypt",
	"rotate",
	"inject",
	"secret-keys",
];
/// `:generate` editors, the other nested level.
pub const GENERATE_SUBS: &[&str] = crate::cmd_generate::SUBS;
/// What `:completions` accepts: the two actions, then the shells.
pub const COMPLETION_SUBS: &[&str] = &["install", "uninstall", "bash", "zsh", "fish", "powershell"];

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

fn destination(shell: &str) -> Result<Where, String> {
	Ok(match shell {
		"bash" => Where::Profile(home()?.join(".bashrc"), r#"eval "$(run :completions bash)""#.into()),
		"zsh" => Where::Profile(home()?.join(".zshrc"), r#"eval "$(run :completions zsh)""#.into()),
		"fish" => Where::File(config_dir()?.join("fish/completions/run.fish")),
		"powershell" | "pwsh" => Where::Profile(
			powershell_profile()?,
			"run :completions powershell | Out-String | Invoke-Expression".into(),
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

pub fn script(shell: &str) -> Result<String, String> {
	let s = match shell {
		"bash" => BASH,
		"zsh" => ZSH,
		"fish" => FISH,
		"powershell" | "pwsh" => POWERSHELL,
		other => return Err(unknown(other)),
	};
	Ok(s.replace("@COMMANDS@", &COMMANDS.join(" "))
		.replace("@FLAGS@", &FLAGS.join(" "))
		.replace("@ENV_SUBS@", &ENV_SUBS.join(" "))
		.replace("@GENERATE_SUBS@", &GENERATE_SUBS.join(" "))
		.replace("@COMPLETION_SUBS@", &COMPLETION_SUBS.join(" ")))
}

const BASH: &str = r#"# run(1) completion. Install: eval "$(run :completions bash)"
_run() {
	local cur prev words cword
	cur="${COMP_WORDS[COMP_CWORD]}"
	prev="${COMP_WORDS[COMP_CWORD-1]}"

	# --dir takes a directory, whatever position it is in.
	if [[ "$prev" == "--dir" ]]; then
		COMPREPLY=( $(compgen -d -- "$cur") )
		return
	fi

	# Find the first word that is neither a flag nor a flag's value.
	local i first=""
	for (( i=1; i < COMP_CWORD; i++ )); do
		case "${COMP_WORDS[i]}" in
			--dir) (( i++ ));;
			-*) ;;
			*) first="${COMP_WORDS[i]}"; break;;
		esac
	done

	# Two commands take a subcommand: one nested level, then file names.
	local subs=""
	case "$first" in
		:env) subs="@ENV_SUBS@" ;;
		:generate) subs="@GENERATE_SUBS@" ;;
		:completions) subs="@COMPLETION_SUBS@" ;;
	esac
	if [[ -n "$subs" ]]; then
		if [[ $(( COMP_CWORD - i )) -eq 1 ]]; then
			COMPREPLY=( $(compgen -W "$subs" -- "$cur") )
		else
			COMPREPLY=( $(compgen -f -- "$cur") )
		fi
		return
	fi

	# Past the target name: its arguments are the target's business, so offer
	# files rather than guessing.
	if [[ -n "$first" ]]; then
		COMPREPLY=( $(compgen -f -- "$cur") )
		return
	fi

	if [[ "$cur" == :* ]]; then
		COMPREPLY=( $(compgen -W "@COMMANDS@" -- "$cur") )
	elif [[ "$cur" == -* ]]; then
		COMPREPLY=( $(compgen -W "@FLAGS@" -- "$cur") )
	else
		COMPREPLY=( $(compgen -W "$(run :list --names 2>/dev/null)" -- "$cur") )
	fi
}
complete -F _run run
"#;

const ZSH: &str = r#"# run(1) completion. Install: eval "$(run :completions zsh)"
_run() {
	local -a words_before
	local first="" i
	for (( i = 2; i < CURRENT; i++ )); do
		case "${words[i]}" in
			--dir) (( i++ ));;
			-*) ;;
			*) first="${words[i]}"; break;;
		esac
	done

	if [[ "${words[CURRENT-1]}" == "--dir" ]]; then
		_files -/
		return
	fi

	local subs=""
	case "$first" in
		:env) subs="@ENV_SUBS@" ;;
		:generate) subs="@GENERATE_SUBS@" ;;
		:completions) subs="@COMPLETION_SUBS@" ;;
	esac
	if [[ -n "$subs" ]]; then
		if (( CURRENT - i == 1 )); then
			compadd -- ${=subs}
		else
			_files
		fi
		return
	fi

	if [[ -n "$first" ]]; then
		_files
		return
	fi

	case "$PREFIX" in
		:*) compadd -- @COMMANDS@ ;;
		-*) compadd -- @FLAGS@ ;;
		*)  compadd -- ${(f)"$(run :list --names 2>/dev/null)"} ;;
	esac
}
compdef _run run
"#;

const FISH: &str = r#"# run(1) completion. Install: run :completions fish > ~/.config/fish/completions/run.fish
function __run_first_word
	set -l toks (commandline -opc)
	set -l skip 0
	for tok in $toks[2..-1]
		if test $skip -eq 1
			set skip 0
			continue
		end
		switch $tok
			case --dir
				set skip 1
			case '-*'
			case '*'
				echo $tok
				return
		end
	end
end

function __run_no_target
	test -z (__run_first_word)
end

function __run_env_sub
	test (__run_first_word) = ":env"; and test (count (commandline -opc)) -eq 2
end

function __run_generate_sub
	test (__run_first_word) = ":generate"; and test (count (commandline -opc)) -eq 2
end

function __run_completions_sub
	test (__run_first_word) = ":completions"; and test (count (commandline -opc)) -eq 2
end

# No target chosen yet: names, commands and flags.
complete -c run -f -n __run_no_target -a "(run :list --names 2>/dev/null)"
complete -c run -f -n __run_no_target -a "@COMMANDS@"
complete -c run -f -n __run_no_target -a "@FLAGS@"
complete -c run -f -n __run_env_sub -a "@ENV_SUBS@"
complete -c run -f -n __run_generate_sub -a "@GENERATE_SUBS@"
complete -c run -f -n __run_completions_sub -a "@COMPLETION_SUBS@"
# Past the target name, arguments are its own business: offer files.
complete -c run -F -n 'not __run_no_target; and not __run_env_sub; and not __run_generate_sub; and not __run_completions_sub'
complete -c run -r -n '__fish_seen_argument -l dir' -a "(__fish_complete_directories)"
"#;

const POWERSHELL: &str = r#"# run(1) completion. Install: run :completions powershell | Out-String | Invoke-Expression
Register-ArgumentCompleter -Native -CommandName run -ScriptBlock {
	param($wordToComplete, $commandAst, $cursorPosition)

	$words = @($commandAst.CommandElements | Select-Object -Skip 1 | ForEach-Object { $_.ToString() })
	$first = $null
	$skip = $false
	foreach ($w in $words) {
		if ($skip) { $skip = $false; continue }
		if ($w -eq '--dir') { $skip = $true; continue }
		if ($w.StartsWith('-')) { continue }
		$first = $w
		break
	}

	$emit = {
		param($items)
		$items | Where-Object { $_ -like "$wordToComplete*" } | ForEach-Object {
			[System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)
		}
	}

	if ($first -eq ':env' -and $words.Count -le 2) { return & $emit @('@ENV_SUBS@'.Split(' ')) }
	if ($first -eq ':generate' -and $words.Count -le 2) { return & $emit @('@GENERATE_SUBS@'.Split(' ')) }
	if ($first -eq ':completions' -and $words.Count -le 2) { return & $emit @('@COMPLETION_SUBS@'.Split(' ')) }
	if ($first) { return [System.Management.Automation.CompletionCompleters]::CompleteFilename($wordToComplete) }
	if ($wordToComplete.StartsWith(':')) { return & $emit @('@COMMANDS@'.Split(' ')) }
	if ($wordToComplete.StartsWith('-')) { return & $emit @('@FLAGS@'.Split(' ')) }
	& $emit @(& run :list --names 2>$null)
}
"#;

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn every_supported_shell_produces_a_script() {
		for sh in ["bash", "zsh", "fish", "powershell", "pwsh"] {
			let s = script(sh).unwrap_or_else(|e| panic!("{sh}: {e}"));
			assert!(s.contains("run :list --names"), "{sh} must ask the binary for names");
			// `@` alone is legal PowerShell array syntax, so check the actual
			// placeholder spellings.
			for ph in [
				"@COMMANDS@",
				"@FLAGS@",
				"@ENV_SUBS@",
				"@GENERATE_SUBS@",
				"@COMPLETION_SUBS@",
			] {
				assert!(!s.contains(ph), "{sh} left {ph} unsubstituted");
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
		// The scripts bake the command list in, so it can drift from `main`'s
		// dispatch. This is the check that it has not.
		let usage = crate::USAGE;
		for c in COMMANDS {
			assert!(usage.contains(c), "`{c}` is completed but not in the usage text");
		}
	}
}
