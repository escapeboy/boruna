use boruna_compiler::ast::{FnDef, Item, Program, TypeDef, TypeDefKind, TypeExpr};
use boruna_compiler::CompileError;

use super::TOOL_RESPONSE_PROTOCOL_VERSION;

/// Extract top-level symbols (functions, records, enums) with their exact
/// declared signatures from `.ax` source.
///
/// Grounds LLM agents on real `.ax` APIs instead of hallucinated ones by
/// surfacing the exact parameter names/types, return type, declared
/// capability set, and requires/ensures arity for every top-level symbol.
///
/// Only lex + parse are needed: parameter, return, and field/variant types
/// are all syntactic annotations already present in the parsed AST, so no
/// type-checking pass is required to report exact declared types.
pub fn extract_symbols(source: &str) -> String {
    let tokens = match boruna_compiler::lexer::lex(source) {
        Ok(t) => t,
        Err(e) => return parse_error_json(&e),
    };

    let program: Program = match boruna_compiler::parser::parse(tokens) {
        Ok(p) => p,
        Err(e) => return parse_error_json(&e),
    };

    let symbols: Vec<serde_json::Value> = program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(f) => Some(fn_symbol(f)),
            Item::TypeDef(t) => Some(type_symbol(t)),
            // Imports and re-exports are not symbol *definitions*.
            Item::Import(_) | Item::Export(_) => None,
        })
        .collect();

    serde_json::json!({
        "success": true,
        "protocol_version": TOOL_RESPONSE_PROTOCOL_VERSION,
        "symbols": symbols,
    })
    .to_string()
}

fn fn_symbol(f: &FnDef) -> serde_json::Value {
    let params: Vec<serde_json::Value> = f
        .params
        .iter()
        .map(|p| {
            serde_json::json!({
                "name": p.name,
                "type": render_type(&p.ty),
            })
        })
        .collect();

    let return_type = f
        .return_type
        .as_ref()
        .map(render_type)
        .unwrap_or_else(|| "Unit".to_string());

    serde_json::json!({
        "kind": "fn",
        "name": f.name,
        "signature": fn_signature(f, &return_type),
        "params": params,
        "return_type": return_type,
        "capabilities": f.capabilities,
        "requires": f.requires.len(),
        "ensures": f.ensures.len(),
        "intent": f.intent,
        "exported": f.exported,
    })
}

fn fn_signature(f: &FnDef, return_type: &str) -> String {
    let params: Vec<String> = f
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, render_type(&p.ty)))
        .collect();
    let mut sig = format!("fn {}({}) -> {}", f.name, params.join(", "), return_type);
    if !f.capabilities.is_empty() {
        sig.push_str(&format!(" !{{{}}}", f.capabilities.join(", ")));
    }
    sig
}

fn type_symbol(t: &TypeDef) -> serde_json::Value {
    match &t.kind {
        TypeDefKind::Record(fields) => {
            let rendered: Vec<serde_json::Value> = fields
                .iter()
                .map(|(name, ty)| {
                    serde_json::json!({
                        "name": name,
                        "type": render_type(ty),
                    })
                })
                .collect();
            let field_sig: Vec<String> = fields
                .iter()
                .map(|(name, ty)| format!("{}: {}", name, render_type(ty)))
                .collect();
            serde_json::json!({
                "kind": "record",
                "name": t.name,
                "signature": format!("type {} {{ {} }}", t.name, field_sig.join(", ")),
                "fields": rendered,
                "capabilities": Vec::<String>::new(),
                "exported": t.exported,
            })
        }
        TypeDefKind::Enum(variants) => {
            let rendered: Vec<serde_json::Value> = variants
                .iter()
                .map(|(name, payload)| {
                    serde_json::json!({
                        "name": name,
                        "payload": payload.as_ref().map(render_type),
                    })
                })
                .collect();
            let variant_sig: Vec<String> = variants
                .iter()
                .map(|(name, payload)| match payload {
                    Some(ty) => format!("{}({})", name, render_type(ty)),
                    None => name.clone(),
                })
                .collect();
            serde_json::json!({
                "kind": "enum",
                "name": t.name,
                "signature": format!("enum {} {{ {} }}", t.name, variant_sig.join(", ")),
                "variants": rendered,
                "capabilities": Vec::<String>::new(),
                "exported": t.exported,
            })
        }
    }
}

/// Render a `TypeExpr` back to its `.ax` surface syntax.
fn render_type(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Named(n) => n.clone(),
        TypeExpr::Option(inner) => format!("Option<{}>", render_type(inner)),
        TypeExpr::Result(ok, err) => {
            format!("Result<{}, {}>", render_type(ok), render_type(err))
        }
        TypeExpr::List(inner) => format!("List<{}>", render_type(inner)),
        TypeExpr::Map(k, v) => format!("Map<{}, {}>", render_type(k), render_type(v)),
        TypeExpr::Fn(params, ret) => {
            let ps: Vec<String> = params.iter().map(render_type).collect();
            format!("({}) -> {}", ps.join(", "), render_type(ret))
        }
    }
}

fn parse_error_json(err: &CompileError) -> String {
    let (message, line, col) = match err {
        CompileError::Lexer { line, col, msg } => (msg.clone(), Some(*line), Some(*col)),
        CompileError::Parse { line, msg } => (msg.clone(), Some(*line), None),
        // extract_symbols only lexes + parses, so Type/Codegen cannot occur.
        CompileError::Type(msg) | CompileError::Codegen(msg) => (msg.clone(), None, None),
    };

    serde_json::json!({
        "success": false,
        "protocol_version": TOOL_RESPONSE_PROTOCOL_VERSION,
        "error_kind": "parse_error",
        "error": message,
        "line": line,
        "col": col,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn parse(json: &str) -> Value {
        serde_json::from_str(json).expect("valid JSON")
    }

    #[test]
    fn extracts_fn_record_and_enum() {
        let source = r#"
type User { name: String, age: Int }

enum Color { Red, Green, Custom(Int) }

fn fetch(url: String, retries: Int) -> Result<String, String> !{net.fetch} {
    Ok("body")
}

fn main() -> Int { 0 }
"#;
        let out = extract_symbols(source);
        let v = parse(&out);
        assert_eq!(v["success"], true);
        let syms = v["symbols"].as_array().expect("symbols array");
        assert_eq!(syms.len(), 4);

        // fetch fn
        let fetch = syms
            .iter()
            .find(|s| s["name"] == "fetch")
            .expect("fetch symbol");
        assert_eq!(fetch["kind"], "fn");
        assert_eq!(fetch["return_type"], "Result<String, String>");
        assert_eq!(fetch["capabilities"], serde_json::json!(["net.fetch"]));
        assert_eq!(fetch["params"][0]["name"], "url");
        assert_eq!(fetch["params"][0]["type"], "String");
        assert_eq!(fetch["params"][1]["name"], "retries");
        assert_eq!(fetch["params"][1]["type"], "Int");
        assert_eq!(
            fetch["signature"],
            "fn fetch(url: String, retries: Int) -> Result<String, String> !{net.fetch}"
        );

        // User record
        let user = syms
            .iter()
            .find(|s| s["name"] == "User")
            .expect("User symbol");
        assert_eq!(user["kind"], "record");
        assert_eq!(user["fields"][0]["name"], "name");
        assert_eq!(user["fields"][0]["type"], "String");
        assert_eq!(user["fields"][1]["name"], "age");
        assert_eq!(user["fields"][1]["type"], "Int");

        // Color enum
        let color = syms
            .iter()
            .find(|s| s["name"] == "Color")
            .expect("Color symbol");
        assert_eq!(color["kind"], "enum");
        let variants = color["variants"].as_array().expect("variants");
        assert_eq!(variants.len(), 3);
        assert_eq!(variants[0]["name"], "Red");
        assert!(variants[0]["payload"].is_null());
        assert_eq!(variants[2]["name"], "Custom");
        assert_eq!(variants[2]["payload"], "Int");
    }

    #[test]
    fn reports_requires_ensures_arity() {
        let source = r#"
fn withdraw(amount: Int) -> Int
    requires amount > 0
    ensures result >= 0
{
    amount
}
"#;
        let out = extract_symbols(source);
        let v = parse(&out);
        assert_eq!(v["success"], true);
        let withdraw = &v["symbols"][0];
        assert_eq!(withdraw["name"], "withdraw");
        assert_eq!(withdraw["requires"], 1);
        assert_eq!(withdraw["ensures"], 1);
    }

    #[test]
    fn parse_failure_reports_error_kind() {
        let out = extract_symbols("@@@ this is not valid .ax");
        let v = parse(&out);
        assert_eq!(v["success"], false);
        assert_eq!(v["error_kind"], "parse_error");
        assert!(v["error"].is_string());
    }
}
