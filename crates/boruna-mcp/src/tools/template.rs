use std::collections::BTreeMap;
use std::path::Path;

/// List available templates in directory.
pub fn list_templates(dir: &str) -> String {
    let path = Path::new(dir);
    match boruna_tooling::templates::list_templates(path) {
        Ok(templates) => {
            let list: Vec<serde_json::Value> = templates
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "version": t.version,
                        "description": t.description,
                        "dependencies": t.dependencies,
                        "capabilities": t.capabilities,
                        "args": t.args.keys().collect::<Vec<_>>(),
                    })
                })
                .collect();
            serde_json::json!({
                "success": true,
                "templates": list,
                "count": list.len(),
            })
            .to_string()
        }
        Err(e) => serde_json::json!({
            "success": false,
            "error_kind": "template_error",
            "message": e,
        })
        .to_string(),
    }
}

/// Apply a template with arguments.
pub fn apply_template(dir: &str, name: &str, args: &[String], validate: bool) -> String {
    let path = Path::new(dir);

    // Parse args from "key=value" format
    let mut arg_map = BTreeMap::new();
    for arg in args {
        if let Some((k, v)) = arg.split_once('=') {
            arg_map.insert(k.to_string(), v.to_string());
        } else {
            return serde_json::json!({
                "success": false,
                "error_kind": "invalid_args",
                "message": format!("argument must be key=value format, got: {arg}"),
            })
            .to_string();
        }
    }

    match boruna_tooling::templates::apply_template(path, name, &arg_map) {
        Ok(result) => {
            let mut json = serde_json::json!({
                "success": true,
                "template_name": result.template_name,
                "output_file": result.output_file,
                "source": result.source,
                "dependencies": result.dependencies,
                "capabilities": result.capabilities,
            });

            if validate {
                match boruna_tooling::templates::validate_template_output(&result.source) {
                    Ok(()) => {
                        json["validation"] = serde_json::json!({"passed": true});
                    }
                    Err(e) => {
                        json["validation"] = serde_json::json!({"passed": false, "error": e});
                    }
                }
            }

            serde_json::to_string_pretty(&json).unwrap_or_else(|_| "{}".into())
        }
        Err(e) => serde_json::json!({
            "success": false,
            "error_kind": "template_error",
            "message": e,
        })
        .to_string(),
    }
}
