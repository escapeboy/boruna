# Bundle Storage on Azure Blob Storage

Sprint reference: post1-T-3.3.

The `--bundle-storage azblob://...` flag tells `boruna workflow
run` to copy the finalized evidence bundle to an Azure Blob
container after the local write succeeds. The local bundle
remains the authoritative record; the Azure copy is an additional
durable destination.

This adapter mirrors the [S3](bundle-storage-s3.md) and
[GCS](bundle-storage-gcs.md) adapters — same trait, same
`BORUNA_BUNDLE_CACHE` semantics, same failure contract, just with
Azure auth + the `azblob://` scheme.

## Status

- **Adapter shipped:** Azure Blob Storage (this guide). Works
  against Azure proper and Azurite for local testing.
- With T-3.3 landing, **all three remote schemes** (S3, GCS,
  Azure) ship. The `BundleStorage` trait can graduate from
  `#[doc(hidden)]` to `pub` in a future doc PR.

## Build with the `azure` feature

```sh
# CLI binary (boruna)
cargo build --release --features boruna-cli/azure

# Direct orchestrator usage from another Rust crate
[dependencies]
boruna-orchestrator = { path = "...", features = ["azure"] }

# All three remote schemes together
cargo build --release --features "boruna-cli/s3,boruna-cli/gcs,boruna-cli/azure"
```

When you build *without* the `azure` feature and pass
`--bundle-storage azblob://...`, the URI rejects at parse time
with a message that points you at the feature flag. Same UX
guarantee as S3 / GCS — never silently ignored.

## Configuring auth

`object_store::azure::MicrosoftAzureBuilder::from_env()` reads:

| Variable | Purpose |
|---|---|
| `AZURE_STORAGE_ACCOUNT_KEY` / `AZURE_STORAGE_ACCESS_KEY` | Storage account master key |
| `AZURE_STORAGE_CLIENT_ID` / `AZURE_STORAGE_CLIENT_SECRET` / `AZURE_STORAGE_TENANT_ID` | Service-principal OAuth |
| `AZURE_STORAGE_SAS_KEY` | Pre-shared SAS token |

The account name comes from the URI (`azblob://<account>/...`),
not the env, so a misconfigured `AZURE_STORAGE_ACCOUNT_NAME`
won't silently mismatch what the operator wrote.

In production, prefer Workload Identity (AKS) or
Managed-Identity-Attached-to-VM — neither requires a key on disk.

### Required RBAC roles

Bind a service principal or managed identity with the predefined
role **Storage Blob Data Contributor** scoped to the container
(or the storage account if you operate many containers per
account).

## URI shape

| Pattern | Effect |
|---|---|
| `azblob://account/container` | Objects land at `<run-id>/<file>` inside `container` |
| `azblob://account/container/prefix` | Objects land at `prefix/<run-id>/<file>` |
| `azblob://account/container/a/b/c/` | Trailing slash normalized; same as `.../a/b/c` |

Unlike S3 (single-namespace) and GCS (single-namespace), Azure
has a two-level namespace: **storage account** + **blob
container**. Both are encoded in the URI so an operator can grep
their config and see exactly which account a bundle landed in.

The `StorageRef` returned by `put` is
`azblob://account/container/prefix/<run-id>`. Treat it as opaque;
only the dispatcher parses it.

## Usage

### Per-run

```sh
export AZURE_STORAGE_ACCOUNT_KEY="$(cat /etc/boruna/azure-key)"

boruna workflow run examples/workflows/llm_code_review \
  --policy allow-all \
  --record \
  --bundle-storage azblob://myacct/audit-bundles/prod
```

### Via env var

```sh
export BORUNA_BUNDLE_STORAGE=azblob://myacct/audit-bundles/prod
```

## Reading bundles back

```rust
use boruna_orchestrator::audit::storage::{from_uri, StorageRef};

let storage = from_uri(Some("azblob://myacct/audit-bundles/prod"))?.unwrap();
let local_dir = storage.get(&StorageRef(
    "azblob://myacct/audit-bundles/prod/<run-id>".into()
))?;
boruna_orchestrator::audit::verify_bundle(&local_dir)?;
```

The cache directory (default `<temp>/boruna-bundle-cache`,
overridable via `BORUNA_BUNDLE_CACHE`) is shared with the S3 and
GCS adapters — set per-bucket cache dirs if you operate
multi-cloud and care about cross-bucket consistency.

## Failure semantics

Same as S3/GCS (see [bundle-storage-s3.md](bundle-storage-s3.md#failure-semantics)).
A storage failure never masks a successful workflow.

## Error taxonomy

`StorageError::Backend { kind, msg }` uses these stable kinds for
Azure operations:

| `kind` | Meaning | Retry? |
|---|---|---|
| `azure.transient` | Network blip, timeout, throttle | Yes — `object_store` already retries internally; bubbled up means retries exhausted. |
| `azure.permanent` | Auth failure, ContainerNotFound, AuthorizationFailure | No — operator config issue. |
| `azure.runtime` | Could not build the tokio runtime backing the adapter | No — host issue. |
| `azure.unexpected_key` | Object listed under the run prefix but doesn't match the expected path layout | No — investigate; possible container pollution. |

`StorageError::NotFound(ref)` fires when `get` is called against
a ref that has zero objects under its prefix.

## Testing against Azurite locally

Programmatic use from Rust against an Azurite container:

```rust
use boruna_orchestrator::audit::storage_azure::AzureBlobBucketBuilder;

let store = AzureBlobBucketBuilder::new(
    "azblob://devstoreaccount1/boruna-audit/local-test"
)
.with_use_emulator(true)
.with_endpoint("http://localhost:10000/devstoreaccount1")
.build()?;
```

`with_use_emulator(true)` switches the SDK into Azurite mode
(well-known account `devstoreaccount1`, well-known shared key).
`with_endpoint` lets the SDK reach a non-default port.

Container creation against Azurite needs a SharedKey-signed PUT
to `?restype=container` — out of scope for the bundled adapter,
which assumes the container exists. In a real test, create the
container with the Azure CLI (`az storage container create`) or
the Azure Storage Explorer.

## Determinism contract

`storage_ref` is operational metadata — it does not feed any
audit-log hash or replay comparison. The bundle's
`bundle_hash` / `audit_log_hash` come from the local manifest and
are independent of where the bundle is also stored.

## Limitations

- **No automatic container creation.** The container must exist
  before you point boruna at it. Failure to find it surfaces as
  `azure.permanent`. Create with `az storage container create
  --name <container> --account-name <account>`.
- **No integration test in this build.** Azurite requires
  SharedKey-signed container-create calls and `object_store`'s
  Azure adapter doesn't expose a `create_container` primitive.
  Adding it would require either pulling in the full
  `azure-storage` crate or hand-rolling the SharedKey signing
  for a single test. The 18 unit tests cover URI parsing,
  path concatenation, ref-to-run-id extraction, and error
  classification — the Boruna-side logic. Verification against
  real Azure is the operator follow-up.
- **No multipart-upload tuning knob.** `object_store` picks
  reasonable defaults.
- **Shared cache directory** across adapters; set per-bucket if
  you care.
- **No retention/lifecycle policy** is configured; use Azure
  Storage's built-in lifecycle management.
