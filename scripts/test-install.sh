#!/bin/sh
# Offline test of install.sh: builds a fake release from target/debug and checks that
# a good archive installs, a tampered archive installs nothing, and a missing target fails.
# Run after `cargo build --workspace`.
set -eu

root="$(cd "$(dirname "$0")/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT INT TERM

ok() { printf 'ok   %s\n' "$*"; }
bad() { printf 'FAIL %s\n' "$*" >&2; exit 1; }
sha() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d ' ' -f 1; else shasum -a 256 "$1" | cut -d ' ' -f 1; fi; }

# One archive per Unix target, all holding the binaries built on this machine, so the test
# does not have to repeat install.sh's platform detection.
make_release() { # dir
    mkdir -p "$1"
    : >"$1/SHA256SUMS"
    for t in x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-apple-darwin aarch64-apple-darwin; do
        name="boruna-0.0.0-test-$t"
        mkdir -p "$work/stage/$name"
        for b in boruna boruna-mcp boruna-pkg boruna-orch; do
            cp "$root/target/debug/$b" "$work/stage/$name/"
        done
        tar -czf "$1/$name.tar.gz" -C "$work/stage" "$name"
        printf '%s  %s\n' "$(sha "$1/$name.tar.gz")" "$name.tar.gz" >>"$1/SHA256SUMS"
    done
}

make_release "$work/good"
BORUNA_DOWNLOAD_BASE="file://$work/good" BORUNA_INSTALL_DIR="$work/bin" sh "$root/install.sh" >"$work/log" 2>&1 ||
    { cat "$work/log"; bad "good release did not install"; }
for b in boruna boruna-mcp boruna-pkg boruna-orch; do
    [ -x "$work/bin/$b" ] || bad "$b was not installed"
done
"$work/bin/boruna" --version | grep -q '^boruna ' || bad "installed boruna does not run"
ok "good release installs all four binaries"

cp -R "$work/good" "$work/tampered"
for f in "$work/tampered"/*.tar.gz; do printf 'x' >>"$f"; done
if BORUNA_DOWNLOAD_BASE="file://$work/tampered" BORUNA_INSTALL_DIR="$work/bin2" sh "$root/install.sh" >"$work/log" 2>&1; then
    bad "tampered archive was accepted"
fi
grep -q 'checksum mismatch' "$work/log" || { cat "$work/log"; bad "tampered archive failed for the wrong reason"; }
[ ! -e "$work/bin2" ] || bad "tampered archive left files behind"
ok "tampered archive is refused and nothing is installed"

mkdir -p "$work/empty"
printf '%s  %s\n' "0000" "boruna-0.0.0-test-sparc-unknown-none.tar.gz" >"$work/empty/SHA256SUMS"
if BORUNA_DOWNLOAD_BASE="file://$work/empty" BORUNA_INSTALL_DIR="$work/bin3" sh "$root/install.sh" >"$work/log" 2>&1; then
    bad "release without this platform was accepted"
fi
grep -q 'has no archive for' "$work/log" || { cat "$work/log"; bad "missing platform failed for the wrong reason"; }
ok "release without this platform fails with a clear message"
