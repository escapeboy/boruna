use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::workflow::definition::{StepKind, WorkflowDef};

/// Validates a workflow definition for structural and semantic correctness.
pub struct WorkflowValidator;

/// A validation error.
#[derive(Debug, Clone)]
pub struct ValidationError {
    pub kind: ValidationErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationErrorKind {
    EmptyWorkflow,
    MissingField,
    CycleDetected,
    UnknownStep,
    UnknownInput,
    InvalidCapability,
    DuplicateEdge,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl WorkflowValidator {
    /// Validate a workflow definition. Returns a list of all errors found.
    pub fn validate(def: &WorkflowDef) -> Result<(), Vec<ValidationError>> {
        let mut errors = Vec::new();

        // Check basic fields
        if def.name.is_empty() {
            errors.push(ValidationError {
                kind: ValidationErrorKind::MissingField,
                message: "workflow name is empty".into(),
            });
        }
        if def.version.is_empty() {
            errors.push(ValidationError {
                kind: ValidationErrorKind::MissingField,
                message: "workflow version is empty".into(),
            });
        }
        if def.steps.is_empty() {
            errors.push(ValidationError {
                kind: ValidationErrorKind::EmptyWorkflow,
                message: "workflow has no steps".into(),
            });
        }

        // Collect all step IDs
        let step_ids: BTreeSet<&str> = def.steps.keys().map(|s| s.as_str()).collect();

        // Validate edges reference existing steps
        for (from, to) in &def.edges {
            if !step_ids.contains(from.as_str()) {
                errors.push(ValidationError {
                    kind: ValidationErrorKind::UnknownStep,
                    message: format!("edge references unknown step '{from}'"),
                });
            }
            if !step_ids.contains(to.as_str()) {
                errors.push(ValidationError {
                    kind: ValidationErrorKind::UnknownStep,
                    message: format!("edge references unknown step '{to}'"),
                });
            }
        }

        // Validate depends_on references
        for (id, step) in &def.steps {
            for dep in &step.depends_on {
                if !step_ids.contains(dep.as_str()) {
                    errors.push(ValidationError {
                        kind: ValidationErrorKind::UnknownStep,
                        message: format!("step '{id}' depends on unknown step '{dep}'"),
                    });
                }
            }
        }

        // Validate input references (format: "step_id.output_name")
        for (id, step) in &def.steps {
            for (input_name, input_ref) in &step.inputs {
                if let Some((ref_step, _ref_output)) = input_ref.split_once('.') {
                    if !step_ids.contains(ref_step) {
                        errors.push(ValidationError {
                            kind: ValidationErrorKind::UnknownInput,
                            message: format!(
                                "step '{id}' input '{input_name}' references unknown step '{ref_step}'"
                            ),
                        });
                    }
                } else {
                    errors.push(ValidationError {
                        kind: ValidationErrorKind::UnknownInput,
                        message: format!(
                            "step '{id}' input '{input_name}' has invalid ref format '{}' (expected 'step.output')",
                            input_ref
                        ),
                    });
                }
            }
        }

        // Validate source steps have a source file
        for (id, step) in &def.steps {
            if let StepKind::Source { source } = &step.kind {
                if source.is_empty() {
                    errors.push(ValidationError {
                        kind: ValidationErrorKind::MissingField,
                        message: format!("step '{id}' has empty source path"),
                    });
                }
            }
        }

        // Check for duplicate edges
        let mut seen_edges = BTreeSet::new();
        for edge in &def.edges {
            if !seen_edges.insert(edge) {
                errors.push(ValidationError {
                    kind: ValidationErrorKind::DuplicateEdge,
                    message: format!("duplicate edge: {} -> {}", edge.0, edge.1),
                });
            }
        }

        // DAG acyclicity check (Kahn's algorithm)
        if let Err(msg) = Self::check_acyclic(def) {
            errors.push(ValidationError {
                kind: ValidationErrorKind::CycleDetected,
                message: msg,
            });
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Compute topological order of steps. Returns step IDs in execution order.
    pub fn topological_order(def: &WorkflowDef) -> Result<Vec<String>, String> {
        let mut in_degree: BTreeMap<&str, usize> = BTreeMap::new();
        let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();

        for id in def.steps.keys() {
            in_degree.entry(id.as_str()).or_insert(0);
        }

        // Build edges from both explicit edges and depends_on
        let all_edges = Self::all_edges(def);
        for (from, to) in &all_edges {
            dependents
                .entry(from.as_str())
                .or_default()
                .push(to.as_str());
            *in_degree.entry(to.as_str()).or_insert(0) += 1;
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

        if order.len() == def.steps.len() {
            Ok(order)
        } else {
            Err(format!(
                "cycle detected: resolved {} of {} steps",
                order.len(),
                def.steps.len()
            ))
        }
    }

    /// Compute topological **levels** of steps for wave-based concurrent
    /// execution: a `Vec<Vec<String>>` where each inner Vec is a "wave"
    /// — all steps in level N have all their dependencies at levels < N.
    /// Within a wave, steps can execute in parallel.
    ///
    /// Returns `Err` if the graph has a cycle (same condition as
    /// `topological_order`). Within each wave, step IDs are sorted
    /// alphabetically — deterministic output regardless of how the
    /// underlying BTreeMap iterators surface ties.
    ///
    /// Example: a fan-out workflow `ingest → {classify, extract,
    /// summarize} → merge` produces:
    /// ```text
    /// [["ingest"], ["classify", "extract", "summarize"], ["merge"]]
    /// ```
    /// Sequential execution would visit each step in order. Concurrent
    /// execution at level 1 runs the three branches in parallel.
    ///
    /// Introduced in `0.3-S4` for the wave-based scheduler.
    pub fn topological_levels(def: &WorkflowDef) -> Result<Vec<Vec<String>>, String> {
        let mut in_degree: BTreeMap<String, usize> = BTreeMap::new();
        let mut dependents: BTreeMap<String, Vec<String>> = BTreeMap::new();

        for id in def.steps.keys() {
            in_degree.entry(id.clone()).or_insert(0);
        }
        let all_edges = Self::all_edges(def);
        for (from, to) in &all_edges {
            dependents.entry(from.clone()).or_default().push(to.clone());
            *in_degree.entry(to.clone()).or_insert(0) += 1;
        }

        let mut levels: Vec<Vec<String>> = Vec::new();
        let mut current: Vec<String> = in_degree
            .iter()
            .filter_map(|(id, &deg)| if deg == 0 { Some(id.clone()) } else { None })
            .collect();
        current.sort();
        let mut total = 0usize;

        while !current.is_empty() {
            total += current.len();
            // Compute next level: any step that loses its last incoming
            // dependency this round.
            let mut next: Vec<String> = Vec::new();
            for id in &current {
                if let Some(deps) = dependents.get(id) {
                    for d in deps {
                        if let Some(deg) = in_degree.get_mut(d) {
                            *deg -= 1;
                            if *deg == 0 {
                                next.push(d.clone());
                            }
                        }
                    }
                }
            }
            next.sort();
            levels.push(current);
            current = next;
        }

        if total == def.steps.len() {
            Ok(levels)
        } else {
            Err(format!(
                "cycle detected: resolved {total} of {} steps",
                def.steps.len()
            ))
        }
    }

    /// Collect all edges from both the edges list and depends_on fields.
    fn all_edges(def: &WorkflowDef) -> Vec<(String, String)> {
        let mut edges: BTreeSet<(String, String)> = BTreeSet::new();

        for (from, to) in &def.edges {
            edges.insert((from.clone(), to.clone()));
        }

        for (id, step) in &def.steps {
            for dep in &step.depends_on {
                edges.insert((dep.clone(), id.clone()));
            }
        }

        edges.into_iter().collect()
    }

    fn check_acyclic(def: &WorkflowDef) -> Result<(), String> {
        Self::topological_order(def).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::definition::*;

    fn simple_source_step(source: &str) -> StepDef {
        StepDef {
            kind: StepKind::Source {
                source: source.into(),
            },
            capabilities: vec![],
            inputs: BTreeMap::new(),
            outputs: BTreeMap::new(),
            depends_on: vec![],
            timeout_ms: None,
            retry: None,
            budget: None,
            required_capability_versions: Default::default(),
        }
    }

    #[test]
    fn test_validate_valid_linear_workflow() {
        let def = WorkflowDef {
            schema_version: 1,
            name: "test".into(),
            version: "1.0.0".into(),
            description: "test workflow".into(),
            steps: BTreeMap::from([
                ("fetch".into(), simple_source_step("steps/fetch.ax")),
                ("transform".into(), simple_source_step("steps/transform.ax")),
                ("store".into(), simple_source_step("steps/store.ax")),
            ]),
            edges: vec![
                ("fetch".into(), "transform".into()),
                ("transform".into(), "store".into()),
            ],
        };
        assert!(WorkflowValidator::validate(&def).is_ok());
    }

    #[test]
    fn test_validate_detects_cycle() {
        let def = WorkflowDef {
            schema_version: 1,
            name: "cycle".into(),
            version: "1.0.0".into(),
            description: String::new(),
            steps: BTreeMap::from([
                ("a".into(), simple_source_step("a.ax")),
                ("b".into(), simple_source_step("b.ax")),
                ("c".into(), simple_source_step("c.ax")),
            ]),
            edges: vec![
                ("a".into(), "b".into()),
                ("b".into(), "c".into()),
                ("c".into(), "a".into()),
            ],
        };
        let errors = WorkflowValidator::validate(&def).unwrap_err();
        assert!(errors
            .iter()
            .any(|e| e.kind == ValidationErrorKind::CycleDetected));
    }

    #[test]
    fn test_validate_unknown_edge_step() {
        let def = WorkflowDef {
            schema_version: 1,
            name: "bad-edge".into(),
            version: "1.0.0".into(),
            description: String::new(),
            steps: BTreeMap::from([("a".into(), simple_source_step("a.ax"))]),
            edges: vec![("a".into(), "nonexistent".into())],
        };
        let errors = WorkflowValidator::validate(&def).unwrap_err();
        assert!(errors
            .iter()
            .any(|e| e.kind == ValidationErrorKind::UnknownStep));
    }

    #[test]
    fn test_validate_empty_workflow() {
        let def = WorkflowDef {
            schema_version: 1,
            name: "empty".into(),
            version: "1.0.0".into(),
            description: String::new(),
            steps: BTreeMap::new(),
            edges: vec![],
        };
        let errors = WorkflowValidator::validate(&def).unwrap_err();
        assert!(errors
            .iter()
            .any(|e| e.kind == ValidationErrorKind::EmptyWorkflow));
    }

    #[test]
    fn test_validate_bad_input_ref() {
        let mut step = simple_source_step("a.ax");
        step.inputs.insert("data".into(), "no_dot_format".into());
        let def = WorkflowDef {
            schema_version: 1,
            name: "bad-ref".into(),
            version: "1.0.0".into(),
            description: String::new(),
            steps: BTreeMap::from([("a".into(), step)]),
            edges: vec![],
        };
        let errors = WorkflowValidator::validate(&def).unwrap_err();
        assert!(errors
            .iter()
            .any(|e| e.kind == ValidationErrorKind::UnknownInput));
    }

    #[test]
    fn test_topological_order_linear() {
        let def = WorkflowDef {
            schema_version: 1,
            name: "linear".into(),
            version: "1.0.0".into(),
            description: String::new(),
            steps: BTreeMap::from([
                ("a".into(), simple_source_step("a.ax")),
                ("b".into(), simple_source_step("b.ax")),
                ("c".into(), simple_source_step("c.ax")),
            ]),
            edges: vec![("a".into(), "b".into()), ("b".into(), "c".into())],
        };
        let order = WorkflowValidator::topological_order(&def).unwrap();
        let a_pos = order.iter().position(|x| x == "a").unwrap();
        let b_pos = order.iter().position(|x| x == "b").unwrap();
        let c_pos = order.iter().position(|x| x == "c").unwrap();
        assert!(a_pos < b_pos);
        assert!(b_pos < c_pos);
    }

    #[test]
    fn test_topological_order_diamond() {
        let def = WorkflowDef {
            schema_version: 1,
            name: "diamond".into(),
            version: "1.0.0".into(),
            description: String::new(),
            steps: BTreeMap::from([
                ("a".into(), simple_source_step("a.ax")),
                ("b".into(), simple_source_step("b.ax")),
                ("c".into(), simple_source_step("c.ax")),
                ("d".into(), simple_source_step("d.ax")),
            ]),
            edges: vec![
                ("a".into(), "b".into()),
                ("a".into(), "c".into()),
                ("b".into(), "d".into()),
                ("c".into(), "d".into()),
            ],
        };
        let order = WorkflowValidator::topological_order(&def).unwrap();
        let a_pos = order.iter().position(|x| x == "a").unwrap();
        let d_pos = order.iter().position(|x| x == "d").unwrap();
        assert!(a_pos < d_pos);
    }

    #[test]
    fn test_validate_with_depends_on() {
        let mut step_b = simple_source_step("b.ax");
        step_b.depends_on = vec!["a".into()];
        let def = WorkflowDef {
            schema_version: 1,
            name: "deps".into(),
            version: "1.0.0".into(),
            description: String::new(),
            steps: BTreeMap::from([
                ("a".into(), simple_source_step("a.ax")),
                ("b".into(), step_b),
            ]),
            edges: vec![],
        };
        assert!(WorkflowValidator::validate(&def).is_ok());
        let order = WorkflowValidator::topological_order(&def).unwrap();
        assert_eq!(order, vec!["a", "b"]);
    }

    #[test]
    fn topological_levels_linear() {
        let def = WorkflowDef {
            schema_version: 1,
            name: "linear".into(),
            version: "1.0.0".into(),
            description: String::new(),
            steps: BTreeMap::from([
                ("a".into(), simple_source_step("a.ax")),
                ("b".into(), simple_source_step("b.ax")),
                ("c".into(), simple_source_step("c.ax")),
            ]),
            edges: vec![("a".into(), "b".into()), ("b".into(), "c".into())],
        };
        let levels = WorkflowValidator::topological_levels(&def).unwrap();
        assert_eq!(levels, vec![vec!["a"], vec!["b"], vec!["c"]]);
    }

    #[test]
    fn topological_levels_fan_out() {
        // ingest → {classify, extract, summarize} → merge.
        // Three middle steps share level 1; "merge" alone in level 2.
        let mut classify = simple_source_step("classify.ax");
        classify.depends_on = vec!["ingest".into()];
        let mut extract = simple_source_step("extract.ax");
        extract.depends_on = vec!["ingest".into()];
        let mut summarize = simple_source_step("summarize.ax");
        summarize.depends_on = vec!["ingest".into()];
        let mut merge = simple_source_step("merge.ax");
        merge.depends_on = vec!["classify".into(), "extract".into(), "summarize".into()];
        let def = WorkflowDef {
            schema_version: 1,
            name: "fan-out".into(),
            version: "1.0.0".into(),
            description: String::new(),
            steps: BTreeMap::from([
                ("ingest".into(), simple_source_step("ingest.ax")),
                ("classify".into(), classify),
                ("extract".into(), extract),
                ("summarize".into(), summarize),
                ("merge".into(), merge),
            ]),
            edges: vec![],
        };
        let levels = WorkflowValidator::topological_levels(&def).unwrap();
        assert_eq!(levels.len(), 3);
        assert_eq!(levels[0], vec!["ingest"]);
        // Level 1 sorted alphabetically: classify, extract, summarize.
        assert_eq!(levels[1], vec!["classify", "extract", "summarize"]);
        assert_eq!(levels[2], vec!["merge"]);
    }

    #[test]
    fn topological_levels_diamond() {
        let mut b = simple_source_step("b.ax");
        b.depends_on = vec!["a".into()];
        let mut c = simple_source_step("c.ax");
        c.depends_on = vec!["a".into()];
        let mut d = simple_source_step("d.ax");
        d.depends_on = vec!["b".into(), "c".into()];
        let def = WorkflowDef {
            schema_version: 1,
            name: "diamond".into(),
            version: "1.0.0".into(),
            description: String::new(),
            steps: BTreeMap::from([
                ("a".into(), simple_source_step("a.ax")),
                ("b".into(), b),
                ("c".into(), c),
                ("d".into(), d),
            ]),
            edges: vec![],
        };
        let levels = WorkflowValidator::topological_levels(&def).unwrap();
        assert_eq!(levels, vec![vec!["a"], vec!["b", "c"], vec!["d"]]);
    }

    #[test]
    fn topological_levels_cycle_errors() {
        let def = WorkflowDef {
            schema_version: 1,
            name: "cycle".into(),
            version: "1.0.0".into(),
            description: String::new(),
            steps: BTreeMap::from([
                ("a".into(), simple_source_step("a.ax")),
                ("b".into(), simple_source_step("b.ax")),
            ]),
            edges: vec![("a".into(), "b".into()), ("b".into(), "a".into())],
        };
        assert!(WorkflowValidator::topological_levels(&def).is_err());
    }

    #[test]
    fn test_validate_approval_gate() {
        let def = WorkflowDef {
            schema_version: 1,
            name: "approval".into(),
            version: "1.0.0".into(),
            description: String::new(),
            steps: BTreeMap::from([
                ("analyze".into(), simple_source_step("analyze.ax")),
                (
                    "approve".into(),
                    StepDef {
                        kind: StepKind::ApprovalGate {
                            required_role: "reviewer".into(),
                            condition: None,
                        },
                        capabilities: vec![],
                        inputs: BTreeMap::new(),
                        outputs: BTreeMap::new(),
                        depends_on: vec!["analyze".into()],
                        timeout_ms: None,
                        retry: None,
                        budget: None,
                        required_capability_versions: Default::default(),
                    },
                ),
                ("store".into(), simple_source_step("store.ax")),
            ]),
            edges: vec![("approve".into(), "store".into())],
        };
        assert!(WorkflowValidator::validate(&def).is_ok());
    }
}
