use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Sentinel value substituted for a redacted string leaf. Verification
/// never inspects this text — the removed content is proven by the
/// entry's `content_sha256` commitment, not by the remnant.
pub const REDACTION_SENTINEL: &str = "[REDACTED]";

/// A single entry in the hash-chained audit log.
///
/// ## Commitment chain (format 1.1)
///
/// Each entry commits to a **content hash** of its event rather than
/// hashing the raw event JSON into the chain directly. Concretely:
///
/// ```text
/// content_sha256 = SHA-256(event_json)
/// entry_hash     = SHA-256(sequence_le || prev_hash_ascii || content_sha256_ascii)
/// ```
///
/// Because the chain links via `content_sha256` (not the event bytes),
/// the event content can later be REDACTED — replaced in place while
/// leaving `content_sha256` untouched — and the chain still recomputes
/// to the identical `entry_hash`. This is what makes a sealed audit log
/// redactable without breaking verification (verifiable redaction).
///
/// ## Back-compat (format 1.0)
///
/// Legacy entries have no `content_sha256` (it deserializes to the empty
/// string via `#[serde(default)]`). Those entries verify under the
/// original formula `SHA-256(sequence_le || prev_hash || event_json)`.
/// Presence of a non-empty `content_sha256` selects the commitment form
/// per entry, so a whole log is either legacy or commitment-form and both
/// verify.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub sequence: u64,
    pub prev_hash: String,
    /// Commitment to the event content: `SHA-256(event_json)`. Empty on
    /// legacy (format 1.0) entries, which chain over the raw event JSON.
    /// Skipped when empty so re-serialized legacy logs stay byte-identical.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content_sha256: String,
    pub event: AuditEvent,
    /// Present iff this entry's event content has been redacted. The
    /// event field then holds a placeholder (PII-bearing strings blanked
    /// to [`REDACTION_SENTINEL`]) and is NON-AUTHORITATIVE — the removed
    /// content is proven only by `content_sha256`. Absent on normal
    /// entries (skipped in JSON, so unredacted logs are byte-unchanged).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redacted: Option<Redaction>,
    pub entry_hash: String,
}

/// Records that an entry was redacted: an AUTHORIZED, recorded removal of
/// event content. It carries the same `content_sha256` commitment that
/// the entry already binds into its `entry_hash`, so a verifier can
/// confirm the redaction is consistent (marker matches the committed
/// hash) without ever seeing the original content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Redaction {
    /// The removed content's `SHA-256` — equal to the entry's
    /// `content_sha256`. Binding the two lets a verifier detect a
    /// redact-then-tamper (rewriting one but not the other).
    pub content_sha256: String,
    /// Optional operator-supplied reason. Advisory metadata; not part of
    /// the commitment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Error from [`AuditLog::redact_entry`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedactError {
    /// Entry index is past the end of the log.
    OutOfRange(usize),
    /// Entry is a legacy (format 1.0) entry with no content commitment;
    /// redaction requires the commitment chain (format 1.1). Migrate the
    /// bundle first.
    LegacyUnsupported,
    /// Entry has already been redacted.
    AlreadyRedacted(usize),
    /// A `--field` target was not present in the event.
    FieldNotFound(String),
    /// Re-serialization of the placeholder event failed.
    Serde(String),
}

impl std::fmt::Display for RedactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RedactError::OutOfRange(i) => write!(f, "entry index {i} is out of range"),
            RedactError::LegacyUnsupported => write!(
                f,
                "entry uses the legacy (1.0) chain with no content commitment; \
                 redaction requires format 1.1 (migrate the bundle first)"
            ),
            RedactError::AlreadyRedacted(i) => write!(f, "entry {i} is already redacted"),
            RedactError::FieldNotFound(name) => {
                write!(f, "field `{name}` not found in the event")
            }
            RedactError::Serde(e) => write!(f, "placeholder serialization failed: {e}"),
        }
    }
}

impl std::error::Error for RedactError {}

/// Events that can be recorded in the audit log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditEvent {
    WorkflowStarted {
        workflow_hash: String,
        policy_hash: String,
    },
    StepStarted {
        step_id: String,
        input_hash: String,
    },
    StepCompleted {
        step_id: String,
        output_hash: String,
        duration_ms: u64,
    },
    StepFailed {
        step_id: String,
        error: String,
    },
    CapabilityInvoked {
        step_id: String,
        capability: String,
        allowed: bool,
    },
    PolicyEvaluated {
        step_id: String,
        rule: String,
        decision: String,
    },
    BudgetConsumed {
        step_id: String,
        tokens: u64,
        remaining: u64,
    },
    ApprovalRequested {
        step_id: String,
        role: String,
    },
    ApprovalGranted {
        step_id: String,
        approver: String,
    },
    ApprovalDenied {
        step_id: String,
        reason: String,
    },
    /// External-trigger gate advanced via `boruna workflow trigger`
    /// (sprint `0.4-S9`). `payload_hash` is the SHA-256 of the
    /// trigger payload — captured rather than the payload itself
    /// because webhook bodies can be large and may contain operator
    /// PII. The hash matches the step's `output_hash` (since the
    /// payload becomes the step's output value), but the dedicated
    /// event distinguishes "advanced via external event" from
    /// "advanced via source step completion" in the audit trail.
    ExternalTriggerReceived {
        step_id: String,
        payload_hash: String,
    },
    WorkflowCompleted {
        result_hash: String,
        total_duration_ms: u64,
    },
}

/// An append-only, hash-chained audit log.
///
/// Each entry's hash includes the previous entry's hash, forming a tamper-evident chain.
#[derive(Debug, Clone)]
pub struct AuditLog {
    entries: Vec<AuditEntry>,
}

impl AuditLog {
    pub fn new() -> Self {
        AuditLog {
            entries: Vec::new(),
        }
    }

    /// Append a new event. Returns the entry's hash.
    ///
    /// Uses the commitment chain (format 1.1): the entry commits to
    /// `content_sha256 = SHA-256(event_json)`, and the chain links via
    /// that commitment so the event can later be redacted in place.
    pub fn append(&mut self, event: AuditEvent) -> String {
        let sequence = self.entries.len() as u64;
        let prev_hash = self
            .entries
            .last()
            .map(|e| e.entry_hash.clone())
            .unwrap_or_else(|| "0".repeat(64));

        let content_sha256 = Self::content_hash(&event);
        let entry_hash = Self::compute_entry_hash(sequence, &prev_hash, &content_sha256);

        self.entries.push(AuditEntry {
            sequence,
            prev_hash,
            content_sha256,
            event,
            redacted: None,
            entry_hash: entry_hash.clone(),
        });

        entry_hash
    }

    /// Verify the integrity of the entire chain. Returns Ok(()) or the index of the first bad entry.
    ///
    /// Each entry is checked under its own format:
    /// - **Commitment (1.1)** — non-empty `content_sha256`: the entry
    ///   hash must equal `SHA-256(seq || prev || content_sha256)`, and
    ///   the content must bind — either the live event hashes to
    ///   `content_sha256` (unredacted), or the entry is redacted and its
    ///   marker carries the same `content_sha256` (authorized removal).
    /// - **Legacy (1.0)** — empty `content_sha256`: the original formula
    ///   `SHA-256(seq || prev || event_json)`.
    pub fn verify(&self) -> Result<(), u64> {
        let mut expected_prev = "0".repeat(64);

        for entry in &self.entries {
            if entry.prev_hash != expected_prev {
                return Err(entry.sequence);
            }
            if entry.content_sha256.is_empty() {
                // Legacy (format 1.0) chain over the raw event JSON.
                let computed =
                    Self::compute_hash_legacy(entry.sequence, &entry.prev_hash, &entry.event);
                if computed != entry.entry_hash {
                    return Err(entry.sequence);
                }
            } else {
                // Commitment (format 1.1) chain over content_sha256.
                let computed = Self::compute_entry_hash(
                    entry.sequence,
                    &entry.prev_hash,
                    &entry.content_sha256,
                );
                if computed != entry.entry_hash {
                    return Err(entry.sequence);
                }
                match &entry.redacted {
                    // Authorized removal: the marker must commit to the
                    // same content hash the chain binds. A redact-then-
                    // tamper that rewrites either side is caught here.
                    Some(r) => {
                        if r.content_sha256 != entry.content_sha256 {
                            return Err(entry.sequence);
                        }
                    }
                    // Unredacted: the live event must match its commitment.
                    None => {
                        if Self::content_hash(&entry.event) != entry.content_sha256 {
                            return Err(entry.sequence);
                        }
                    }
                }
            }
            expected_prev = entry.entry_hash.clone();
        }

        Ok(())
    }

    /// Redact an entry's event content in place (verifiable redaction).
    ///
    /// Blanks PII-bearing string leaves in the event to
    /// [`REDACTION_SENTINEL`] while preserving the entry's
    /// `content_sha256` commitment (and therefore its `entry_hash`, its
    /// `prev_hash` links, and the log's overall [`Self::hash`]). The
    /// entry is stamped with a [`Redaction`] marker; verification then
    /// treats the event as non-authoritative and validates only the
    /// commitment. `verify()` still PASSES; a tamper that alters the
    /// commitment still FAILS.
    ///
    /// - `field: None` blanks every string leaf in the event.
    /// - `field: Some(name)` blanks only that named field of the event.
    ///
    /// Returns the preserved `content_sha256` on success. Errors if the
    /// index is out of range, the entry is legacy-format, already
    /// redacted, or the named field is absent.
    pub fn redact_entry(
        &mut self,
        index: usize,
        field: Option<&str>,
        reason: Option<String>,
    ) -> Result<String, RedactError> {
        let entry = self
            .entries
            .get_mut(index)
            .ok_or(RedactError::OutOfRange(index))?;
        if entry.content_sha256.is_empty() {
            return Err(RedactError::LegacyUnsupported);
        }
        if entry.redacted.is_some() {
            return Err(RedactError::AlreadyRedacted(index));
        }

        let mut value =
            serde_json::to_value(&entry.event).map_err(|e| RedactError::Serde(e.to_string()))?;
        match field {
            None => blank_all_strings(&mut value),
            Some(name) => {
                // The event serializes as a single-key object
                // `{"Variant": {fields...}}`; descend into the inner
                // field object to reach the target field.
                let target = value
                    .as_object_mut()
                    .and_then(|o| o.values_mut().next())
                    .and_then(|inner| inner.as_object_mut())
                    .and_then(|fields| fields.get_mut(name))
                    .ok_or_else(|| RedactError::FieldNotFound(name.to_string()))?;
                blank_all_strings(target);
            }
        }
        let placeholder: AuditEvent =
            serde_json::from_value(value).map_err(|e| RedactError::Serde(e.to_string()))?;

        entry.event = placeholder;
        entry.redacted = Some(Redaction {
            content_sha256: entry.content_sha256.clone(),
            reason,
        });
        Ok(entry.content_sha256.clone())
    }

    /// Sequences of entries that have been redacted, in order. Used by
    /// `verify_bundle` to report which entries carry redactions.
    pub fn redacted_sequences(&self) -> Vec<u64> {
        self.entries
            .iter()
            .filter(|e| e.redacted.is_some())
            .map(|e| e.sequence)
            .collect()
    }

    /// Get all entries.
    pub fn entries(&self) -> &[AuditEntry] {
        &self.entries
    }

    /// Compute the SHA-256 hash of the log (hash of the last entry, or zeros if empty).
    pub fn hash(&self) -> String {
        self.entries
            .last()
            .map(|e| e.entry_hash.clone())
            .unwrap_or_else(|| "0".repeat(64))
    }

    /// Serialize the log to JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&self.entries)
    }

    /// Deserialize a log from JSON entries.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let entries: Vec<AuditEntry> = serde_json::from_str(json)?;
        Ok(AuditLog { entries })
    }

    /// Construct an `AuditLog` from an owned `Vec<AuditEntry>` (sprint
    /// `0.4-S9`). Used by callers that round-trip the log through a
    /// surrounding container (e.g., a run's persisted metadata blob)
    /// and need to restore the in-memory log without re-serializing
    /// to JSON. The chain is **not** re-verified here — call
    /// [`Self::verify`] explicitly if the source is untrusted, or
    /// use [`Self::from_entries_verified`] to fuse load + verify.
    pub fn from_entries(entries: Vec<AuditEntry>) -> Self {
        AuditLog { entries }
    }

    /// Construct + verify in one step. Returns the constructed
    /// `AuditLog` on success, or `Err(bad_seq)` on the first entry
    /// whose hash chain breaks — same error semantics as
    /// [`Self::verify`]. Use this when loading the log from a
    /// less-trusted surface (e.g., `metadata.audit_log` rehydrated
    /// from sqlite) so chain-integrity violations surface at load
    /// time instead of propagating into derived artifacts (evidence
    /// bundles, append operations) that would otherwise treat the
    /// tampered chain as valid.
    pub fn from_entries_verified(entries: Vec<AuditEntry>) -> Result<Self, u64> {
        let log = AuditLog { entries };
        log.verify()?;
        Ok(log)
    }

    /// Consume the log and return its owned entries (sprint `0.4-S9`).
    /// Companion to [`Self::from_entries`] for round-tripping through
    /// a containing struct.
    pub fn into_entries(self) -> Vec<AuditEntry> {
        self.entries
    }

    /// Content commitment for an event: `SHA-256(event_json)`. This is
    /// what the entry (and thus the chain) binds, so the event bytes can
    /// be redacted later without disturbing the chain.
    fn content_hash(event: &AuditEvent) -> String {
        let event_json = serde_json::to_string(event).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(event_json.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Commitment-chain (format 1.1) entry hash:
    /// `SHA-256(sequence_le || prev_hash || content_sha256)`.
    fn compute_entry_hash(sequence: u64, prev_hash: &str, content_sha256: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(sequence.to_le_bytes());
        hasher.update(prev_hash.as_bytes());
        hasher.update(content_sha256.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Legacy (format 1.0) entry hash used to verify pre-1.1 logs:
    /// `SHA-256(sequence_le || prev_hash || event_json)`. Retained for
    /// back-compat verification only; `append` never produces this form.
    fn compute_hash_legacy(sequence: u64, prev_hash: &str, event: &AuditEvent) -> String {
        let event_json = serde_json::to_string(event).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(sequence.to_le_bytes());
        hasher.update(prev_hash.as_bytes());
        hasher.update(event_json.as_bytes());
        format!("{:x}", hasher.finalize())
    }
}

/// Recursively replace every string leaf in `v` with
/// [`REDACTION_SENTINEL`]. Object KEYS (which carry the serde enum tag
/// and field names) are left intact so the value still deserializes back
/// into the same `AuditEvent` variant.
fn blank_all_strings(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::String(s) => *s = REDACTION_SENTINEL.to_string(),
        serde_json::Value::Array(a) => a.iter_mut().for_each(blank_all_strings),
        serde_json::Value::Object(o) => o.values_mut().for_each(blank_all_strings),
        _ => {}
    }
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_append_and_verify() {
        let mut log = AuditLog::new();
        log.append(AuditEvent::WorkflowStarted {
            workflow_hash: "abc".into(),
            policy_hash: "def".into(),
        });
        log.append(AuditEvent::StepStarted {
            step_id: "s1".into(),
            input_hash: "inp".into(),
        });
        log.append(AuditEvent::StepCompleted {
            step_id: "s1".into(),
            output_hash: "out".into(),
            duration_ms: 100,
        });
        log.append(AuditEvent::WorkflowCompleted {
            result_hash: "res".into(),
            total_duration_ms: 200,
        });

        assert_eq!(log.entries().len(), 4);
        assert!(log.verify().is_ok());
    }

    #[test]
    fn test_chain_integrity() {
        let mut log = AuditLog::new();
        for i in 0..100 {
            log.append(AuditEvent::StepStarted {
                step_id: format!("step_{i}"),
                input_hash: format!("hash_{i}"),
            });
        }
        assert_eq!(log.entries().len(), 100);
        assert!(log.verify().is_ok());
    }

    #[test]
    fn test_tamper_detection() {
        let mut log = AuditLog::new();
        log.append(AuditEvent::WorkflowStarted {
            workflow_hash: "a".into(),
            policy_hash: "b".into(),
        });
        log.append(AuditEvent::StepStarted {
            step_id: "s1".into(),
            input_hash: "x".into(),
        });
        log.append(AuditEvent::StepCompleted {
            step_id: "s1".into(),
            output_hash: "y".into(),
            duration_ms: 50,
        });

        assert!(log.verify().is_ok());

        // Tamper with the middle entry
        log.entries[1].event = AuditEvent::StepStarted {
            step_id: "TAMPERED".into(),
            input_hash: "x".into(),
        };

        assert!(log.verify().is_err());
        assert_eq!(log.verify().unwrap_err(), 1);
    }

    #[test]
    fn test_empty_log_verifies() {
        let log = AuditLog::new();
        assert!(log.verify().is_ok());
    }

    #[test]
    fn test_hash_deterministic() {
        let mut log1 = AuditLog::new();
        let mut log2 = AuditLog::new();

        log1.append(AuditEvent::WorkflowStarted {
            workflow_hash: "a".into(),
            policy_hash: "b".into(),
        });
        log2.append(AuditEvent::WorkflowStarted {
            workflow_hash: "a".into(),
            policy_hash: "b".into(),
        });

        assert_eq!(log1.hash(), log2.hash());
    }

    #[test]
    fn test_serialize_deserialize() {
        let mut log = AuditLog::new();
        log.append(AuditEvent::WorkflowStarted {
            workflow_hash: "abc".into(),
            policy_hash: "def".into(),
        });
        log.append(AuditEvent::WorkflowCompleted {
            result_hash: "res".into(),
            total_duration_ms: 100,
        });

        let json = log.to_json().unwrap();
        let restored = AuditLog::from_json(&json).unwrap();
        assert_eq!(restored.entries().len(), 2);
        assert!(restored.verify().is_ok());
        assert_eq!(log.hash(), restored.hash());
    }

    #[test]
    fn test_prev_hash_chain() {
        let mut log = AuditLog::new();
        let h1 = log.append(AuditEvent::StepStarted {
            step_id: "a".into(),
            input_hash: "x".into(),
        });
        log.append(AuditEvent::StepStarted {
            step_id: "b".into(),
            input_hash: "y".into(),
        });

        // Second entry's prev_hash should be the first entry's hash
        assert_eq!(log.entries()[0].entry_hash, h1);
        assert_eq!(log.entries()[1].prev_hash, h1);
    }

    // ---- verifiable redaction: commitment chain (format 1.1) ----

    fn sample_log() -> AuditLog {
        let mut log = AuditLog::new();
        log.append(AuditEvent::WorkflowStarted {
            workflow_hash: "wf".into(),
            policy_hash: "po".into(),
        });
        log.append(AuditEvent::ApprovalGranted {
            step_id: "s1".into(),
            approver: "alice@example.com".into(),
        });
        log.append(AuditEvent::WorkflowCompleted {
            result_hash: "res".into(),
            total_duration_ms: 10,
        });
        log
    }

    #[test]
    fn commitment_chain_populates_content_hash_and_verifies() {
        let log = sample_log();
        assert!(log.verify().is_ok());
        // Every entry carries a 64-hex content commitment, and the entry
        // hash is the commitment-form hash (not the legacy form).
        for e in log.entries() {
            assert_eq!(e.content_sha256.len(), 64, "content_sha256 must be set");
            assert_eq!(
                e.entry_hash,
                AuditLog::compute_entry_hash(e.sequence, &e.prev_hash, &e.content_sha256)
            );
            assert_eq!(e.content_sha256, AuditLog::content_hash(&e.event));
        }
    }

    #[test]
    fn legacy_format_1_0_log_still_verifies() {
        // Hand-build a legacy entry: empty content_sha256, entry_hash via
        // the ORIGINAL formula SHA-256(seq || prev || event_json).
        let event = AuditEvent::StepStarted {
            step_id: "s1".into(),
            input_hash: "inp".into(),
        };
        let prev = "0".repeat(64);
        let entry_hash = AuditLog::compute_hash_legacy(0, &prev, &event);
        let legacy = AuditEntry {
            sequence: 0,
            prev_hash: prev,
            content_sha256: String::new(), // legacy marker
            event,
            redacted: None,
            entry_hash,
        };
        let log = AuditLog::from_entries(vec![legacy]);
        assert!(log.verify().is_ok(), "legacy 1.0 entry must still verify");

        // A round-trip through JSON (no content_sha256 field present)
        // rehydrates as legacy and still verifies.
        let json = log.to_json().unwrap();
        assert!(!json.contains("content_sha256"));
        let restored = AuditLog::from_json(&json).unwrap();
        assert!(restored.verify().is_ok());
    }

    #[test]
    fn redact_whole_event_preserves_chain_and_removes_pii() {
        let mut log = sample_log();
        let anchor_hash = log.hash();
        let orig_commit = log.entries()[1].content_sha256.clone();

        let returned = log
            .redact_entry(1, None, Some("GDPR erasure request".into()))
            .unwrap();

        // Chain still verifies; the log's overall hash is UNCHANGED.
        assert!(log.verify().is_ok(), "redacted log must still verify");
        assert_eq!(
            log.hash(),
            anchor_hash,
            "audit_log hash invariant under redaction"
        );
        assert_eq!(returned, orig_commit);

        let e = &log.entries()[1];
        // The commitment is preserved; the marker records the redaction.
        assert_eq!(e.content_sha256, orig_commit);
        let r = e.redacted.as_ref().expect("marker present");
        assert_eq!(r.content_sha256, orig_commit);
        assert_eq!(r.reason.as_deref(), Some("GDPR erasure request"));

        // The PII (approver email) is gone; strings blanked to sentinel.
        match &e.event {
            AuditEvent::ApprovalGranted { step_id, approver } => {
                assert_eq!(step_id, REDACTION_SENTINEL);
                assert_eq!(approver, REDACTION_SENTINEL);
                assert!(!approver.contains("alice"));
            }
            other => panic!("variant must be preserved, got {other:?}"),
        }
        assert_eq!(log.redacted_sequences(), vec![1]);
    }

    #[test]
    fn redact_single_field_blanks_only_that_field() {
        let mut log = sample_log();
        log.redact_entry(1, Some("approver"), None).unwrap();
        assert!(log.verify().is_ok());
        match &log.entries()[1].event {
            AuditEvent::ApprovalGranted { step_id, approver } => {
                assert_eq!(step_id, "s1"); // untouched
                assert_eq!(approver, REDACTION_SENTINEL); // blanked
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn redact_unknown_field_errors() {
        let mut log = sample_log();
        assert_eq!(
            log.redact_entry(1, Some("no_such_field"), None),
            Err(RedactError::FieldNotFound("no_such_field".into()))
        );
    }

    #[test]
    fn redact_already_redacted_errors() {
        let mut log = sample_log();
        log.redact_entry(1, None, None).unwrap();
        assert_eq!(
            log.redact_entry(1, None, None),
            Err(RedactError::AlreadyRedacted(1))
        );
    }

    #[test]
    fn redact_out_of_range_errors() {
        let mut log = sample_log();
        assert_eq!(
            log.redact_entry(99, None, None),
            Err(RedactError::OutOfRange(99))
        );
    }

    #[test]
    fn redact_legacy_entry_unsupported() {
        let event = AuditEvent::StepFailed {
            step_id: "s1".into(),
            error: "boom".into(),
        };
        let prev = "0".repeat(64);
        let entry_hash = AuditLog::compute_hash_legacy(0, &prev, &event);
        let mut log = AuditLog::from_entries(vec![AuditEntry {
            sequence: 0,
            prev_hash: prev,
            content_sha256: String::new(),
            event,
            redacted: None,
            entry_hash,
        }]);
        assert_eq!(
            log.redact_entry(0, None, None),
            Err(RedactError::LegacyUnsupported)
        );
    }

    #[test]
    fn tamper_of_unredacted_event_is_detected() {
        // Motivated attacker edits the event but keeps content_sha256 and
        // entry_hash: the content-binding check catches the mismatch.
        let mut log = sample_log();
        log.entries[1].event = AuditEvent::ApprovalGranted {
            step_id: "s1".into(),
            approver: "attacker".into(),
        };
        assert_eq!(log.verify().unwrap_err(), 1);
    }

    #[test]
    fn redact_then_tamper_commitment_fails() {
        // Redact, then rewrite the marker's content_sha256 to forge a
        // different original. The marker/commitment binding breaks.
        let mut log = sample_log();
        log.redact_entry(1, None, None).unwrap();
        assert!(log.verify().is_ok());

        log.entries[1].redacted = Some(Redaction {
            content_sha256: "f".repeat(64),
            reason: None,
        });
        assert_eq!(log.verify().unwrap_err(), 1);

        // Alternatively, rewriting the entry's committed hash breaks the
        // entry_hash recompute (chain integrity).
        let mut log2 = sample_log();
        log2.redact_entry(1, None, None).unwrap();
        log2.entries[1].content_sha256 = "e".repeat(64);
        assert!(log2.verify().is_err());
    }

    #[test]
    fn redaction_survives_json_round_trip() {
        let mut log = sample_log();
        log.redact_entry(1, None, Some("privacy".into())).unwrap();
        let json = log.to_json().unwrap();
        let restored = AuditLog::from_json(&json).unwrap();
        assert!(restored.verify().is_ok());
        assert_eq!(restored.redacted_sequences(), vec![1]);
        assert_eq!(restored.hash(), log.hash());
    }
}
