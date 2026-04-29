# KEK Rotation

Sprint reference: post1-T-2.4.

`boruna evidence rotate-kek` re-wraps the data-encryption-key (DEK)
of one or more encrypted evidence bundles under a new
key-encryption-key (KEK). The DEK itself is **not** changed — only
the manifest's `wrapped_dek`, `wrapped_dek_nonce`, and `kek_id`
fields. Every per-file AES-GCM authentication tag in the bundle
remains valid because the keying material that produced those tags
hasn't moved.

## When to rotate

- A KEK is suspected leaked or has been retired in your KMS.
- A scheduled rotation policy (e.g. "rotate every 6 months").
- Migrating bundles from a legacy `kek_id` to a new one.

Compliance auditors typically only need to see *that* rotation
happened — the bundle's `bundle_hash` updates because the manifest
contents change, but the audit log inside the bundle is unchanged.

## Single-bundle rotation

```sh
boruna evidence rotate-kek \
  /path/to/<run-id> \
  --old-kek $(printenv OLD_KEK_HEX) \
  --new-kek $(printenv NEW_KEK_HEX) \
  --kek-id-to "rotation-2026-q3"
```

The bundle's `manifest.json` is rewritten atomically (sibling tmp +
rename). The rest of the bundle directory is untouched.

## Batch rotation

Point `--target` at a parent directory whose immediate
subdirectories are bundles:

```sh
boruna evidence rotate-kek \
  /var/lib/boruna/evidence \
  --old-kek $OLD --new-kek $NEW \
  --kek-id-to "rotation-2026-q3" \
  --parallelism 4
```

Each bundle is processed in its own rayon task, bounded by
`--parallelism` (default `min(8, num_cpus)`). Per-bundle failures
do not abort the batch — already-rotated bundles stay rotated, and
the failed bundle is reported on stderr. The CLI exits non-zero if
any bundle failed.

## Dry-run

Always dry-run first on a representative bundle:

```sh
boruna evidence rotate-kek bundles/abc... \
  --old-kek $OLD --new-kek $NEW --kek-id-to new-id \
  --dry-run
```

Dry-run validates that the old KEK can unwrap, that the new KEK
re-wraps cleanly, and prints the planned `kek_id` change. **No
files are modified.**

## kek_id_from filter

Use `--kek-id-from <id>` to defend against accidental
double-rotation in a mixed-state batch (some bundles already
rotated, others not):

```sh
boruna evidence rotate-kek bundles/ \
  --old-kek $OLD --new-kek $NEW \
  --kek-id-from "rotation-2026-q2" \
  --kek-id-to "rotation-2026-q3"
```

Bundles whose current `kek_id` does not match `--kek-id-from` are
reported as failures (`bundle kek_id is 'X'; --kek-id-from is 'Y'`)
without being modified.

## Verifying after rotation

```sh
boruna evidence verify /path/to/<run-id> \
  --bundle-encryption-key $NEW_KEK_HEX
```

Verifying with the **old** KEK after rotation MUST fail with
`evidence.encryption_key_mismatch` — that's the regression the
rotation tool's tests assert on.

## Spec reference

The 1.0 evidence-bundle reader contract already accommodates
re-wrapping: per-file ciphertext stays valid because the DEK is
unchanged. See `docs/spec/evidence-bundle-1.0.md`. No spec version
bump is required for this feature.
