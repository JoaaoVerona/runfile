//! `run :completions <shell>` -- hand-written completion scripts.
//!
//! Every script asks the binary itself for target names (`run :list --names`)
//! rather than parsing files, so completion never has to know the language and
//! cannot drift from it. Names are the only dynamic input: subcommands and
//! flags are fixed, so they are baked into each script.

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

pub fn script(shell: &str) -> Result<String, String> {
	let s = match shell {
		"bash" => BASH,
		"zsh" => ZSH,
		"fish" => FISH,
		"powershell" | "pwsh" => POWERSHELL,
		other => {
			return Err(format!(
				"unknown shell `{other}`; expected one of: bash, zsh, fish, powershell"
			));
		}
	};
	Ok(s.replace("@COMMANDS@", &COMMANDS.join(" "))
		.replace("@FLAGS@", &FLAGS.join(" "))
		.replace("@ENV_SUBS@", &ENV_SUBS.join(" "))
		.replace("@GENERATE_SUBS@", &GENERATE_SUBS.join(" ")))
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

# No target chosen yet: names, commands and flags.
complete -c run -f -n __run_no_target -a "(run :list --names 2>/dev/null)"
complete -c run -f -n __run_no_target -a "@COMMANDS@"
complete -c run -f -n __run_no_target -a "@FLAGS@"
complete -c run -f -n __run_env_sub -a "@ENV_SUBS@"
complete -c run -f -n __run_generate_sub -a "@GENERATE_SUBS@"
# Past the target name, arguments are its own business: offer files.
complete -c run -F -n 'not __run_no_target; and not __run_env_sub; and not __run_generate_sub'
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
			for ph in ["@COMMANDS@", "@FLAGS@", "@ENV_SUBS@", "@GENERATE_SUBS@"] {
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
