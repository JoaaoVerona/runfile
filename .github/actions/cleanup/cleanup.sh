#!/usr/bin/env bash
# Removes everything the `setup` action leaves on a runner *outside* the
# workspace:
#
#   1. `$HOME/.runfiles/` — the machine-wide target directory, where the
#      `global-targets` and `runfile-source` inputs install their files.
#   2. The machine-local state directory, holding `state.json` (which
#      preparation targets have been run).
#   3. `$RUNNER_TEMP/runfile-source/` — the `.env` materialized from the
#      `env-file-source` input, i.e. from a GitHub secret.
#
# Shared by the `setup` action (run before it registers or materializes anything,
# so a job never inherits leftovers) and the `cleanup` action (run at the end of
# a job, so a job never leaves any behind). Both matter on self-hosted runners,
# where `$HOME` survives between jobs and `$RUNNER_TEMP` is not always cleared:
# a killed or cancelled job would otherwise leak its global targets — and its
# plaintext secrets — into every later run on that machine.
#
# Deliberately NOT touched:
#   - The OS credential store. Secret keys reach CI through `RUNFILE_PRIVATE_KEYS`
#     (see the `secret-keys` input), so this action never adds keyring entries and
#     has no business deleting a machine's existing ones.
#   - `$RUNNER_TEMP/runfile-bin/`, the installed CLI. It holds nothing sensitive,
#     and removing it would break any job that placed the `cleanup` action
#     somewhere other than last.
set -euo pipefail

# --- 1. Machine-wide targets --------------------------------------------------

# A fixed path, by design: there is no setting that moves it, so there is
# nothing to interrogate the CLI about.
globals="$HOME/.runfiles"
if [ -e "$globals" ]; then
	rm -rf "$globals"
	echo "Removed machine-wide targets: $globals"
else
	echo "No machine-wide targets to remove: $globals"
fi

# --- 2. Machine-local state ---------------------------------------------------

# Mirrors `runfile_settings::settings_dir()`. There is no CLI command to ask:
# `:config` was removed along with the settings file it managed.
if [ -n "${RUNFILE_CONFIG_DIR:-}" ]; then
	dir="$RUNFILE_CONFIG_DIR"
else
	case "$(printf '%s' "${RUNNER_OS:-}" | tr '[:upper:]' '[:lower:]')" in
		windows) dir="${APPDATA:-$HOME/AppData/Roaming}/runfile" ;;
		macos)   dir="$HOME/Library/Application Support/runfile" ;;
		*)       dir="${XDG_CONFIG_HOME:-$HOME/.config}/runfile" ;;
	esac
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
	echo "Removed Runfile state: $dir"
else
	echo "No Runfile state to remove: $dir"
fi

# --- 3. Sources materialized from GitHub secrets ------------------------------

# Path kept in sync with the `Materialize env file source` step in
# ../setup/action.yml. An "encrypted" .env only has its *values* encrypted — the
# file is still secret material you would rather not leave on disk.
if [ -n "${RUNNER_TEMP:-}" ]; then
	sources="${RUNNER_TEMP//\\//}/runfile-source"
	if [ -e "$sources" ]; then
		rm -rf "$sources"
		echo "Removed materialized env source: $sources"
	else
		echo "No materialized env source to remove: $sources"
	fi
fi
