use boruna_compiler::CompileError;

const AST_SIZE_LIMIT: usize = 102_400; // 100 KB

/// Compile source and return JSON with module info or errors.
pub fn compile_source(source: &str, name: &str) -> String {
    match boruna_compiler::compile(name, source) {
        Ok(module) => {
            let info = serde_json::json!({
                "success": true,
                "module": {
                    "name": module.name,
                    "version": module.version,
                    "functions": module.functions.len(),
                    "types": module.types.len(),
                    "constants": module.constants.len(),
                    "entry": module.entry,
                }
            });
            serde_json::to_string_pretty(&info).unwrap_or_else(|_| "{}".into())
        }
        Err(e) => compile_error_json(&e),
    }
}

/// Parse source to AST and return JSON (truncated at 100KB).
pub fn parse_ast(source: &str) -> String {
    let tokens = match boruna_compiler::lexer::lex(source) {
        Ok(t) => t,
        Err(e) => {
            return compile_error_json(&e);
        }
    };

    let program = match boruna_compiler::parser::parse(tokens) {
        Ok(p) => p,
        Err(e) => {
            return compile_error_json(&e);
        }
    };

    let json = match serde_json::to_string_pretty(&program) {
        Ok(j) => j,
        Err(e) => {
            return serde_json::json!({
                "success": false,
                "error_kind": "serialization_error",
                "message": format!("{e}")
            })
            .to_string();
        }
    };

    if json.len() > AST_SIZE_LIMIT {
        let truncated = &json[..AST_SIZE_LIMIT];
        serde_json::json!({
            "success": true,
            "truncated": true,
            "ast_size": json.len(),
            "ast": truncated,
        })
        .to_string()
    } else {
        serde_json::json!({
            "success": true,
            "truncated": false,
            "ast": serde_json::from_str::<serde_json::Value>(&json).unwrap_or(serde_json::Value::Null),
        })
        .to_string()
    }
}

pub fn compile_error_json(err: &CompileError) -> String {
    let error = match err {
        CompileError::Lexer { line, col, msg } => {
            serde_json::json!({
                "severity": "error",
                "code": "E001",
                "message": msg,
                "line": line,
                "col": col,
            })
        }
        CompileError::Parse { line, msg } => {
            serde_json::json!({
                "severity": "error",
                "code": "E002",
                "message": msg,
                "line": line,
            })
        }
        CompileError::Type(msg) => {
            serde_json::json!({
                "severity": "error",
                "code": "E009",
                "message": msg,
            })
        }
        CompileError::Codegen(msg) => {
            serde_json::json!({
                "severity": "error",
                "code": "E008",
                "message": msg,
            })
        }
    };

    serde_json::json!({
        "success": false,
        "errors": [error],
    })
    .to_string()
}
