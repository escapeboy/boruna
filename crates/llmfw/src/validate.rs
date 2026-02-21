use boruna_compiler::ast::{Item, Program};

use crate::error::FrameworkError;

/// Validates that a program conforms to the App protocol.
///
/// Required functions:
/// - `init()` — 0 params, returns State
/// - `update(state, msg)` — 2 params, no capabilities
/// - `view(state)` — 1 param, no capabilities
///
/// Optional:
/// - `policies()` — 0 params, no capabilities
pub struct AppValidator;

#[derive(Debug)]
pub struct ValidationResult {
    pub has_init: bool,
    pub has_update: bool,
    pub has_view: bool,
    pub has_policies: bool,
    pub state_type: Option<String>,
    pub message_type: Option<String>,
    pub errors: Vec<String>,
}

impl AppValidator {
    /// Validate a parsed program against the App protocol.
    pub fn validate(program: &Program) -> Result<ValidationResult, FrameworkError> {
        let mut result = ValidationResult {
            has_init: false,
            has_update: false,
            has_view: false,
            has_policies: false,
            state_type: None,
            message_type: None,
            errors: Vec::new(),
        };

        for item in &program.items {
            match item {
                Item::Function(f) => {
                    match f.name.as_str() {
                        "init" => {
                            result.has_init = true;
                            if !f.params.is_empty() {
                                result.errors.push(
                                    "init() must take 0 parameters".into()
                                );
                            }
                            // init may have capabilities (for initial setup)
                        }
                        "update" => {
                            result.has_update = true;
                            if f.params.len() != 2 {
                                result.errors.push(format!(
                                    "update() must take 2 parameters (state, msg), got {}",
                                    f.params.len()
                                ));
                            }
                            if !f.capabilities.is_empty() {
                                result.errors.push(
                                    "update() must be pure — no capability annotations allowed".into()
                                );
                            }
                        }
                        "view" => {
                            result.has_view = true;
                            if f.params.len() != 1 {
                                result.errors.push(format!(
                                    "view() must take 1 parameter (state), got {}",
                                    f.params.len()
                                ));
                            }
                            if !f.capabilities.is_empty() {
                                result.errors.push(
                                    "view() must be pure — no capability annotations allowed".into()
                                );
                            }
                        }
                        "policies" => {
                            result.has_policies = true;
                            if !f.params.is_empty() {
                                result.errors.push(
                                    "policies() must take 0 parameters".into()
                                );
                            }
                            if !f.capabilities.is_empty() {
                                result.errors.push(
                                    "policies() must be pure — no capability annotations allowed".into()
                                );
                            }
                        }
                        _ => {}
                    }
                }
                Item::TypeDef(t) => {
                    // Detect State and Message types by convention
                    if t.name.ends_with("State") || t.name == "State" {
                        result.state_type = Some(t.name.clone());
                    }
                    if t.name.ends_with("Msg") || t.name == "Msg" || t.name == "Message" {
                        result.message_type = Some(t.name.clone());
                    }
                }
                _ => {}
            }
        }

        if !result.has_init {
            result.errors.push("missing required function: init()".into());
        }
        if !result.has_update {
            result.errors.push("missing required function: update()".into());
        }
        if !result.has_view {
            result.errors.push("missing required function: view()".into());
        }

        if !result.errors.is_empty() {
            return Err(FrameworkError::Validation(
                result.errors.join("; ")
            ));
        }

        Ok(result)
    }

    /// Quick check — does this program conform to the App protocol?
    pub fn is_valid_app(program: &Program) -> bool {
        Self::validate(program).is_ok()
    }
}
