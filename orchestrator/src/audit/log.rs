use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A single entry in the hash-chained audit log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub sequence: u64,
    pub prev_hash: String,
    pub event: AuditEvent,
    pub entry_hash: String,
}

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
    pub fn append(&mut self, event: AuditEvent) -> String {
        let sequence = self.entries.len() as u64;
        let prev_hash = self
            .entries
            .last()
            .map(|e| e.entry_hash.clone())
            .unwrap_or_else(|| "0".repeat(64));

        let entry_hash = Self::compute_hash(sequence, &prev_hash, &event);

        self.entries.push(AuditEntry {
            sequence,
            prev_hash,
            event,
            entry_hash: entry_hash.clone(),
        });

        entry_hash
    }

    /// Verify the integrity of the entire chain. Returns Ok(()) or the index of the first bad entry.
    pub fn verify(&self) -> Result<(), u64> {
        let mut expected_prev = "0".repeat(64);

        for entry in &self.entries {
            if entry.prev_hash != expected_prev {
                return Err(entry.sequence);
            }
            let computed = Self::compute_hash(entry.sequence, &entry.prev_hash, &entry.event);
            if computed != entry.entry_hash {
                return Err(entry.sequence);
            }
            expected_prev = entry.entry_hash.clone();
        }

        Ok(())
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

    fn compute_hash(sequence: u64, prev_hash: &str, event: &AuditEvent) -> String {
        let event_json = serde_json::to_string(event).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(sequence.to_le_bytes());
        hasher.update(prev_hash.as_bytes());
        hasher.update(event_json.as_bytes());
        format!("{:x}", hasher.finalize())
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
}
