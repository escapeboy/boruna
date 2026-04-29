# Bundle Storage

Evidence bundles are written to local disk by the orchestrator (see [Evidence Bundles](evidence-bundles.md)). The **Bundle Storage** layer is an optional second-stage that copies the finalized bundle to a durable remote backend after the local write succeeds.

The local bundle is always the authoritative record. Remote storage is additive durability — operators choose to enable it for compliance, archival, or cross-region disaster recovery.

## Backends

| Scheme | Backend | Feature flag | Guide |
|---|---|---|---|
| `local:<root>` | Local filesystem | always available | (no separate guide — see CLI help) |
| `s3://<bucket>[/<prefix>]` | AWS S3 / S3-compatible (MinIO, R2, B2) | `s3` | [bundle-storage-s3.md](../guides/bundle-storage-s3.md) |
| `gs://<bucket>[/<prefix>]` | Google Cloud Storage | `gcs` | [bundle-storage-gcs.md](../guides/bundle-storage-gcs.md) |
| `azblob://<account>/<container>[/<prefix>]` | Azure Blob Storage | `azure` | [bundle-storage-azure.md](../guides/bundle-storage-azure.md) |

All four schemes share the same trait, the same dispatcher, and the same failure contract — everything below applies to every backend.

## How it works

Pass the URI to `boruna workflow run --record`:

```sh
boruna workflow run my-workflow \
  --policy allow-all \
  --record \
  --bundle-storage s3://my-audit-bucket/prod
```

After the run finalizes locally, the orchestrator:

1. Resolves the URI to a `BundleStorage` adapter via `from_uri`.
2. Calls `storage.put(run_id, bundle_dir)`.
3. Records the returned `StorageRef` in the run output as `storage_ref`.

The `BORUNA_BUNDLE_STORAGE` env var is the standard way to set this once for an entire deployment.

## Failure contract

> **A storage failure never masks a successful workflow.**

If the remote backend is unreachable, returns auth errors, or hits any other backend failure:

- The orchestrator logs a warning to stderr.
- The local bundle remains on disk and remains the authoritative record.
- The workflow exits successfully (the remote copy is not required for the run to be considered complete).
- `storage_ref` is omitted from the output.

Operators who require remote durability should monitor the warning output and re-run `evidence verify` against the local bundle to confirm it is still on disk before pruning it.

## Reading bundles back

Once you have a `storage_ref`, materialize the bundle into a local cache directory:

```rust
use boruna_orchestrator::audit::storage::{from_uri, StorageRef};

let storage = from_uri(Some("s3://my-audit-bucket/prod"))?.unwrap();
let local_dir = storage.get(&StorageRef("s3://my-audit-bucket/prod/<run-id>".into()))?;
boruna_orchestrator::audit::verify_bundle(&local_dir)?;
```

The cache directory defaults to `<temp>/boruna-bundle-cache` and is overridable via `BORUNA_BUNDLE_CACHE`. It is shared across all remote adapters; if you operate multi-cloud and care about cross-bucket consistency, set per-bucket cache directories yourself.

## OFF-feature behavior

When you build the binary *without* the feature for a remote scheme, the corresponding URI rejects at parse time with an actionable message that points at the feature flag:

```
$ boruna workflow run --bundle-storage s3://b/p ...
warning: --bundle-storage URI invalid: s3://b/p requires the `s3` feature; rebuild with `--features boruna-cli/s3`
```

This is a deliberate design choice: silently ignoring the URI would create an audit gap (the operator believes their bundle is going to S3 but it is not). The warning is loud, the build directive is explicit, and the workflow still completes successfully against local storage.

## Determinism contract

The `storage_ref` returned by `put` is **operational metadata** — it does not feed any audit-log hash or replay comparison. The bundle's `bundle_hash` and `audit_log_hash` are computed from the local manifest before remote upload begins; where the bundle is also stored does not affect those hashes.

This means: **a bundle uploaded to S3 yesterday and to GCS today carries the same `bundle_hash`**. The two `storage_ref`s differ; the underlying audit content is identical.

## Stable error taxonomy

`BundleStorage::put / get / list` return `StorageError`. The `Backend { kind, msg }` variant carries a stable per-adapter `kind` string so integrators can branch on retry semantics:

| Adapter | `kind` taxonomy |
|---|---|
| S3 | `s3.transient`, `s3.permanent`, `s3.runtime`, `s3.unexpected_key` |
| GCS | `gcs.transient`, `gcs.permanent`, `gcs.runtime`, `gcs.unexpected_key` |
| Azure | `azure.transient`, `azure.permanent`, `azure.runtime`, `azure.unexpected_key` |

`StorageError` is `#[non_exhaustive]`, so adding a new variant in a future release is additive (existing match arms keep compiling). Likewise, new `kind` strings are additive — integrators switching on `kind` should treat unknown values as `transient` (retryable) by default.

## See also

- [Evidence Bundles](evidence-bundles.md) — what's inside a bundle and how the audit chain works
- The per-adapter operator guides linked in the table above
- API reference: `boruna_orchestrator::audit::storage` — the trait, dispatcher, and `LocalFs` adapter
- API reference: `boruna_orchestrator::audit::storage_{s3,gcs,azure}` — the per-provider implementations (each behind its own feature flag)
