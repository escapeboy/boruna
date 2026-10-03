#!/bin/sh
# Install the Boruna binaries (boruna, boruna-mcp, boruna-pkg, boruna-orch) from a GitHub release.
#
#   curl -fsSL https://raw.githubusercontent.com/escapeboy/boruna/master/install.sh | sh
#
# Linux (x86_64, arm64) and macOS (Apple Silicon, Intel). On Windows use install.ps1.
# The archive is checked against the release's SHA256SUMS before anything is installed.
#
# Environment:
#   BORUNA_VERSION       release tag to install, e.g. v3.4.0 (default: latest)
#   BORUNA_INSTALL_DIR   where the binaries go (default: $HOME/.local/bin)
#   BORUNA_DOWNLOAD_BASE base URL holding SHA256SUMS and the archives (default: the GitHub release)
set -eu

REPO="escapeboy/boruna"
BINARIES="boruna boruna-mcp boruna-pkg boruna-orch"

say() { printf '%s\n' "$*"; }
fail() { printf 'install.sh: error: %s\n' "$*" >&2; exit 1; }

detect_target() {
    os="$(uname -s)"
    arch="$(uname -m)"
    case "$arch" in
        x86_64 | amd64) arch="x86_64" ;;
        arm64 | aarch64) arch="aarch64" ;;
        *) fail "unsupported CPU architecture '$arch' (supported: x86_64, arm64). Build from source: https://github.com/$REPO" ;;
    esac
    case "$os" in
        Linux) printf '%s-unknown-linux-musl' "$arch" ;;
        Darwin) printf '%s-apple-darwin' "$arch" ;;
        MINGW* | MSYS* | CYGWIN*) fail "on Windows use install.ps1 (see the README)" ;;
        *) fail "unsupported operating system '$os' (supported: Linux, macOS). Build from source: https://github.com/$REPO" ;;
    esac
}

download() { # url dest
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --retry 3 -o "$2" "$1" || fail "download failed: $1"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$2" "$1" || fail "download failed: $1"
    else
        fail "need curl or wget"
    fi
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d ' ' -f 1
    else
        fail "need sha256sum or shasum to verify the download"
    fi
}

main() {
    target="$(detect_target)"
    version="${BORUNA_VERSION:-latest}"
    install_dir="${BORUNA_INSTALL_DIR:-$HOME/.local/bin}"
    if [ -n "${BORUNA_DOWNLOAD_BASE:-}" ]; then
        base="$BORUNA_DOWNLOAD_BASE"
    elif [ "$version" = "latest" ]; then
        base="https://github.com/$REPO/releases/latest/download"
    else
        base="https://github.com/$REPO/releases/download/$version"
    fi

    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT INT TERM

    say "Installing Boruna ($version) for $target"
    download "$base/SHA256SUMS" "$tmp/SHA256SUMS"

    # Lines look like "<hash>  <file>" or "<hash> *<file>" (binary mode on Windows builders).
    line="$(grep -- "-$target\.tar\.gz\$" "$tmp/SHA256SUMS" || true)"
    [ -n "$line" ] || fail "release $version has no archive for $target"
    expected="$(printf '%s' "$line" | cut -d ' ' -f 1)"
    archive="$(printf '%s' "$line" | sed 's/^[0-9a-f]* [ *]//')"

    download "$base/$archive" "$tmp/$archive"
    actual="$(sha256_of "$tmp/$archive")"
    [ "$actual" = "$expected" ] || fail "checksum mismatch for $archive (expected $expected, got $actual); nothing was installed"
    say "Checksum OK: $archive"

    tar -xzf "$tmp/$archive" -C "$tmp"
    src="$tmp/${archive%.tar.gz}"
    mkdir -p "$install_dir"
    for bin in $BINARIES; do
        [ -f "$src/$bin" ] || fail "archive is missing $bin"
        cp "$src/$bin" "$install_dir/$bin.tmp"
        chmod 755 "$install_dir/$bin.tmp"
        mv -f "$install_dir/$bin.tmp" "$install_dir/$bin"
    done

    say "Installed to $install_dir: $BINARIES"
    "$install_dir/boruna" --version
    case ":$PATH:" in
        *":$install_dir:"*) ;;
        *) say "Note: $install_dir is not on your PATH. Add this to your shell profile:"
           say "  export PATH=\"$install_dir:\$PATH\"" ;;
    esac
}

main "$@"
