#!/usr/bin/env bash
# Removes everything the `setup` action leaves on a runner *outside* the
# workspace:
#
#   1. The machine-global settings directory — `settings.json` (global files,
#      path aliases, custom shell paths) plus the `state.json` preparation-state
#      file that lives beside it.
#   2. `$RUNNER_TEMP/runfile-source/` — the Runfile and `.env` materialized from
#      the `runfile-source` / `env-file-source` inputs, i.e. from GitHub secrets.
#
# Shared by the `setup` action (run before it registers or materializes anything,
# so a job never inherits leftovers) and the `cleanup` action (run at the end of
# a job, so a job never leaves any behind). Both matter on self-hosted runners,
# where `$HOME` survives between jobs and `$RUNNER_TEMP` is not always cleared:
# a killed or cancelled job would otherwise leak its `global-files` registrations
# — and its plaintext secrets — into every later run on that machine.
#
# Deliberately NOT touched:
#   - The OS credential store. Secret keys reach CI through `RUNFILE_PRIVATE_KEYS`
#     (see the `secret-keys` input), so this action never adds keyring entries and
#     has no business deleting a machine's existing ones.
#   - `$RUNNER_TEMP/runfile-bin/`, the installed CLI. It holds nothing sensitive,
#     and removing it would break any job that placed the `cleanup` action
#     somewhere other than last.
set -euo pipefail

# --- 1. Machine-global settings ----------------------------------------------

# `run :config --path` is authoritative when the CLI is on PATH: it honours
# `RUNFILE_CONFIG_DIR` and any future relocation of the settings directory. It
# prints the `settings.json` path, so the directory is its parent.
dir=""
if command -v run >/dev/null 2>&1; then
	settings_file="$(run :config --path 2>/dev/null || true)"
	# Normalise before `dirname`: on Windows the CLI prints backslashes, which
	# `dirname` does not treat as separators (it would answer ".").
	settings_file="${settings_file//\\//}"
	settings_file="${settings_file%$'\r'}"
	[ -n "$settings_file" ] && dir="$(dirname "$settings_file")"
fi

# Fallback for when `run` is unavailable — the install step failed, or this is a
# cleanup step for a job that never got that far. Mirrors
# `runfile_settings::settings_dir()`.
if [ -z "$dir" ]; then
	if [ -n "${RUNFILE_CONFIG_DIR:-}" ]; then
		dir="$RUNFILE_CONFIG_DIR"
	else
		case "$(printf '%s' "${RUNNER_OS:-}" | tr '[:upper:]' '[:lower:]')" in
			windows) dir="${APPDATA:-$HOME/AppData/Roaming}/runfile" ;;
			macos)   dir="$HOME/Library/Application Support/runfile" ;;
			*)       dir="${XDG_CONFIG_HOME:-$HOME/.config}/runfile" ;;
		esac
	fi
fi

# MSYS tools (git-bash on Windows runners) accept `C:/x` but not `C:\x`.
dir="${dir//\\//}"

# Cheap blast-radius guard: a mis-resolved path must never turn into `rm -rf /`.
case "$dir" in
	"" | "/" | "$HOME" | "$HOME/")
		echo "Refusing to remove suspicious settings directory: '$dir'" >&2
		exit 1
		;;
esac

if [ -e "$dir" ]; then
	rm -rf "$dir"
	echo "Removed Runfile global settings: $dir"
else
	echo "No Runfile global settings to remove: $dir"
fi

# --- 2. Sources materialized from GitHub secrets ------------------------------

# Path kept in sync with the `Materialize Runfile source` / `Materialize env file
# source` steps in ../setup/action.yml. Both write into this one directory, so
# removing it covers both. An "encrypted" .env only has its *values* encrypted —
# the file is still secret material you would rather not leave on disk.
if [ -n "${RUNNER_TEMP:-}" ]; then
	sources="${RUNNER_TEMP//\\//}/runfile-source"
	if [ -e "$sources" ]; then
		rm -rf "$sources"
		echo "Removed materialized Runfile/env sources: $sources"
	else
		echo "No materialized Runfile/env sources to remove: $sources"
	fi
fi
