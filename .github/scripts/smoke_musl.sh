#!/usr/bin/env bash
#
# Smoke-test a published Boruna release artifact for one Linux musl
# target. Invoked from .github/workflows/smoke-musl.yml.
#
# Usage:
#   smoke_musl.sh <tag> <target-triple> <docker-image> <kind>
#
# Where:
#   <tag>            e.g. v1.0.0-rc2
#   <target-triple>  e.g. x86_64-unknown-linux-musl
#   <docker-image>   e.g. alpine:3.19 or arm64v8/alpine:3.19
#   <kind>           "native" (real platform) or "emulated" (qemu)
#
# Outputs `docs/release-smoke-tests/<tag>-musl-<arch>.md`.
#
# The shape of the report mirrors `docs/release-smoke-tests/v1.0.0-rc2.md`
# (the macOS arm64 reference) — sections: Scope, Steps performed,
# Findings, Verdict.

set -euo pipefail

TAG="${1:?missing tag}"
TARGET="${2:?missing target triple}"
IMAGE="${3:?missing docker image}"
KIND="${4:?missing kind (native|emulated)}"

case "$TARGET" in
  x86_64-unknown-linux-musl) ARCH="x86_64" ;;
  aarch64-unknown-linux-musl) ARCH="aarch64" ;;
  *) echo "ERROR: unsupported target $TARGET" >&2; exit 1 ;;
esac

# Strip the leading 'v' so version-bearing filenames match the
# release-pipeline's naming convention.
VERSION="${TAG#v}"
ARTIFACT="boruna-${VERSION}-${TARGET}.tar.gz"
RELEASE_URL="https://github.com/escapeboy/boruna/releases/download/${TAG}"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
REPORT_DIR="$REPO_ROOT/docs/release-smoke-tests"
mkdir -p "$REPORT_DIR"
REPORT="$REPORT_DIR/${TAG}-musl-${ARCH}.md"

cd "$WORK"

echo "::group::download $ARTIFACT"
curl -sSLf -o "$ARTIFACT" "$RELEASE_URL/$ARTIFACT"
curl -sSLf -o SHA256SUMS "$RELEASE_URL/SHA256SUMS"
echo "::endgroup::"

# Extract the line for our artifact and verify.
EXPECTED=$(grep -E "  ${ARTIFACT}\$" SHA256SUMS | awk '{print $1}')
COMPUTED=$(sha256sum "$ARTIFACT" | awk '{print $1}')

if [ "$EXPECTED" != "$COMPUTED" ]; then
  echo "ERROR: SHA256 mismatch" >&2
  echo "  expected: $EXPECTED" >&2
  echo "  computed: $COMPUTED" >&2
  exit 1
fi

echo "::group::extract"
mkdir -p extracted
tar -xzf "$ARTIFACT" -C extracted
ls -la extracted
echo "::endgroup::"

# Run the binary inside the target container so the libc/loader is
# the actual musl runtime, not the host's glibc. We bind-mount the
# extracted dir and the workflow examples.
echo "::group::run in $IMAGE"
docker pull "$IMAGE"

# Capture binary version + capability list.
VERSION_OUT=$(docker run --rm \
  -v "$WORK/extracted:/boruna:ro" \
  "$IMAGE" /boruna/boruna --version)

CAP_OUT=$(docker run --rm \
  -v "$WORK/extracted:/boruna:ro" \
  "$IMAGE" /boruna/boruna capability list 2>&1 || true)

# Run an example workflow + verify evidence.
WORKFLOW_OUT=$(docker run --rm \
  -v "$WORK/extracted:/boruna:ro" \
  -v "$REPO_ROOT/examples:/examples:ro" \
  "$IMAGE" sh -c '
    set -e
    mkdir -p /tmp/data /tmp/evidence
    /boruna/boruna workflow run /examples/workflows/llm_code_review \
      --policy allow-all \
      --record \
      --data-dir /tmp/data \
      --evidence-dir /tmp/evidence
    run_id=$(ls /tmp/evidence | head -1)
    /boruna/boruna evidence verify "/tmp/evidence/$run_id"
  ' 2>&1)
echo "::endgroup::"

# Capture command output as files so we can sed-prefix them into the
# report block without losing line breaks.
echo "$CAP_OUT" | head -20 | sed 's/^/   /' > cap_out.indented
echo "$WORKFLOW_OUT" | tail -20 | sed 's/^/   /' > workflow_out.indented
CAP_BLOCK=$(cat cap_out.indented)
WORKFLOW_BLOCK=$(cat workflow_out.indented)

NOTE=""
if [ "$KIND" = "emulated" ]; then
  NOTE='> **Note:** this report runs the binary under qemu-user-static
> emulation. It exercises the userspace musl libc and the binary
> itself, but NOT the kernel/silicon combination an aarch64
> operator would actually deploy on. Real-hardware smoke on real
> aarch64 silicon (Pi, Graviton, ...) remains operator-side.

'
fi

cat > "$REPORT" <<EOF
# ${TAG} Release Smoke-Test Report (musl ${ARCH})

Performed: $(date -u '+%Y-%m-%d %H:%M:%S UTC')
Performed by: \`.github/workflows/smoke-musl.yml\` (${KIND} run, image \`${IMAGE}\`).

## Scope

Container-based verification that the GitHub Releases artifact for \`${TAG}\`
on target \`${TARGET}\` works as published — checksum integrity, binary
launches inside the target's musl loader, example workflow runs to
completion, evidence bundle verifies.

This is the automated counterpart to the operator-side smoke test on
real Alpine + aarch64 hardware described in
[\`docs/release-smoke-tests/v1.0.0-rc2.md\`](./v1.0.0-rc2.md). It catches
regressions a hosted CI run would catch (broken artifact, missing
loader, tarball layout drift) without standing in for real-hardware
verification.

${NOTE}## Steps performed

1. **Download** \`SHA256SUMS\` and \`${ARTIFACT}\` from
   <${RELEASE_URL}>.
2. **Verify** the SHA-256 digest matches:
   - Expected: \`${EXPECTED}\`
   - Computed: \`${COMPUTED}\`
   - **Match: ✓**
3. **Extract** the tarball.
4. **Run** \`boruna --version\` inside \`${IMAGE}\`:
   \`\`\`
   ${VERSION_OUT}
   \`\`\`
5. **Run** \`boruna capability list\`. Output:
   \`\`\`
${CAP_BLOCK}
   \`\`\`
6. **Run** the \`llm_code_review\` example workflow with
   \`--policy allow-all --record\`, then \`evidence verify\`. Output (tail):
   \`\`\`
${WORKFLOW_BLOCK}
   \`\`\`

## Verdict

The published \`${TARGET}\` artifact for \`${TAG}\` is **container-fit**:
checksum verified, binary executes under the musl loader, the example
workflow runs to completion, and the evidence bundle verifies.

Real-hardware verification on Alpine x86_64 / aarch64 hardware remains
operator-side per
[\`docs/release-smoke-tests/v1.0.0-rc2.md\`](./v1.0.0-rc2.md) §
"Targets NOT covered by this smoke test".
EOF

echo "wrote $REPORT"
