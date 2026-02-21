use std::fs;
use std::path::{Path, PathBuf};

use crate::conflict::LockTable;
use crate::engine::WorkGraph;

/// Local file-based JSON storage for orchestrator state.
pub struct Store {
    base_dir: PathBuf,
}

impl Store {
    pub fn new(base_dir: &Path) -> Result<Self, String> {
        let graphs_dir = base_dir.join("graphs");
        let bundles_dir = base_dir.join("bundles");
        let locks_dir = base_dir.join("locks");
        let gates_dir = base_dir.join("gates");

        for dir in [&graphs_dir, &bundles_dir, &locks_dir, &gates_dir] {
            fs::create_dir_all(dir)
                .map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
        }

        Ok(Self {
            base_dir: base_dir.to_path_buf(),
        })
    }

    /// Save a work graph.
    pub fn save_graph(&self, graph: &WorkGraph) -> Result<(), String> {
        let path = self
            .base_dir
            .join("graphs")
            .join(format!("{}.json", graph.id));
        let json =
            serde_json::to_string_pretty(graph).map_err(|e| format!("serialize error: {e}"))?;
        fs::write(&path, json).map_err(|e| format!("write error: {e}"))
    }

    /// Load a work graph by ID.
    pub fn load_graph(&self, graph_id: &str) -> Result<WorkGraph, String> {
        let path = self
            .base_dir
            .join("graphs")
            .join(format!("{graph_id}.json"));
        let data = fs::read_to_string(&path).map_err(|e| format!("read error: {e}"))?;
        serde_json::from_str(&data).map_err(|e| format!("parse error: {e}"))
    }

    /// List all graph IDs.
    pub fn list_graphs(&self) -> Result<Vec<String>, String> {
        let dir = self.base_dir.join("graphs");
        let mut ids = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|e| format!("read dir error: {e}"))? {
            let entry = entry.map_err(|e| format!("entry error: {e}"))?;
            if let Some(name) = entry.path().file_stem() {
                ids.push(name.to_string_lossy().to_string());
            }
        }
        Ok(ids)
    }

    /// Find the most recently modified graph.
    pub fn latest_graph(&self) -> Result<Option<String>, String> {
        let dir = self.base_dir.join("graphs");
        let mut latest: Option<(String, std::time::SystemTime)> = None;

        for entry in fs::read_dir(&dir).map_err(|e| format!("read dir error: {e}"))? {
            let entry = entry.map_err(|e| format!("entry error: {e}"))?;
            let meta = entry
                .metadata()
                .map_err(|e| format!("metadata error: {e}"))?;
            let modified = meta
                .modified()
                .map_err(|e| format!("modified error: {e}"))?;
            if let Some(stem) = entry.path().file_stem() {
                let id = stem.to_string_lossy().to_string();
                match &latest {
                    None => latest = Some((id, modified)),
                    Some((_, prev_time)) if modified > *prev_time => {
                        latest = Some((id, modified));
                    }
                    _ => {}
                }
            }
        }

        Ok(latest.map(|(id, _)| id))
    }

    /// Save lock table.
    pub fn save_locks(&self, locks: &LockTable) -> Result<(), String> {
        let path = self.base_dir.join("locks").join("locks.json");
        let json =
            serde_json::to_string_pretty(locks).map_err(|e| format!("serialize error: {e}"))?;
        fs::write(&path, json).map_err(|e| format!("write error: {e}"))
    }

    /// Load lock table.
    pub fn load_locks(&self) -> Result<LockTable, String> {
        let path = self.base_dir.join("locks").join("locks.json");
        if !path.exists() {
            return Ok(LockTable::new());
        }
        let data = fs::read_to_string(&path).map_err(|e| format!("read error: {e}"))?;
        serde_json::from_str(&data).map_err(|e| format!("parse error: {e}"))
    }

    /// Save gate results for a node.
    pub fn save_gate_result(
        &self,
        node_id: &str,
        result: &serde_json::Value,
    ) -> Result<(), String> {
        let path = self
            .base_dir
            .join("gates")
            .join(format!("{node_id}.gate.json"));
        let json =
            serde_json::to_string_pretty(result).map_err(|e| format!("serialize error: {e}"))?;
        fs::write(&path, json).map_err(|e| format!("write error: {e}"))
    }

    /// Load gate results for a node.
    pub fn load_gate_result(&self, node_id: &str) -> Result<serde_json::Value, String> {
        let path = self
            .base_dir
            .join("gates")
            .join(format!("{node_id}.gate.json"));
        let data = fs::read_to_string(&path).map_err(|e| format!("read error: {e}"))?;
        serde_json::from_str(&data).map_err(|e| format!("parse error: {e}"))
    }

    /// Path to the bundles directory.
    pub fn bundles_dir(&self) -> PathBuf {
        self.base_dir.join("bundles")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{NodeStatus, Role, WorkNode};

    #[test]
    fn test_save_and_load_graph() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();

        let graph = WorkGraph {
            schema_version: 1,
            id: "G-test".into(),
            description: "test graph".into(),
            nodes: vec![WorkNode {
                id: "WN-001".into(),
                description: "node 1".into(),
                inputs: vec![],
                outputs: vec!["boruna-bytecode".into()],
                dependencies: vec![],
                owner_role: Role::Implementer,
                tags: vec!["compiler".into()],
                status: NodeStatus::Pending,
                assigned_to: None,
                patch_bundle: None,
                review_result: None,
            }],
        };

        store.save_graph(&graph).unwrap();
        let loaded = store.load_graph("G-test").unwrap();
        assert_eq!(loaded.id, "G-test");
        assert_eq!(loaded.nodes.len(), 1);
        assert_eq!(loaded.nodes[0].id, "WN-001");
    }

    #[test]
    fn test_list_graphs() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();

        let g1 = WorkGraph {
            schema_version: 1,
            id: "G-001".into(),
            description: "a".into(),
            nodes: vec![],
        };
        let g2 = WorkGraph {
            schema_version: 1,
            id: "G-002".into(),
            description: "b".into(),
            nodes: vec![],
        };
        store.save_graph(&g1).unwrap();
        store.save_graph(&g2).unwrap();

        let ids = store.list_graphs().unwrap();
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn test_save_and_load_locks() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();

        let mut locks = LockTable::new();
        locks
            .acquire("WN-001", &["boruna-bytecode".into()], "now")
            .unwrap();

        store.save_locks(&locks).unwrap();
        let loaded = store.load_locks().unwrap();
        assert_eq!(loaded.active_locks().len(), 1);
    }

    #[test]
    fn test_load_empty_locks() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let locks = store.load_locks().unwrap();
        assert_eq!(locks.active_locks().len(), 0);
    }

    #[test]
    fn test_gate_results() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();

        let result = serde_json::json!({
            "compile": {"status": "pass"},
            "test": {"status": "pass", "total": 179},
        });
        store.save_gate_result("WN-001", &result).unwrap();
        let loaded = store.load_gate_result("WN-001").unwrap();
        assert_eq!(loaded["test"]["total"], 179);
    }
}
