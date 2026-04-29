# Bundle Storage on S3

Sprint reference: post1-T-3.1.

The `--bundle-storage s3://...` flag tells `boruna workflow run` to
copy the finalized evidence bundle to an S3 bucket after the local
write succeeds. The local bundle remains the authoritative record;
the S3 copy is an additional durable destination that survives the
local data directory being wiped.

## Status

- **Adapter shipped:** S3 (this guide). Works against AWS S3 and
  S3-compatible endpoints (MinIO, Cloudflare R2, Backblaze B2,
  LocalStack).
- **Reserved schemes:** `gs://` (T-3.2, GCS) and `azblob://` (T-3.3,
  Azure Blob) are rejected at parse time until those adapters ship.

## Build with the `s3` feature

The S3 adapter pulls in `object_store` + `tokio` + `reqwest`. To keep
the default binary lean, the adapter is OFF by default. Enable it at
build time:

```sh
# CLI binary (boruna)
cargo build --release --features boruna-cli/s3

# Direct orchestrator usage from another Rust crate
[dependencies]
boruna-orchestrator = { path = "...", features = ["s3"] }
```

When you build *without* the `s3` feature and pass
`--bundle-storage s3://...`, the URI rejects at parse time with a
message that points you at the feature flag. Crucially: it never
silently ignores the flag and ships an audit gap.

## Configuring auth

`object_store::aws::AmazonS3Builder::from_env()` reads the standard
AWS environment variables:

| Variable | Purpose |
|---|---|
| `AWS_ACCESS_KEY_ID` | IAM access key |
| `AWS_SECRET_ACCESS_KEY` | IAM secret |
| `AWS_SESSION_TOKEN` | STS session token (optional) |
| `AWS_REGION` | Defaults to `us-east-1` if unset |
| `AWS_ENDPOINT_URL` | Override for MinIO / R2 / LocalStack |
| `AWS_ALLOW_HTTP` | Set to `true` for non-HTTPS endpoints (testing only) |

For production AWS, prefer instance/role credentials (the SDK picks
those up automatically when none of the env vars above are set).

### Required IAM permissions

```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": [
        "s3:PutObject",
        "s3:GetObject",
        "s3:ListBucket",
        "s3:DeleteObject"
      ],
      "Resource": [
        "arn:aws:s3:::your-bucket",
        "arn:aws:s3:::your-bucket/*"
      ]
    }
  ]
}
```

`s3:DeleteObject` is reserved for a future `evidence prune` flow;
the current adapter does not delete objects.

## Usage

### Per-run

```sh
export AWS_ACCESS_KEY_ID=...
export AWS_SECRET_ACCESS_KEY=...
export AWS_REGION=eu-west-1

boruna workflow run examples/workflows/llm_code_review \
  --policy allow-all \
  --record \
  --bundle-storage s3://my-audit-bucket/prod/llm-review
```

After the run finalizes locally, the CLI prints:

```
evidence bundle: ./data/evidence/<run-id>
  bundle_hash: <hex>
  audit_log_hash: <hex>
  files: 12
  storage_ref: s3://my-audit-bucket/prod/llm-review/<run-id>
```

### Via env var

The flag falls back to `BORUNA_BUNDLE_STORAGE`, so operators
typically set this once in their service environment:

```sh
export BORUNA_BUNDLE_STORAGE=s3://my-audit-bucket/prod
```

## URI shape

| Pattern | Effect |
|---|---|
| `s3://bucket` | Objects land at `<run-id>/<file>` |
| `s3://bucket/prefix` | Objects land at `prefix/<run-id>/<file>` |
| `s3://bucket/a/b/c/` | Trailing slash normalized; same as `s3://bucket/a/b/c` |

The `StorageRef` returned by `put` is `s3://bucket/prefix/<run-id>`.
Treat it as opaque; only the dispatcher parses it.

## Reading bundles back

Once you have the `storage_ref`, use the orchestrator API to
materialize the bundle into a local cache directory:

```rust
use boruna_orchestrator::audit::storage::{from_uri, StorageRef};

let storage = from_uri(Some("s3://my-audit-bucket/prod"))?.unwrap();
let local_dir = storage.get(&StorageRef("s3://my-audit-bucket/prod/<run-id>".into()))?;
// local_dir is now under BORUNA_BUNDLE_CACHE (defaults to <temp>/boruna-bundle-cache)
// and contains the same files the original run finalized locally.

boruna_orchestrator::audit::verify_bundle(&local_dir)?;
```

The cache directory is overwritten on each `get`. There is no
automatic GC; sweep it periodically with `find $BORUNA_BUNDLE_CACHE
-mtime +N -delete` or similar.

## Failure semantics

| Condition | Behavior |
|---|---|
| `--bundle-storage` URI invalid | Run still completes; warning printed; `storage_ref` not recorded. |
| S3 put fails (network / auth / quota) | Run still completes; warning printed; `storage_ref` not recorded. **The local bundle is the authoritative record.** |
| `s3://` passed without `s3` feature | Run rejects at flag parse time with an actionable message. |

A storage failure never masks a successful workflow. Conversely,
operators who require S3 durability should monitor the warning
output and re-run `evidence verify` against the local bundle to
confirm it's still on disk.

## Error taxonomy

`StorageError::Backend { kind, msg }` uses these stable kinds for S3
operations:

| `kind` | Meaning | Retry? |
|---|---|---|
| `s3.transient` | Network blip, timeout, throttle | Yes — `object_store` already retries internally; bubbled up means retries exhausted. |
| `s3.permanent` | Auth failure, NoSuchBucket, AccessDenied | No — operator config issue. |
| `s3.runtime` | Could not build the tokio runtime backing the adapter | No — host issue. |
| `s3.unexpected_key` | Object listed under the run prefix but doesn't match the expected path layout | No — investigate; possible bucket pollution. |

`StorageError::NotFound(ref)` fires when `get` is called against a
ref that has zero objects under its prefix.

## Testing against MinIO locally

```sh
# Spin up MinIO
docker run -p 9000:9000 -p 9001:9001 \
  -e MINIO_ROOT_USER=minioadmin \
  -e MINIO_ROOT_PASSWORD=minioadmin \
  quay.io/minio/minio server /data --console-address ":9001"

# Create a bucket via the console (http://localhost:9001) or mc
mc alias set local http://localhost:9000 minioadmin minioadmin
mc mb local/boruna-audit

# Run boruna against it
export AWS_ENDPOINT_URL=http://localhost:9000
export AWS_ACCESS_KEY_ID=minioadmin
export AWS_SECRET_ACCESS_KEY=minioadmin
export AWS_REGION=us-east-1
export AWS_ALLOW_HTTP=true

boruna workflow run examples/workflows/llm_code_review \
  --policy allow-all \
  --record \
  --bundle-storage s3://boruna-audit/local-test
```

The `--features s3-it` integration test under `orchestrator/tests/`
runs the full round-trip against a testcontainers-managed MinIO
container; see `orchestrator/tests/s3_integration.rs` for the
canonical example.

## Determinism contract

`storage_ref` is operational metadata — it does not feed any
audit-log hash or replay comparison. The bundle's
`bundle_hash` / `audit_log_hash` come from the local manifest and
are independent of where the bundle is also stored.

## Limitations

- No automatic bucket creation. The bucket must exist before you
  point boruna at it. Failure to find the bucket surfaces as
  `s3.permanent`.
- No multipart-upload tuning knob. `object_store` picks reasonable
  defaults (5 MB part size). Files smaller than that go via single
  PUT; larger ones use multipart automatically.
- The cache directory is shared across adapters — if you rotate
  between two buckets that contain different bundles for the same
  `run_id`, the cache content is whichever was fetched most
  recently. Operators who care about cross-bucket consistency
  should set `BORUNA_BUNDLE_CACHE` to a per-bucket directory.
- No retention / lifecycle policy is configured on the bucket;
  operators define this server-side via S3 lifecycle rules.
