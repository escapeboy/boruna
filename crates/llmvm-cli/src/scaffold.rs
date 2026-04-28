//! `boruna new` — interactive scaffold for new workflows from templates.
//!
//! Wraps `boruna_tooling::templates` with stdin-driven prompting. The
//! caller supplies a reader (stdin or a `Cursor` in tests) and a writer
//! (stdout or a `Vec<u8>` in tests) so the prompt loop is fully
//! testable. Non-interactive behaviour is requested via `no_input`,
//! which errors loudly on missing values rather than silently picking
//! defaults the user didn't see.
//!
//! See `docs/design-boruna-new-scaffold.md` for the design.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use boruna_tooling::templates;

/// Arguments parsed by `clap` for `boruna new`.
#[derive(Debug, Clone)]
pub struct NewArgs {
    /// Optional template name. If absent, the user is prompted to pick.
    pub template: Option<String>,
    /// Templates directory (defaults to ./templates).
    pub templates_dir: PathBuf,
    /// Target directory for generated files. Prompted if absent.
    pub target: Option<PathBuf>,
    /// Pre-supplied template variables (`--var key=value`, repeatable).
    pub vars: Vec<String>,
    /// Non-interactive — error on any missing value rather than prompt.
    pub no_input: bool,
    /// Allow writing into a non-empty target directory.
    pub force: bool,
}

/// Result returned by `run_new` — the binary path discards it; tests
/// inspect the fields. `#[allow(dead_code)]` keeps the binary build
/// clean under `-D warnings`.
#[derive(Debug)]
#[allow(dead_code)]
pub struct ScaffoldOutcome {
    pub template_name: String,
    pub target_dir: PathBuf,
    pub written_files: Vec<PathBuf>,
}

/// Entry point. Generic over `R: BufRead` and `W: Write` so unit tests
/// can drive the prompt loop with `Cursor::new(b"...")` instead of stdin.
pub fn run_new<R: BufRead, W: Write>(
    mut reader: R,
    mut writer: W,
    args: NewArgs,
) -> Result<ScaffoldOutcome, String> {
    let templates_dir = &args.templates_dir;
    if !templates_dir.exists() {
        return Err(format!(
            "templates directory does not exist: {}",
            templates_dir.display()
        ));
    }

    // 1. Resolve template name.
    let template_name = match &args.template {
        Some(name) => {
            // Validate the name now so we fail fast with a clear error.
            templates::load_template(templates_dir, name)
                .map_err(|e| format!("template '{name}': {e}"))?;
            name.clone()
        }
        None => {
            if args.no_input {
                return Err(
                    "no template specified (positional arg required with --no-input)".into(),
                );
            }
            pick_template(&mut reader, &mut writer, templates_dir)?
        }
    };

    let manifest = templates::load_template(templates_dir, &template_name)
        .map_err(|e| format!("load template manifest: {e}"))?;

    // 2. Resolve target dir.
    let target_dir = match &args.target {
        Some(t) => t.clone(),
        None => {
            let default = PathBuf::from(format!("./{}", template_name));
            if args.no_input {
                default
            } else {
                prompt_target(&mut reader, &mut writer, &default)?
            }
        }
    };

    // 3. Parse pre-supplied vars from --var flags.
    let mut answers: BTreeMap<String, String> = BTreeMap::new();
    for raw in &args.vars {
        let (k, v) = raw
            .split_once('=')
            .ok_or_else(|| format!("invalid --var format: '{raw}' (expected key=value)"))?;
        answers.insert(k.trim().to_string(), v.to_string());
    }

    // 4. For each manifest arg, fill from --var, default, or prompt.
    for (name, spec) in &manifest.args {
        if answers.contains_key(name) {
            continue;
        }
        let default_str = spec.default.as_ref().map(json_value_to_string);
        if args.no_input {
            match default_str {
                Some(d) => {
                    answers.insert(name.clone(), d);
                }
                None => {
                    if spec.required {
                        return Err(format!(
                            "--no-input set but '{name}' has no default and was not supplied via --var"
                        ));
                    }
                    // Non-required, no default — skip silently.
                }
            }
        } else {
            let answer =
                prompt_variable(&mut reader, &mut writer, name, spec, default_str.as_deref())?;
            if !answer.is_empty() {
                answers.insert(name.clone(), answer);
            } else if spec.required {
                return Err(format!("required variable '{name}' was left blank"));
            }
        }
    }

    // 5. Confirm summary (interactive only).
    if !args.no_input {
        writeln!(writer, "\nSummary:").map_err(io_err)?;
        writeln!(writer, "  template: {}", template_name).map_err(io_err)?;
        writeln!(writer, "  target:   {}", target_dir.display()).map_err(io_err)?;
        for (k, v) in &answers {
            writeln!(writer, "  {k} = {v}").map_err(io_err)?;
        }
        write!(writer, "\nProceed? [Y/n]: ").map_err(io_err)?;
        writer.flush().map_err(io_err)?;
        let line = read_line(&mut reader)?;
        let trimmed = line.trim().to_lowercase();
        if !(trimmed.is_empty() || trimmed == "y" || trimmed == "yes") {
            return Err("aborted by user".into());
        }
    }

    // 6. Refuse to overwrite a non-empty target dir without --force.
    if target_dir.exists() {
        if !target_dir.is_dir() {
            return Err(format!(
                "target path exists and is not a directory: {}",
                target_dir.display()
            ));
        }
        let non_empty = fs::read_dir(&target_dir)
            .map_err(|e| format!("read target dir: {e}"))?
            .next()
            .is_some();
        if non_empty && !args.force {
            return Err(format!(
                "target directory is not empty: {} (use --force to overwrite)",
                target_dir.display()
            ));
        }
    } else {
        fs::create_dir_all(&target_dir).map_err(|e| format!("create target dir: {e}"))?;
    }

    // 7. Apply template.
    let result = templates::apply_template(templates_dir, &template_name, &answers)
        .map_err(|e| format!("apply template: {e}"))?;

    let out_path = target_dir.join(&result.output_file);
    fs::write(&out_path, &result.source).map_err(|e| format!("write output: {e}"))?;

    let written_files = vec![out_path.clone()];

    // 8. Print next-step hints.
    writeln!(
        writer,
        "\nOK: scaffolded {} at {}",
        template_name,
        target_dir.display()
    )
    .map_err(io_err)?;
    writeln!(writer, "\nNext steps:").map_err(io_err)?;
    writeln!(writer, "  cd {}", target_dir.display()).map_err(io_err)?;
    writeln!(
        writer,
        "  boruna run {} --policy allow-all",
        result.output_file
    )
    .map_err(io_err)?;
    if !result.dependencies.is_empty() {
        writeln!(writer, "  deps: {}", result.dependencies.join(", ")).map_err(io_err)?;
    }
    if !result.capabilities.is_empty() {
        writeln!(writer, "  caps: {}", result.capabilities.join(", ")).map_err(io_err)?;
    }

    Ok(ScaffoldOutcome {
        template_name,
        target_dir,
        written_files,
    })
}

fn pick_template<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    templates_dir: &Path,
) -> Result<String, String> {
    let templates =
        templates::list_templates(templates_dir).map_err(|e| format!("list templates: {e}"))?;
    if templates.is_empty() {
        return Err(format!("no templates found in {}", templates_dir.display()));
    }
    writeln!(writer, "Available templates:").map_err(io_err)?;
    for (i, t) in templates.iter().enumerate() {
        writeln!(writer, "  [{}] {} — {}", i + 1, t.name, t.description).map_err(io_err)?;
    }
    write!(writer, "Pick a template [1-{}]: ", templates.len()).map_err(io_err)?;
    writer.flush().map_err(io_err)?;
    let line = read_line(reader)?;
    let idx: usize = line
        .trim()
        .parse()
        .map_err(|_| format!("invalid selection: '{}'", line.trim()))?;
    if idx == 0 || idx > templates.len() {
        return Err(format!(
            "selection out of range: {} (expected 1..={})",
            idx,
            templates.len()
        ));
    }
    Ok(templates[idx - 1].name.clone())
}

fn prompt_target<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    default: &Path,
) -> Result<PathBuf, String> {
    write!(writer, "Target directory [{}]: ", default.display()).map_err(io_err)?;
    writer.flush().map_err(io_err)?;
    let line = read_line(reader)?;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        Ok(default.to_path_buf())
    } else {
        Ok(PathBuf::from(trimmed))
    }
}

fn prompt_variable<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    name: &str,
    spec: &templates::ArgSpec,
    default: Option<&str>,
) -> Result<String, String> {
    writeln!(writer, "\n{} ({})", name, spec.description).map_err(io_err)?;
    match default {
        Some(d) => write!(writer, "  value [{d}]: "),
        None if spec.required => write!(writer, "  value (required): "),
        None => write!(writer, "  value (optional): "),
    }
    .map_err(io_err)?;
    writer.flush().map_err(io_err)?;
    let line = read_line(reader)?;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        if let Some(d) = default {
            return Ok(d.to_string());
        }
        return Ok(String::new());
    }
    Ok(trimmed.to_string())
}

fn read_line<R: BufRead>(reader: &mut R) -> Result<String, String> {
    let mut buf = String::new();
    reader
        .read_line(&mut buf)
        .map_err(|e| format!("read input: {e}"))?;
    Ok(buf)
}

fn json_value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn io_err(e: std::io::Error) -> String {
    format!("write output: {e}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn make_templates_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        // Template with a default.
        let with_default = dir.path().join("widget");
        fs::create_dir_all(&with_default).unwrap();
        let manifest = serde_json::json!({
            "name": "widget",
            "version": "0.1.0",
            "description": "Widget demo",
            "dependencies": ["std.ui"],
            "capabilities": [],
            "args": {
                "entity_name": {
                    "type": "string",
                    "required": true,
                    "description": "Entity name",
                    "default": "things"
                },
                "fields": {
                    "type": "string",
                    "required": true,
                    "description": "Comma-separated fields",
                    "default": "name|price"
                }
            }
        });
        fs::write(
            with_default.join("template.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();
        fs::write(
            with_default.join("app.ax.template"),
            "// {{entity_name}} fields: {{fields}}\nfn main() -> Int { 0 }\n",
        )
        .unwrap();

        // Template without defaults, used for the no-input failure case.
        let no_default = dir.path().join("strict");
        fs::create_dir_all(&no_default).unwrap();
        let manifest2 = serde_json::json!({
            "name": "strict",
            "version": "0.1.0",
            "description": "Strict, no defaults",
            "dependencies": [],
            "capabilities": [],
            "args": {
                "name": {
                    "type": "string",
                    "required": true,
                    "description": "Required name"
                }
            }
        });
        fs::write(
            no_default.join("template.json"),
            serde_json::to_string_pretty(&manifest2).unwrap(),
        )
        .unwrap();
        fs::write(no_default.join("app.ax.template"), "// {{name}}\n").unwrap();
        dir
    }

    #[test]
    fn scaffold_with_all_args_no_input_succeeds() {
        let templates_dir = make_templates_dir();
        let target = tempfile::tempdir().unwrap();
        let target_path = target.path().join("out");

        let args = NewArgs {
            template: Some("widget".into()),
            templates_dir: templates_dir.path().to_path_buf(),
            target: Some(target_path.clone()),
            vars: vec!["entity_name=widgets".into(), "fields=a|b".into()],
            no_input: true,
            force: false,
        };
        let mut out = Vec::new();
        let outcome =
            run_new(Cursor::new(Vec::<u8>::new()), &mut out, args).expect("scaffold succeeds");
        assert_eq!(outcome.template_name, "widget");
        assert_eq!(outcome.written_files.len(), 1);
        let body = fs::read_to_string(&outcome.written_files[0]).unwrap();
        assert!(body.contains("widgets fields: a|b"), "body was: {body}");
    }

    #[test]
    fn scaffold_prompts_for_missing_variables() {
        let templates_dir = make_templates_dir();
        let target = tempfile::tempdir().unwrap();
        let target_path = target.path().join("out");

        // Lines: entity override, fields override, confirm.
        let stdin = b"orders\nname|qty\ny\n";
        let args = NewArgs {
            template: Some("widget".into()),
            templates_dir: templates_dir.path().to_path_buf(),
            target: Some(target_path.clone()),
            vars: vec![],
            no_input: false,
            force: false,
        };
        let mut out = Vec::new();
        let outcome = run_new(Cursor::new(stdin), &mut out, args).expect("scaffold succeeds");
        let body = fs::read_to_string(&outcome.written_files[0]).unwrap();
        assert!(body.contains("orders fields: name|qty"), "body: {body}");
        let printed = String::from_utf8(out).unwrap();
        assert!(printed.contains("entity_name"), "missing prompt: {printed}");
        assert!(printed.contains("fields"), "missing prompt: {printed}");
        assert!(printed.contains("Summary:"), "missing summary: {printed}");
    }

    #[test]
    fn scaffold_refuses_overwrite_without_force() {
        let templates_dir = make_templates_dir();
        let target = tempfile::tempdir().unwrap();
        let target_path = target.path().join("out");
        fs::create_dir_all(&target_path).unwrap();
        fs::write(target_path.join("existing.txt"), "hi").unwrap();

        let args = NewArgs {
            template: Some("widget".into()),
            templates_dir: templates_dir.path().to_path_buf(),
            target: Some(target_path.clone()),
            vars: vec!["entity_name=x".into(), "fields=y".into()],
            no_input: true,
            force: false,
        };
        let mut out = Vec::new();
        let err = run_new(Cursor::new(Vec::<u8>::new()), &mut out, args).unwrap_err();
        assert!(err.contains("not empty"), "err: {err}");
    }

    #[test]
    fn scaffold_force_overwrites_target() {
        let templates_dir = make_templates_dir();
        let target = tempfile::tempdir().unwrap();
        let target_path = target.path().join("out");
        fs::create_dir_all(&target_path).unwrap();
        fs::write(target_path.join("existing.txt"), "hi").unwrap();

        let args = NewArgs {
            template: Some("widget".into()),
            templates_dir: templates_dir.path().to_path_buf(),
            target: Some(target_path.clone()),
            vars: vec!["entity_name=x".into(), "fields=y".into()],
            no_input: true,
            force: true,
        };
        let mut out = Vec::new();
        let outcome =
            run_new(Cursor::new(Vec::<u8>::new()), &mut out, args).expect("force overwrites");
        assert!(outcome.written_files[0].exists());
        // Existing file untouched (we only write the template output).
        assert!(target_path.join("existing.txt").exists());
    }

    #[test]
    fn scaffold_no_input_errors_on_missing_default() {
        let templates_dir = make_templates_dir();
        let target = tempfile::tempdir().unwrap();
        let target_path = target.path().join("out");

        let args = NewArgs {
            template: Some("strict".into()),
            templates_dir: templates_dir.path().to_path_buf(),
            target: Some(target_path),
            vars: vec![],
            no_input: true,
            force: false,
        };
        let mut out = Vec::new();
        let err = run_new(Cursor::new(Vec::<u8>::new()), &mut out, args).unwrap_err();
        assert!(
            err.contains("--no-input") && err.contains("'name'"),
            "err: {err}"
        );
    }

    #[test]
    fn scaffold_invalid_template_name_errors_clearly() {
        let templates_dir = make_templates_dir();
        let target = tempfile::tempdir().unwrap();
        let target_path = target.path().join("out");

        let args = NewArgs {
            template: Some("does-not-exist".into()),
            templates_dir: templates_dir.path().to_path_buf(),
            target: Some(target_path),
            vars: vec![],
            no_input: true,
            force: false,
        };
        let mut out = Vec::new();
        let err = run_new(Cursor::new(Vec::<u8>::new()), &mut out, args).unwrap_err();
        assert!(
            err.contains("does-not-exist") && err.contains("not found"),
            "err: {err}"
        );
    }

    #[test]
    fn scaffold_uses_default_when_user_presses_enter() {
        let templates_dir = make_templates_dir();
        let target = tempfile::tempdir().unwrap();
        let target_path = target.path().join("out");

        // Empty entity_name → default "things"; empty fields → default
        // "name|price"; confirm with empty (Y default).
        let stdin = b"\n\n\n";
        let args = NewArgs {
            template: Some("widget".into()),
            templates_dir: templates_dir.path().to_path_buf(),
            target: Some(target_path),
            vars: vec![],
            no_input: false,
            force: false,
        };
        let mut out = Vec::new();
        let outcome = run_new(Cursor::new(stdin), &mut out, args).expect("scaffold succeeds");
        let body = fs::read_to_string(&outcome.written_files[0]).unwrap();
        assert!(body.contains("things fields: name|price"), "body: {body}");
    }
}
