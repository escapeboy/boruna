#!/usr/bin/env bash
# Pre-release validation script. Run BEFORE cutting a release tag.
#
# Usage:
#   scripts/pre-release-check.sh <version>
#
# Example:
#   scripts/pre-release-check.sh 1.0.0
#   scripts/pre-release-check.sh 1.0.0-rc3
#
# Exit codes:
#   0 — every check passed; safe to cut the tag.
#   1 — one or more checks failed; resolve before tagging.
#
# This script does NOT push, tag, or modify anything. It is read-only.

set -euo pipefail

if [ "${1:-}" = "" ]; then
  echo "ERROR: missing version argument" >&2
  echo "Usage: $0 <version>   (e.g. 1.0.0 or 1.0.0-rc3)" >&2
  exit 1
fi

VERSION="$1"
TAG="v$VERSION"
FAILED=0

ok()   { printf "  PASS  %s\n" "$1"; }
fail() { printf "  FAIL  %s\n" "$1" >&2; FAILED=$((FAILED + 1)); }
hdr()  { printf "\n--- %s ---\n" "$1"; }

cd "$(git rev-parse --show-toplevel)"

# 1. Repository state
hdr "Repository state"

if ! git diff-index --quiet HEAD --; then
  fail "working tree has uncommitted changes (run 'git status' to inspect)"
else
  ok "working tree clean"
fi

CURRENT_BRANCH=$(git branch --show-current)
if [ "$CURRENT_BRANCH" != "master" ] && [ "$CURRENT_BRANCH" != "main" ]; then
  fail "expected to be on master/main, on '$CURRENT_BRANCH'"
else
  ok "on $CURRENT_BRANCH"
fi

if git rev-parse "$TAG" >/dev/null 2>&1; then
  fail "tag $TAG already exists locally — choose a different version OR delete the local tag"
else
  ok "$TAG does not exist locally yet"
fi

if git ls-remote --tags origin "$TAG" 2>/dev/null | grep -q "$TAG"; then
  fail "$TAG already exists on origin — choose a different version"
else
  ok "$TAG does not exist on origin yet"
fi

# 2. Workspace version matches the requested tag
hdr "Cargo.toml version"

CARGO_VERSION=$(grep -E '^version = "' Cargo.toml | head -1 | sed -E 's/version = "(.*)"/\1/')
if [ "$CARGO_VERSION" != "$VERSION" ]; then
  fail "Cargo.toml [workspace.package].version is '$CARGO_VERSION', expected '$VERSION'"
  echo "    Fix: sed -i '' 's/version = \"$CARGO_VERSION\"/version = \"$VERSION\"/' Cargo.toml" >&2
else
  ok "Cargo.toml version = $VERSION"
fi

# 3. README badge
hdr "README version badge"

README_DASHES_VERSION=$(echo "$VERSION" | sed 's/-/--/g')
# Color is informational (orange = rc, blue = stable, red = security
# advisory, etc.); accept any color so a GA cut isn't blocked by the
# rc badge change.
if grep -E "version-${README_DASHES_VERSION}-(orange|blue|green|red|yellow|brightgreen|lightgrey)" README.md >/dev/null 2>&1; then
  ok "README badge matches $VERSION"
else
  fail "README badge does NOT mention $VERSION (looking for: 'version-${README_DASHES_VERSION}-<color>')"
  echo "    Fix: update the version badge line in README.md" >&2
fi

# 4. CHANGELOG section exists and is non-empty
hdr "CHANGELOG section"

# Extract the section between "## [VERSION]" and the next "## [" header.
SECTION=$(awk -v ver="$VERSION" '
  /^## \[/ {
    if (in_section) exit
    if ($0 ~ "^## \\[" ver "\\]") { in_section = 1; next }
  }
  in_section { print }
' CHANGELOG.md)

if [ -z "$SECTION" ]; then
  fail "no CHANGELOG section for [$VERSION]"
  echo "    Fix: add a '## [$VERSION] - YYYY-MM-DD' section to CHANGELOG.md before tagging" >&2
elif [ "$(echo "$SECTION" | tr -d '[:space:]')" = "" ]; then
  fail "CHANGELOG section for [$VERSION] is whitespace-only"
else
  LINES=$(echo "$SECTION" | wc -l | tr -d ' ')
  ok "CHANGELOG section [$VERSION] has $LINES lines of content"
fi

# 5. Versioned-spec constants match expectations
hdr "Versioned spec constants"

check_const() {
  local file="$1"
  local pattern="$2"
  local name="$3"
  if grep -E "$pattern" "$file" >/dev/null 2>&1; then
    ok "$name in $file"
  else
    fail "$name NOT found in $file (pattern: $pattern)"
  fi
}

check_const "crates/llmc/src/lib.rs" \
  'pub const LANGUAGE_VERSION: &str = "1\.0"' \
  "LANGUAGE_VERSION = 1.0"
check_const "crates/llmbc/src/lib.rs" \
  'pub const BYTECODE_VERSION: &str = "1\.0"' \
  "BYTECODE_VERSION = 1.0"
check_const "orchestrator/src/audit/mod.rs" \
  'pub const BUNDLE_FORMAT_VERSION: &str = "1\.0"' \
  "BUNDLE_FORMAT_VERSION = 1.0"
check_const "orchestrator/src/workflow/definition.rs" \
  'pub const WORKFLOW_DAG_SCHEMA_VERSION: u32 = 1' \
  "WORKFLOW_DAG_SCHEMA_VERSION = 1"

# 6. Build + test gates (the same gates CI runs)
hdr "Build + test gates"

echo "  running cargo fmt --all -- --check ..."
if cargo fmt --all -- --check >/dev/null 2>&1; then
  ok "fmt clean"
else
  fail "fmt not clean — run 'cargo fmt --all'"
fi

echo "  running cargo build --workspace --features boruna-cli/serve ..."
if cargo build --workspace --features boruna-cli/serve >/dev/null 2>&1; then
  ok "build clean"
else
  fail "build failed — run 'cargo build --workspace --features boruna-cli/serve' to see errors"
fi

echo "  running cargo clippy --workspace --features boruna-cli/serve --all-targets -- -D warnings ..."
if cargo clippy --workspace --features boruna-cli/serve --all-targets -- -D warnings >/dev/null 2>&1; then
  ok "clippy clean"
else
  fail "clippy has warnings — run with the same args to see them"
fi

echo "  running cargo test --workspace --features boruna-cli/serve ..."
if cargo test --workspace --features boruna-cli/serve >/dev/null 2>&1; then
  ok "tests pass"
else
  fail "tests failed — run with the same args to see failures"
fi

echo "  running cargo bench -p boruna-benches --no-run ..."
if cargo bench -p boruna-benches --no-run >/dev/null 2>&1; then
  ok "benches compile"
else
  fail "bench compile failed"
fi

# 7. Smoke tests — example workflows run end-to-end
hdr "Examples smoke gate (matches CI)"

SMOKE_DIR=$(mktemp -d)
trap 'rm -rf "$SMOKE_DIR"' EXIT

for dir in examples/workflows/*/; do
  example=$(basename "$dir")
  DATA_DIR="$SMOKE_DIR/$example"
  EV_DIR="$DATA_DIR/evidence"
  mkdir -p "$DATA_DIR"
  if cargo run --bin boruna -- workflow run "$dir" \
      --policy allow-all --record \
      --data-dir "$DATA_DIR" --evidence-dir "$EV_DIR" >/dev/null 2>&1; then
    run_id=$(ls "$EV_DIR" 2>/dev/null | head -1)
    if [ -z "$run_id" ]; then
      fail "$example: no run produced"
    elif cargo run --bin boruna -- evidence verify "$EV_DIR/$run_id" 2>&1 | grep -q "VALID"; then
      ok "$example end-to-end + verify"
    else
      fail "$example: bundle did not verify"
    fi
  else
    fail "$example: workflow run failed"
  fi
done

# 8. Summary
hdr "Summary"

if [ "$FAILED" -eq 0 ]; then
  echo ""
  echo "All pre-release checks passed for $TAG."
  echo ""
  echo "Next steps:"
  echo "  git commit -am 'release: $TAG'"
  echo "  git tag -a $TAG -m 'Boruna $TAG'"
  echo "  git push origin $CURRENT_BRANCH $TAG"
  echo ""
  exit 0
else
  echo ""
  echo "$FAILED pre-release check(s) failed."
  echo "   Resolve the issues above before tagging $TAG."
  echo ""
  exit 1
fi
