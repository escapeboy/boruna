# data_ingestion_pipeline

A 4-step workflow demonstrating HTTP fetch, database storage, blob archival, and notifications.

## Steps

1. **fetch_data** — Builds an HTTP GET request using `std-http`; in live mode would invoke `net.fetch`.
2. **store_record** — Constructs a typed DB insert query using `std-db`.
3. **archive_blob** — Writes the record to blob storage using `std-storage`.
4. **notify_complete** — Pushes a completion notification using `std-notifications`.

## Stdlib packages referenced

| Package | Functions used |
|---------|----------------|
| `std-http` | `http_get`, `http_request`, `http_default_retry` |
| `std-db` | `db_insert`, `db_select`, `db_where`, `db_limit`, `db_to_effect` |
| `std-storage` | `storage_key`, `storage_make_entry`, `storage_set`, `storage_bump_version` |
| `std-notifications` | `notification_init`, `notification_push`, `notification_make` |

## Import note

Step files currently inline the stdlib surface directly with a comment header:
`// Inline from std.X — import pending full package resolver integration`

Full `import std.http` syntax is parsed by the compiler but package path resolution
in workflow step context is a planned post-1.0 feature. The structural usage pattern
is identical to what import-based resolution will produce.

## Validate

```bash
cargo run --bin boruna -- workflow validate examples/workflows/data_ingestion_pipeline
```
