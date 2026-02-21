use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Module-level lock table. Each lock maps a module path to the holding node ID.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LockTable {
    pub locks: HashMap<String, LockEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockEntry {
    pub module: String,
    pub held_by: String,
    pub acquired_at: String,
}

impl LockTable {
    pub fn new() -> Self {
        Self {
            locks: HashMap::new(),
        }
    }

    /// Try to acquire locks for the given modules on behalf of a node.
    /// Returns Ok(()) if all locks acquired, Err with conflicting module and holder.
    pub fn acquire(
        &mut self,
        node_id: &str,
        modules: &[String],
        timestamp: &str,
    ) -> Result<(), LockConflict> {
        // Check for conflicts first
        for module in modules {
            if let Some(entry) = self.locks.get(module) {
                if entry.held_by != node_id {
                    return Err(LockConflict {
                        module: module.clone(),
                        requested_by: node_id.to_string(),
                        held_by: entry.held_by.clone(),
                    });
                }
            }
        }

        // No conflicts — acquire all
        for module in modules {
            self.locks.insert(
                module.clone(),
                LockEntry {
                    module: module.clone(),
                    held_by: node_id.to_string(),
                    acquired_at: timestamp.to_string(),
                },
            );
        }

        Ok(())
    }

    /// Release all locks held by a node.
    pub fn release(&mut self, node_id: &str) {
        self.locks.retain(|_, entry| entry.held_by != node_id);
    }

    /// Force-release a specific lock.
    pub fn force_release(&mut self, module: &str) {
        self.locks.remove(module);
    }

    /// Check if any of the given modules are locked by someone other than node_id.
    pub fn check_conflicts(&self, node_id: &str, modules: &[String]) -> Vec<LockConflict> {
        let mut conflicts = Vec::new();
        for module in modules {
            if let Some(entry) = self.locks.get(module) {
                if entry.held_by != node_id {
                    conflicts.push(LockConflict {
                        module: module.clone(),
                        requested_by: node_id.to_string(),
                        held_by: entry.held_by.clone(),
                    });
                }
            }
        }
        conflicts
    }

    /// List all active locks.
    pub fn active_locks(&self) -> Vec<&LockEntry> {
        self.locks.values().collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockConflict {
    pub module: String,
    pub requested_by: String,
    pub held_by: String,
}

impl std::fmt::Display for LockConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "lock conflict: module '{}' requested by '{}' but held by '{}'",
            self.module, self.requested_by, self.held_by
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_acquire_and_release() {
        let mut table = LockTable::new();
        let result = table.acquire("WN-001", &["boruna-bytecode".into()], "now");
        assert!(result.is_ok());
        assert_eq!(table.active_locks().len(), 1);

        table.release("WN-001");
        assert_eq!(table.active_locks().len(), 0);
    }

    #[test]
    fn test_conflict_detection() {
        let mut table = LockTable::new();
        table
            .acquire("WN-001", &["boruna-bytecode".into()], "now")
            .unwrap();

        let result = table.acquire("WN-002", &["boruna-bytecode".into()], "now");
        assert!(result.is_err());
        let conflict = result.unwrap_err();
        assert_eq!(conflict.module, "boruna-bytecode");
        assert_eq!(conflict.held_by, "WN-001");
    }

    #[test]
    fn test_same_node_reacquire() {
        let mut table = LockTable::new();
        table
            .acquire("WN-001", &["boruna-bytecode".into()], "now")
            .unwrap();
        // Same node can reacquire its own lock
        let result = table.acquire("WN-001", &["boruna-bytecode".into()], "now");
        assert!(result.is_ok());
    }

    #[test]
    fn test_multiple_modules() {
        let mut table = LockTable::new();
        table
            .acquire(
                "WN-001",
                &["boruna-bytecode".into(), "boruna-vm".into()],
                "now",
            )
            .unwrap();
        assert_eq!(table.active_locks().len(), 2);

        // Conflict on one module
        let conflicts =
            table.check_conflicts("WN-002", &["boruna-vm".into(), "boruna-compiler".into()]);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].module, "boruna-vm");
    }

    #[test]
    fn test_force_release() {
        let mut table = LockTable::new();
        table
            .acquire("WN-001", &["boruna-bytecode".into()], "now")
            .unwrap();
        table.force_release("boruna-bytecode");
        assert_eq!(table.active_locks().len(), 0);
    }

    #[test]
    fn test_no_conflict_different_modules() {
        let mut table = LockTable::new();
        table
            .acquire("WN-001", &["boruna-bytecode".into()], "now")
            .unwrap();
        let result = table.acquire("WN-002", &["boruna-vm".into()], "now");
        assert!(result.is_ok());
        assert_eq!(table.active_locks().len(), 2);
    }
}
