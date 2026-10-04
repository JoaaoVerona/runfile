#!/bin/sh
set -eu

INSTALL_DIR="${RUNFILE_INSTALL_DIR:-$HOME/.local/bin}"
# Version precedence: the argument, then RUNFILE_VERSION (`curl … | sh` passes
# no argument), then latest -- the order install.ps1 has always had. This read
# the argument alone, so the RUNFILE_VERSION the README documents installed
# the newest release instead (audit SA-036).
VERSION="${1:-${RUNFILE_VERSION:-latest}}"

# The version goes into the download URL's path, so it has to be a release tag
# and nothing else: curl removes dot-segments before it sends a request, and a
# `/` or a `..` in the version reached somewhere other than these releases
# (audit SA-032). The characters are spelled out rather than ranged so that no
# locale can widen them, and a newline is not one of them: grep matches a line.
is_release_tag() {
  case "$1" in
    *[!0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz.-]*) return 1 ;;
  esac
  printf '%s\n' "$1" | LC_ALL=C grep -Eqx 'v?[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?'
}

if [ "$VERSION" != latest ]; then
  if ! is_release_tag "$VERSION"; then
    # Shown without its control characters, so it cannot break the line.
    shown="$(printf '%s' "$VERSION" | tr -d '[:cntrl:]')"
    echo "runfile: invalid version: $shown (expected latest, or a release tag such as v1.2.3)" >&2
    exit 1
  fi
  # Every release is tagged with the `v`, as `run :update` assumes too.
  case "$VERSION" in v*) ;; *) VERSION="v$VERSION" ;; esac
fi

# Where releases come from. Gitea cuts every one; GitHub mirrors them when the
# mirror is pushed, so it can lag. `run :update --channel=github` sets this.
case "${RUNFILE_CHANNEL:-gitea}" in
  gitea)  releases="https://git.joaoverona.com/joaaoverona/runfile/releases" ;;
  github) releases="https://github.com/JoaaoVerona/runfile/releases" ;;
  *) echo "runfile: unknown channel: $RUNFILE_CHANNEL (gitea or github)" >&2; exit 1 ;;
esac
server="$(printf '%s' "$releases" | cut -d/ -f3)"

case "$(uname -s)" in
  Linux*)  os="unknown-linux-musl" ;;
  Darwin*) os="apple-darwin" ;;
  *) echo "runfile: unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac

case "$(uname -m)" in
  x86_64|amd64)  arch="x86_64" ;;
  aarch64|arm64) arch="aarch64" ;;
  *) echo "runfile: unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

target="${arch}-${os}"
archive="runfile-cli-${target}.tar.xz"

# Both hosts answer /releases/latest with a redirect to the newest release's tag
# page, so where it leads names the version, and the archive is then downloaded
# by name: /releases/download/<tag>/<asset> is the one shape the two share.
# Their own `latest` download aliases are not -- Gitea's is
# /releases/download/latest/<asset>, GitHub's /releases/latest/download/<asset>.
#
# `--path-as-is` sends a path exactly as built, and `--proto` keeps every hop,
# redirects included, on https.
if [ "$VERSION" = "latest" ]; then
  page="$(curl -fsSL --proto '=https' --path-as-is -o /dev/null -w '%{url_effective}' "$releases/latest")" || {
    echo "runfile: found no release on $server" >&2
    exit 1
  }
  case "$page" in
    */releases/tag/*) VERSION="${page##*/}" ;;
    *) echo "runfile: found no release on $server" >&2; exit 1 ;;
  esac
  # The tag it names goes into a URL as well.
  is_release_tag "$VERSION" || {
    echo "runfile: $releases/latest led to $page, which is not a release" >&2
    exit 1
  }
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $archive ($VERSION) from $server..."
curl -fsSL --proto '=https' --path-as-is "$releases/download/$VERSION/$archive" -o "$tmp/$archive"
tar -xJf "$tmp/$archive" -C "$tmp"

mkdir -p "$INSTALL_DIR"
mv "$tmp/runfile-cli-${target}/run" "$INSTALL_DIR/run"
chmod +x "$INSTALL_DIR/run"

echo "Installed run $VERSION to $INSTALL_DIR/run"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    echo
    echo "Add $INSTALL_DIR to your PATH:"
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
    ;;
esac
