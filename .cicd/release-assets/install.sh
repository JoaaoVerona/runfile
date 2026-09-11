#!/bin/sh
set -eu

INSTALL_DIR="${RUNFILE_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${1:-latest}"

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
if [ "$VERSION" = "latest" ]; then
  page="$(curl -fsSL -o /dev/null -w '%{url_effective}' "$releases/latest")" || {
    echo "runfile: found no release on $server" >&2
    exit 1
  }
  case "$page" in
    */releases/tag/*) VERSION="${page##*/}" ;;
    *) echo "runfile: found no release on $server" >&2; exit 1 ;;
  esac
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $archive ($VERSION) from $server..."
curl -fsSL "$releases/download/$VERSION/$archive" -o "$tmp/$archive"
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
