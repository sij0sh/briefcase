#!/bin/sh
# briefcase installer (POSIX sh).
# Usage: curl -fsSL https://raw.githubusercontent.com/<owner>/<repo>/main/scripts/install.sh | GITHUB_REPO=<owner>/<repo> sh
# Args (piped): ... | sh -s -- --repo <owner/repo> --dir <path> --version <tag|latest>
set -eu
(set -o pipefail) 2>/dev/null && set -o pipefail

REPO="${GITHUB_REPO:-sij0sh/briefcase}"
TAG="latest"
INSTALL_DIR="${BRIEFCASE_INSTALL_DIR:-$HOME/.local/bin}"

usage() {
    cat <<EOF
briefcase installer

Usage:
  curl -fsSL https://raw.githubusercontent.com/$REPO/main/scripts/install.sh | sh
  install.sh --help | --repo <owner/repo> | --dir <path> | --version <tag|latest>

Options:
  --repo <owner/repo>     GitHub repository that publishes the releases.
  --dir <path>            Install directory (default: \$HOME/.local/bin,
                          overridable via BRIEFCASE_INSTALL_DIR).
  --version <tag|latest>  Release tag to install (default: latest).
  --help                  Show this help.

Environment:
  GITHUB_REPO             Repository to download from (default: sij0sh/briefcase).
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --help|-h)
            usage
            exit 0
            ;;
        --repo)
            [ $# -ge 2 ] || { echo "error: --repo requires owner/repo" >&2; exit 1; }
            REPO="$2"
            shift 2
            ;;
        --dir)
            [ $# -ge 2 ] || { echo "error: --dir requires a path" >&2; exit 1; }
            INSTALL_DIR="$2"
            shift 2
            ;;
        --version)
            [ $# -ge 2 ] || { echo "error: --version requires a tag or 'latest'" >&2; exit 1; }
            TAG="$2"
            shift 2
            ;;
        *)
            echo "error: unknown argument: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

[ -n "$REPO" ] || { echo "error: set GITHUB_REPO or pass --repo <owner/repo>" >&2; exit 1; }

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
    Darwin|Linux) ;;
    *)
        echo "error: unsupported OS '$os'. On Windows use install.ps1:" >&2
        echo "  https://github.com/$REPO#installation" >&2
        exit 1
        ;;
esac

case "$arch" in
    x86_64|amd64) arch="x86_64" ;;
    arm64|aarch64) arch="aarch64" ;;
    *)
        echo "error: unsupported architecture '$arch'" >&2
        exit 1
        ;;
esac

case "$os:$arch" in
    Darwin:x86_64)  TARGET="x86_64-apple-darwin" ;;
    Darwin:aarch64) TARGET="aarch64-apple-darwin" ;;
    Linux:x86_64)   TARGET="x86_64-unknown-linux-gnu" ;;
    Linux:aarch64)  TARGET="aarch64-unknown-linux-gnu" ;;
esac

if [ "$TAG" = "latest" ]; then
    url="https://github.com/$REPO/releases/latest/download/briefcase-$TARGET.tar.gz"
else
    url="https://github.com/$REPO/releases/download/$TAG/briefcase-$TARGET.tar.gz"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
trap 'exit 1' INT TERM

archive="$tmp/briefcase-$TARGET.tar.gz"
echo "Downloading briefcase ($TARGET) from $url"

if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "$archive" "$url" || { echo "error: download failed: $url" >&2; exit 1; }
elif command -v wget >/dev/null 2>&1; then
    wget -O "$archive" "$url" || { echo "error: download failed: $url" >&2; exit 1; }
else
    echo "error: curl or wget is required to download" >&2
    exit 1
fi

[ -s "$archive" ] || { echo "error: asset not found or empty: $url" >&2; exit 1; }

checksum_url="${url%/*}/SHA256SUMS"
checksums="$tmp/SHA256SUMS"
if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "$checksums" "$checksum_url"
else
    wget -O "$checksums" "$checksum_url"
fi
expected="$(awk -v name="$(basename "$archive")" '$2 == name {print $1}' "$checksums")"
[ "${#expected}" -eq 64 ] || { echo "error: missing or ambiguous archive checksum" >&2; exit 1; }
if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$archive" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$archive" | awk '{print $1}')"
else
    echo "error: sha256sum or shasum is required" >&2
    exit 1
fi
[ "$actual" = "$expected" ] || { echo "error: archive checksum mismatch" >&2; exit 1; }

tar -xzf "$archive" -C "$tmp"

bin=""
if [ -f "$tmp/briefcase" ]; then
    bin="$tmp/briefcase"
else
    for f in "$tmp"/*/briefcase; do
        if [ -f "$f" ]; then
            bin="$f"
            break
        fi
    done
fi
[ -n "$bin" ] || { echo "error: briefcase binary not found in archive" >&2; exit 1; }

mkdir -p "$INSTALL_DIR"
mv "$bin" "$INSTALL_DIR/briefcase"
chmod +x "$INSTALL_DIR/briefcase"

case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
        echo ""
        echo "NOTE: $INSTALL_DIR is not on your PATH."
        echo "Add this line to ~/.bashrc, ~/.zshrc, or your shell's profile:"
        echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
        ;;
esac

if "$INSTALL_DIR/briefcase" --version >/dev/null 2>&1; then
    echo "Installed: $("$INSTALL_DIR/briefcase" --version 2>/dev/null | head -n 1)"
else
    echo "Installed to $INSTALL_DIR/briefcase (could not run --version locally)"
fi

echo ""
echo "Next steps:"
echo "  1. Open a new terminal (or: export PATH=\"$INSTALL_DIR:\$PATH\")."
echo "  2. Run: briefcase install --agent pi --dry-run, then without --dry-run"
echo "  3. In each repository, run pi (or: briefcase sync)"
