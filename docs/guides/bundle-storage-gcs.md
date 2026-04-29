# Bundle Storage on Google Cloud Storage

Sprint reference: post1-T-3.2.

The `--bundle-storage gs://...` flag tells `boruna workflow run`
to copy the finalized evidence bundle to a GCS bucket after the
local write succeeds. The local bundle remains the authoritative
record; the GCS copy is an additional durable destination.

This adapter mirrors the [S3 adapter](bundle-storage-s3.md) — same
trait, same `BORUNA_BUNDLE_CACHE` semantics, same failure
contract, just with GCS auth + the `gs://` scheme.

## Status

- **Adapter shipped:** GCS (this guide). Works against Google
  Cloud Storage and `fsouza/fake-gcs-server` for local testing.
- **Already shipped:** S3 (T-3.1). See
  [bundle-storage-s3.md](bundle-storage-s3.md).
- **Reserved scheme:** `azblob://` (T-3.3, Azure Blob) is rejected
  at parse time until that adapter ships.

## Build with the `gcs` feature

```sh
# CLI binary (boruna)
cargo build --release --features boruna-cli/gcs

# Direct orchestrator usage from another Rust crate
[dependencies]
boruna-orchestrator = { path = "...", features = ["gcs"] }

# Combined with S3 if you operate multi-cloud
cargo build --release --features "boruna-cli/s3,boruna-cli/gcs"
```

When you build *without* the `gcs` feature and pass
`--bundle-storage gs://...`, the URI rejects at parse time with a
message that points you at the feature flag. Same UX guarantee as
S3 — never silently ignored.

## Configuring auth

`object_store::gcp::GoogleCloudStorageBuilder::from_env()` reads:

| Variable | Purpose |
|---|---|
| `GOOGLE_SERVICE_ACCOUNT` / `GOOGLE_SERVICE_ACCOUNT_PATH` | Path to a JSON service-account key file |
| `GOOGLE_SERVICE_ACCOUNT_KEY` | The JSON service-account key inline (handy for K8s secrets) |
| `GOOGLE_APPLICATION_CREDENTIALS` | Application Default Credentials (Workload Identity, gcloud login) |

In production, prefer Workload Identity (GKE) or
Service-Account-Attached-to-VM (GCE) — neither requires a key on
disk.

### Required IAM permissions

Bind a service account with the predefined role
`roles/storage.objectAdmin` on the bucket, or the more granular:

```
storage.buckets.get
storage.objects.create
storage.objects.delete
storage.objects.get
storage.objects.list
```

`storage.objects.delete` is reserved for a future `evidence prune`
flow; the current adapter does not delete objects.

## Usage

### Per-run

```sh
export GOOGLE_APPLICATION_CREDENTIALS=/etc/boruna/sa.json
# Or for a key in env:
# export GOOGLE_SERVICE_ACCOUNT_KEY="$(cat /etc/boruna/sa.json)"

boruna workflow run examples/workflows/llm_code_review \
  --policy allow-all \
  --record \
  --bundle-storage gs://my-audit-bucket/prod/llm-review
```

After the run finalizes locally, the CLI prints:

```
evidence bundle: ./data/evidence/<run-id>
  bundle_hash: <hex>
  audit_log_hash: <hex>
  files: 12
  storage_ref: gs://my-audit-bucket/prod/llm-review/<run-id>
```

### Via env var

```sh
export BORUNA_BUNDLE_STORAGE=gs://my-audit-bucket/prod
```

## URI shape

| Pattern | Effect |
|---|---|
| `gs://bucket` | Objects land at `<run-id>/<file>` |
| `gs://bucket/prefix` | Objects land at `prefix/<run-id>/<file>` |
| `gs://bucket/a/b/c/` | Trailing slash normalized; same as `gs://bucket/a/b/c` |

The `StorageRef` returned by `put` is `gs://bucket/prefix/<run-id>`.
Treat it as opaque; only the dispatcher parses it.

## Reading bundles back

```rust
use boruna_orchestrator::audit::storage::{from_uri, StorageRef};

let storage = from_uri(Some("gs://my-audit-bucket/prod"))?.unwrap();
let local_dir = storage.get(&StorageRef("gs://my-audit-bucket/prod/<run-id>".into()))?;
boruna_orchestrator::audit::verify_bundle(&local_dir)?;
```

The cache directory (default `<temp>/boruna-bundle-cache`,
overridable via `BORUNA_BUNDLE_CACHE`) is shared with the S3
adapter — set per-bucket cache dirs if you operate multi-cloud
and care about cross-bucket consistency.

## Failure semantics

Same as S3 (see [bundle-storage-s3.md](bundle-storage-s3.md#failure-semantics)).
A storage failure never masks a successful workflow.

## Error taxonomy

`StorageError::Backend { kind, msg }` uses these stable kinds for
GCS operations:

| `kind` | Meaning | Retry? |
|---|---|---|
| `gcs.transient` | Network blip, timeout, throttle | Yes — `object_store` already retries internally; bubbled up means retries exhausted. |
| `gcs.permanent` | Auth failure, NoSuchBucket, AccessDenied | No — operator config issue. |
| `gcs.runtime` | Could not build the tokio runtime backing the adapter | No — host issue. |
| `gcs.unexpected_key` | Object listed under the run prefix but doesn't match the expected path layout | No — investigate; possible bucket pollution. |

`StorageError::NotFound(ref)` fires when `get` is called against a
ref that has zero objects under its prefix.

## Testing against fake-gcs-server locally

```sh
# Spin up fake-gcs-server
docker run -p 4443:4443 \
  fsouza/fake-gcs-server:1.49.2 \
  -scheme http -host 0.0.0.0 -port 4443

# Create a bucket
curl -X POST 'http://localhost:4443/storage/v1/b?project=test-project' \
  -H 'Content-Type: application/json' \
  -d '{"name":"boruna-audit"}'

# Programmatic use from Rust:
use boruna_orchestrator::audit::storage_gcs::GcsBucketBuilder;
let store = GcsBucketBuilder::new("gs://boruna-audit/local-test")
    .with_endpoint("http://localhost:4443")
    .build()?;
```

(fake-gcs-server doesn't need credentials; the adapter still calls
`from_env()`, which simply doesn't pick anything up — the
endpoint override is what matters.)

The `--features gcs-it` integration tests under
`orchestrator/tests/` run the full round-trip against a
testcontainers-managed fake-gcs-server container; see
`orchestrator/tests/gcs_integration.rs` for the canonical example.

## Determinism contract

`storage_ref` is operational metadata — it does not feed any
audit-log hash or replay comparison. The bundle's
`bundle_hash` / `audit_log_hash` come from the local manifest and
are independent of where the bundle is also stored.

## Limitations

Same as S3: no automatic bucket creation, no multipart-upload
tuning knob, shared cache directory across adapters, no
retention/lifecycle policy (configure server-side via GCS
Object Lifecycle rules).
