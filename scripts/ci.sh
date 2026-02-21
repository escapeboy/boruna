#!/usr/bin/env bash
set -euo pipefail

echo "=== Boruna CI ==="
echo ""

# 1. Format check
echo "--- Format Check ---"
cargo fmt --all -- --check
echo "PASS: formatting"
echo ""

# 2. Clippy (zero warnings)
echo "--- Clippy ---"
cargo clippy --workspace -- -D warnings
echo "PASS: clippy"
echo ""

# 3. Build
echo "--- Build ---"
cargo build --workspace
echo "PASS: build"
echo ""

# 4. Unit + integration tests
echo "--- Tests ---"
cargo test --workspace
echo "PASS: tests"
echo ""

# 5. Workflow validation (all example workflows)
echo "--- Workflow Validation ---"
for dir in examples/workflows/*/; do
    name=$(basename "$dir")
    cargo run --bin boruna -- workflow validate "$dir"
    echo "  PASS: $name"
done
echo ""

# 6. Workflow execution (mock mode)
echo "--- Workflow Execution ---"
TMPDIR=$(mktemp -d)
trap "rm -rf $TMPDIR" EXIT

# Linear workflow: should complete
OUTPUT=$(cargo run --bin boruna -- workflow run examples/workflows/llm_code_review \
    --policy allow-all --record --evidence-dir "$TMPDIR/evidence" 2>&1)
echo "$OUTPUT" | grep -q "Completed"
echo "  PASS: llm_code_review (completed)"

# Fan-out workflow: should complete
OUTPUT=$(cargo run --bin boruna -- workflow run examples/workflows/document_processing \
    --policy allow-all 2>&1)
echo "$OUTPUT" | grep -q "Completed"
echo "  PASS: document_processing (completed)"

# Approval gate workflow: should pause
OUTPUT=$(cargo run --bin boruna -- workflow run examples/workflows/customer_support_triage \
    --policy allow-all 2>&1)
echo "$OUTPUT" | grep -q "Paused"
echo "  PASS: customer_support_triage (paused at approval)"

# 7. Evidence verification
echo ""
echo "--- Evidence Verification ---"
BUNDLE_DIR=$(ls -d "$TMPDIR/evidence"/run-* | head -1)
OUTPUT=$(cargo run --bin boruna -- evidence verify "$BUNDLE_DIR" 2>&1)
echo "$OUTPUT" | grep -q "VALID"
echo "  PASS: evidence bundle verified"

echo ""
echo "=== All CI checks passed ==="
