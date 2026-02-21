use boruna_orchestrator::workflow::definition::WorkflowDef;
use boruna_orchestrator::workflow::validator::WorkflowValidator;

/// Validate a workflow definition (JSON string).
pub fn validate_workflow(workflow_json: &str) -> String {
    // Parse the workflow definition
    let def: WorkflowDef = match serde_json::from_str(workflow_json) {
        Ok(d) => d,
        Err(e) => {
            return serde_json::json!({
                "success": false,
                "error_kind": "parse_error",
                "message": format!("invalid workflow JSON: {e}"),
            })
            .to_string();
        }
    };

    // Validate
    match WorkflowValidator::validate(&def) {
        Ok(()) => {
            // Get topological order
            let topo = WorkflowValidator::topological_order(&def);
            serde_json::json!({
                "success": true,
                "workflow_name": def.name,
                "workflow_version": def.version,
                "steps_count": def.steps.len(),
                "edges_count": def.edges.len(),
                "execution_order": topo.unwrap_or_default(),
            })
            .to_string()
        }
        Err(errors) => {
            let error_list: Vec<serde_json::Value> = errors
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "kind": format!("{:?}", e.kind),
                        "message": e.message,
                    })
                })
                .collect();
            serde_json::json!({
                "success": false,
                "error_kind": "validation_error",
                "errors": error_list,
            })
            .to_string()
        }
    }
}
