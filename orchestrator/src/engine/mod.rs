mod graph;

pub use graph::*;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// The DAG scheduler. Manages node lifecycle, topological ordering, and concurrency.
pub struct Scheduler {
    pub graph: WorkGraph,
    pub max_parallel: usize,
}

impl Scheduler {
    pub fn new(graph: WorkGraph, max_parallel: usize) -> Self {
        Self {
            graph,
            max_parallel,
        }
    }

    /// Validate the graph is a DAG (no cycles). Returns Err with cycle description if invalid.
    pub fn validate(&self) -> Result<(), String> {
        // Kahn's algorithm: if topo sort doesn't consume all nodes, there's a cycle.
        let mut in_degree: BTreeMap<&str, usize> = BTreeMap::new();
        for node in &self.graph.nodes {
            in_degree.entry(&node.id).or_insert(0);
            for dep in &node.dependencies {
                *in_degree.entry(dep.as_str()).or_insert(0) += 0;
            }
        }

        // Build adjacency: dependency -> dependents
        let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for node in &self.graph.nodes {
            for dep in &node.dependencies {
                dependents.entry(dep.as_str()).or_default().push(&node.id);
                *in_degree.entry(&node.id).or_insert(0) += 1;
            }
        }

        let mut queue: VecDeque<&str> = VecDeque::new();
        for (id, &deg) in &in_degree {
            if deg == 0 {
                queue.push_back(id);
            }
        }

        let mut visited = 0usize;
        while let Some(id) = queue.pop_front() {
            visited += 1;
            if let Some(deps) = dependents.get(id) {
                for &dep in deps {
                    if let Some(d) = in_degree.get_mut(dep) {
                        *d -= 1;
                        if *d == 0 {
                            queue.push_back(dep);
                        }
                    }
                }
            }
        }

        if visited == self.graph.nodes.len() {
            Ok(())
        } else {
            Err(format!(
                "cycle detected: visited {visited} of {} nodes",
                self.graph.nodes.len()
            ))
        }
    }

    /// Compute the set of node IDs that are ready (all deps passed).
    pub fn ready_nodes(&self) -> Vec<String> {
        let passed: BTreeSet<&str> = self
            .graph
            .nodes
            .iter()
            .filter(|n| n.status == NodeStatus::Passed)
            .map(|n| n.id.as_str())
            .collect();

        let running_count = self
            .graph
            .nodes
            .iter()
            .filter(|n| n.status == NodeStatus::Running)
            .count();

        let available_slots = self.max_parallel.saturating_sub(running_count);

        self.graph
            .nodes
            .iter()
            .filter(|n| n.status == NodeStatus::Pending)
            .filter(|n| n.dependencies.iter().all(|d| passed.contains(d.as_str())))
            .take(available_slots)
            .map(|n| n.id.clone())
            .collect()
    }

    /// Transition pending nodes whose dependencies are met to ready state.
    /// Returns IDs of nodes that became ready.
    pub fn advance(&mut self) -> Vec<String> {
        let ready = self.ready_nodes();
        for id in &ready {
            if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == *id) {
                node.status = NodeStatus::Ready;
            }
        }
        ready
    }

    /// Assign a ready node to a role. Returns the node ID if one was assigned.
    pub fn assign_next(&mut self, role: Role) -> Option<String> {
        // First advance pending -> ready
        self.advance();

        let node = self
            .graph
            .nodes
            .iter_mut()
            .find(|n| n.status == NodeStatus::Ready && n.owner_role == role)?;

        node.status = NodeStatus::Running;
        Some(node.id.clone())
    }

    /// Mark a node as passed.
    pub fn mark_passed(&mut self, node_id: &str) -> Result<(), String> {
        let node = self
            .graph
            .nodes
            .iter_mut()
            .find(|n| n.id == node_id)
            .ok_or_else(|| format!("node not found: {node_id}"))?;
        node.status = NodeStatus::Passed;
        Ok(())
    }

    /// Mark a node as failed.
    pub fn mark_failed(&mut self, node_id: &str) -> Result<(), String> {
        let node = self
            .graph
            .nodes
            .iter_mut()
            .find(|n| n.id == node_id)
            .ok_or_else(|| format!("node not found: {node_id}"))?;
        node.status = NodeStatus::Failed;
        Ok(())
    }

    /// Mark a node as blocked.
    pub fn mark_blocked(&mut self, node_id: &str) -> Result<(), String> {
        let node = self
            .graph
            .nodes
            .iter_mut()
            .find(|n| n.id == node_id)
            .ok_or_else(|| format!("node not found: {node_id}"))?;
        node.status = NodeStatus::Blocked;
        Ok(())
    }

    /// Topological sort: returns node IDs in valid execution order.
    pub fn topological_order(&self) -> Result<Vec<String>, String> {
        let mut in_degree: BTreeMap<&str, usize> = BTreeMap::new();
        let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();

        for node in &self.graph.nodes {
            in_degree.entry(&node.id).or_insert(0);
            for dep in &node.dependencies {
                dependents.entry(dep.as_str()).or_default().push(&node.id);
                *in_degree.entry(&node.id).or_insert(0) += 1;
            }
        }

        let mut queue: VecDeque<&str> = VecDeque::new();
        for (id, &deg) in &in_degree {
            if deg == 0 {
                queue.push_back(id);
            }
        }

        let mut order = Vec::new();
        while let Some(id) = queue.pop_front() {
            order.push(id.to_string());
            if let Some(deps) = dependents.get(id) {
                for &dep in deps {
                    if let Some(d) = in_degree.get_mut(dep) {
                        *d -= 1;
                        if *d == 0 {
                            queue.push_back(dep);
                        }
                    }
                }
            }
        }

        if order.len() == self.graph.nodes.len() {
            Ok(order)
        } else {
            Err("cycle detected".to_string())
        }
    }

    /// Summary statistics.
    pub fn summary(&self) -> GraphSummary {
        let mut s = GraphSummary {
            total: self.graph.nodes.len(),
            ..GraphSummary::default()
        };
        for node in &self.graph.nodes {
            match node.status {
                NodeStatus::Pending => s.pending += 1,
                NodeStatus::Ready => s.ready += 1,
                NodeStatus::Running => s.running += 1,
                NodeStatus::Blocked => s.blocked += 1,
                NodeStatus::Failed => s.failed += 1,
                NodeStatus::Passed => s.passed += 1,
            }
        }
        s
    }
}

#[derive(Debug, Default, serde::Serialize)]
pub struct GraphSummary {
    pub total: usize,
    pub pending: usize,
    pub ready: usize,
    pub running: usize,
    pub blocked: usize,
    pub failed: usize,
    pub passed: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(id: &str, deps: &[&str], role: Role) -> WorkNode {
        WorkNode {
            id: id.to_string(),
            description: format!("node {id}"),
            inputs: vec![],
            outputs: vec![],
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
            owner_role: role,
            tags: vec![],
            status: NodeStatus::Pending,
            assigned_to: None,
            patch_bundle: None,
            review_result: None,
        }
    }

    #[test]
    fn test_validate_dag() {
        let graph = WorkGraph {
            schema_version: 1,
            id: "G-test".into(),
            description: "test".into(),
            nodes: vec![
                make_node("A", &[], Role::Implementer),
                make_node("B", &["A"], Role::Implementer),
                make_node("C", &["A"], Role::Reviewer),
            ],
        };
        let sched = Scheduler::new(graph, 4);
        assert!(sched.validate().is_ok());
    }

    #[test]
    fn test_detect_cycle() {
        let graph = WorkGraph {
            schema_version: 1,
            id: "G-cycle".into(),
            description: "test".into(),
            nodes: vec![
                make_node("A", &["C"], Role::Implementer),
                make_node("B", &["A"], Role::Implementer),
                make_node("C", &["B"], Role::Implementer),
            ],
        };
        let sched = Scheduler::new(graph, 4);
        assert!(sched.validate().is_err());
    }

    #[test]
    fn test_ready_nodes_initial() {
        let graph = WorkGraph {
            schema_version: 1,
            id: "G-test".into(),
            description: "test".into(),
            nodes: vec![
                make_node("A", &[], Role::Implementer),
                make_node("B", &["A"], Role::Implementer),
                make_node("C", &[], Role::Reviewer),
            ],
        };
        let sched = Scheduler::new(graph, 4);
        let ready = sched.ready_nodes();
        assert_eq!(ready, vec!["A", "C"]);
    }

    #[test]
    fn test_ready_after_pass() {
        let graph = WorkGraph {
            schema_version: 1,
            id: "G-test".into(),
            description: "test".into(),
            nodes: vec![
                make_node("A", &[], Role::Implementer),
                make_node("B", &["A"], Role::Implementer),
            ],
        };
        let mut sched = Scheduler::new(graph, 4);
        // B not ready yet
        assert_eq!(sched.ready_nodes(), vec!["A"]);
        // Pass A
        sched.graph.nodes[0].status = NodeStatus::Passed;
        assert_eq!(sched.ready_nodes(), vec!["B"]);
    }

    #[test]
    fn test_concurrency_limit() {
        let graph = WorkGraph {
            schema_version: 1,
            id: "G-test".into(),
            description: "test".into(),
            nodes: vec![
                make_node("A", &[], Role::Implementer),
                make_node("B", &[], Role::Implementer),
                make_node("C", &[], Role::Implementer),
            ],
        };
        let mut sched = Scheduler::new(graph, 2);
        // Only 2 should be ready due to max_parallel
        assert_eq!(sched.ready_nodes().len(), 2);
        // Mark one running
        sched.graph.nodes[0].status = NodeStatus::Running;
        // Now only 1 slot available
        assert_eq!(sched.ready_nodes().len(), 1);
    }

    #[test]
    fn test_topological_order() {
        let graph = WorkGraph {
            schema_version: 1,
            id: "G-test".into(),
            description: "test".into(),
            nodes: vec![
                make_node("A", &[], Role::Implementer),
                make_node("B", &["A"], Role::Implementer),
                make_node("C", &["A", "B"], Role::Reviewer),
            ],
        };
        let sched = Scheduler::new(graph, 4);
        let order = sched.topological_order().unwrap();
        let a_pos = order.iter().position(|x| x == "A").unwrap();
        let b_pos = order.iter().position(|x| x == "B").unwrap();
        let c_pos = order.iter().position(|x| x == "C").unwrap();
        assert!(a_pos < b_pos);
        assert!(b_pos < c_pos);
    }

    #[test]
    fn test_assign_next() {
        let graph = WorkGraph {
            schema_version: 1,
            id: "G-test".into(),
            description: "test".into(),
            nodes: vec![
                make_node("A", &[], Role::Implementer),
                make_node("B", &[], Role::Reviewer),
            ],
        };
        let mut sched = Scheduler::new(graph, 4);
        assert_eq!(sched.assign_next(Role::Implementer), Some("A".into()));
        assert_eq!(sched.assign_next(Role::Reviewer), Some("B".into()));
        // No more implementer nodes
        assert_eq!(sched.assign_next(Role::Implementer), None);
    }

    #[test]
    fn test_summary() {
        let graph = WorkGraph {
            schema_version: 1,
            id: "G-test".into(),
            description: "test".into(),
            nodes: vec![
                make_node("A", &[], Role::Implementer),
                make_node("B", &["A"], Role::Implementer),
                make_node("C", &[], Role::Reviewer),
            ],
        };
        let mut sched = Scheduler::new(graph, 4);
        sched.graph.nodes[0].status = NodeStatus::Passed;
        sched.graph.nodes[2].status = NodeStatus::Running;
        let s = sched.summary();
        assert_eq!(s.total, 3);
        assert_eq!(s.passed, 1);
        assert_eq!(s.running, 1);
        assert_eq!(s.pending, 1);
    }
}
