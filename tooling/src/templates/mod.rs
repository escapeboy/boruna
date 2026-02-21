//! Template engine for generating deterministic framework apps.
//!
//! Templates are `.ax.template` files with `{{variable}}` placeholders.
//! Each template has a `template.json` manifest describing args.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Template manifest (template.json).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateManifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub dependencies: Vec<String>,
    pub capabilities: Vec<String>,
    pub args: BTreeMap<String, ArgSpec>,
}

/// Argument specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArgSpec {
    #[serde(rename = "type")]
    pub arg_type: String,
    pub required: bool,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
}

/// Result of template application.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateResult {
    pub template_name: String,
    pub output_file: String,
    pub source: String,
    pub dependencies: Vec<String>,
    pub capabilities: Vec<String>,
}

/// List available templates in a directory.
pub fn list_templates(templates_dir: &Path) -> Result<Vec<TemplateManifest>, String> {
    let mut templates = Vec::new();
    if !templates_dir.exists() {
        return Ok(templates);
    }
    let entries =
        std::fs::read_dir(templates_dir).map_err(|e| format!("read templates dir: {e}"))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("read entry: {e}"))?;
        let path = entry.path();
        if path.is_dir() {
            let manifest_path = path.join("template.json");
            if manifest_path.exists() {
                let data = std::fs::read_to_string(&manifest_path)
                    .map_err(|e| format!("read {}: {e}", manifest_path.display()))?;
                let manifest: TemplateManifest = serde_json::from_str(&data)
                    .map_err(|e| format!("parse {}: {e}", manifest_path.display()))?;
                templates.push(manifest);
            }
        }
    }
    templates.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(templates)
}

/// Load a template manifest by name.
pub fn load_template(templates_dir: &Path, name: &str) -> Result<TemplateManifest, String> {
    let manifest_path = templates_dir.join(name).join("template.json");
    if !manifest_path.exists() {
        return Err(format!("template '{name}' not found"));
    }
    let data = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("read template manifest: {e}"))?;
    serde_json::from_str(&data).map_err(|e| format!("parse template manifest: {e}"))
}

/// Apply a template with given arguments.
pub fn apply_template(
    templates_dir: &Path,
    name: &str,
    args: &BTreeMap<String, String>,
) -> Result<TemplateResult, String> {
    let manifest = load_template(templates_dir, name)?;

    // Validate required args
    for (arg_name, spec) in &manifest.args {
        if spec.required && !args.contains_key(arg_name) {
            return Err(format!("missing required argument: {arg_name}"));
        }
    }

    // Read template file
    let template_path = templates_dir.join(name).join("app.ax.template");
    if !template_path.exists() {
        return Err("template file not found: app.ax.template".to_string());
    }
    let template =
        std::fs::read_to_string(&template_path).map_err(|e| format!("read template: {e}"))?;

    // Substitute variables
    let source = substitute(&template, args);

    Ok(TemplateResult {
        template_name: manifest.name.clone(),
        output_file: format!("{name}_app.ax"),
        source,
        dependencies: manifest.dependencies.clone(),
        capabilities: manifest.capabilities.clone(),
    })
}

/// Apply a template from a template string (no filesystem).
pub fn apply_template_string(template: &str, args: &BTreeMap<String, String>) -> String {
    substitute(template, args)
}

/// Substitute `{{key}}` placeholders in template.
fn substitute(template: &str, args: &BTreeMap<String, String>) -> String {
    let mut result = template.to_string();
    for (key, value) in args {
        let placeholder = format!("{{{{{key}}}}}");
        result = result.replace(&placeholder, value);
    }
    result
}

/// Validate that generated source compiles.
pub fn validate_template_output(source: &str) -> Result<(), String> {
    boruna_compiler::compile("template_output", source)
        .map_err(|e| format!("template output does not compile: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_substitute_basic() {
        let template = "Hello {{name}}, welcome to {{place}}!";
        let mut args = BTreeMap::new();
        args.insert("name".into(), "Alice".into());
        args.insert("place".into(), "Wonderland".into());
        let result = substitute(template, &args);
        assert_eq!(result, "Hello Alice, welcome to Wonderland!");
    }

    #[test]
    fn test_substitute_repeated() {
        let template = "{{x}} + {{x}} = {{result}}";
        let mut args = BTreeMap::new();
        args.insert("x".into(), "2".into());
        args.insert("result".into(), "4".into());
        let result = substitute(template, &args);
        assert_eq!(result, "2 + 2 = 4");
    }

    #[test]
    fn test_substitute_no_match() {
        let template = "no placeholders here";
        let args = BTreeMap::new();
        let result = substitute(template, &args);
        assert_eq!(result, "no placeholders here");
    }

    #[test]
    fn test_apply_template_string() {
        let template = "// Entity: {{entity}}\nfn main() -> Int { 0 }";
        let mut args = BTreeMap::new();
        args.insert("entity".into(), "users".into());
        let result = apply_template_string(template, &args);
        assert!(result.contains("Entity: users"));
    }

    #[test]
    fn test_list_templates() {
        let dir = tempfile::tempdir().unwrap();
        let t_dir = dir.path().join("test-template");
        std::fs::create_dir_all(&t_dir).unwrap();
        let manifest = TemplateManifest {
            name: "test-template".into(),
            version: "0.1.0".into(),
            description: "A test".into(),
            dependencies: vec![],
            capabilities: vec![],
            args: BTreeMap::new(),
        };
        let json = serde_json::to_string_pretty(&manifest).unwrap();
        std::fs::write(t_dir.join("template.json"), &json).unwrap();

        let templates = list_templates(dir.path()).unwrap();
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].name, "test-template");
    }

    #[test]
    fn test_apply_template_fs() {
        let dir = tempfile::tempdir().unwrap();
        let t_dir = dir.path().join("demo");
        std::fs::create_dir_all(&t_dir).unwrap();

        let manifest = TemplateManifest {
            name: "demo".into(),
            version: "0.1.0".into(),
            description: "Demo".into(),
            dependencies: vec!["std.ui".into()],
            capabilities: vec!["db.query".into()],
            args: {
                let mut m = BTreeMap::new();
                m.insert(
                    "entity".into(),
                    ArgSpec {
                        arg_type: "string".into(),
                        required: true,
                        description: "Entity name".into(),
                        default: None,
                    },
                );
                m
            },
        };
        std::fs::write(
            t_dir.join("template.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();
        std::fs::write(
            t_dir.join("app.ax.template"),
            "// App for {{entity}}\nfn main() -> Int { 0 }\n",
        )
        .unwrap();

        let mut args = BTreeMap::new();
        args.insert("entity".into(), "products".into());
        let result = apply_template(dir.path(), "demo", &args).unwrap();
        assert_eq!(result.template_name, "demo");
        assert!(result.source.contains("App for products"));
        assert_eq!(result.dependencies, vec!["std.ui"]);
        assert_eq!(result.capabilities, vec!["db.query"]);
    }

    #[test]
    fn test_apply_template_missing_arg() {
        let dir = tempfile::tempdir().unwrap();
        let t_dir = dir.path().join("demo");
        std::fs::create_dir_all(&t_dir).unwrap();

        let manifest = TemplateManifest {
            name: "demo".into(),
            version: "0.1.0".into(),
            description: "Demo".into(),
            dependencies: vec![],
            capabilities: vec![],
            args: {
                let mut m = BTreeMap::new();
                m.insert(
                    "name".into(),
                    ArgSpec {
                        arg_type: "string".into(),
                        required: true,
                        description: "Required".into(),
                        default: None,
                    },
                );
                m
            },
        };
        std::fs::write(
            t_dir.join("template.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();
        std::fs::write(t_dir.join("app.ax.template"), "hello").unwrap();

        let args = BTreeMap::new();
        let err = apply_template(dir.path(), "demo", &args).unwrap_err();
        assert!(err.contains("missing required argument"));
    }

    #[test]
    fn test_validate_template_output_valid() {
        let source = "fn main() -> Int { 42 }\n";
        assert!(validate_template_output(source).is_ok());
    }

    #[test]
    fn test_validate_template_output_invalid() {
        let source = "this is not valid code {{{";
        assert!(validate_template_output(source).is_err());
    }

    #[test]
    fn test_template_manifest_roundtrip() {
        let manifest = TemplateManifest {
            name: "test".into(),
            version: "0.1.0".into(),
            description: "A test template".into(),
            dependencies: vec!["std.ui".into()],
            capabilities: vec!["db.query".into()],
            args: BTreeMap::new(),
        };
        let json = serde_json::to_string(&manifest).unwrap();
        let parsed: TemplateManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test");
        assert_eq!(parsed.dependencies.len(), 1);
    }
}
