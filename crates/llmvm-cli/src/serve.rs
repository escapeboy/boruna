use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{Form, State};
use axum::response::Html;
use axum::routing::{get, post};
use axum::Router;

use boruna_bytecode::Value;
use boruna_compiler::ast::{BinOp, Block, Expr, Item, Pattern, Stmt};
use boruna_framework::runtime::AppMessage;
use boruna_framework::testing::TestHarness;

use crate::parse_message;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

struct AppContext {
    harness: TestHarness,
    source_path: PathBuf,
    message_tags: Vec<String>,
    state_fields: Vec<String>,
    cycle_log_display: Vec<CycleEntry>,
}

#[derive(Clone)]
struct CycleEntry {
    cycle: u64,
    tag: String,
    state_after: String,
    effects: Vec<String>,
}

type SharedState = Arc<Mutex<AppContext>>;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
pub async fn run_serve(file: PathBuf, port: u16) -> Result<(), Box<dyn std::error::Error>> {
    let ctx = build_context(&file)?;
    let shared: SharedState = Arc::new(Mutex::new(ctx));

    let app = Router::new()
        .route("/", get(handle_index))
        .route("/send", post(handle_send))
        .route("/reset", post(handle_reset))
        .route("/api/state", get(handle_api_state))
        .with_state(shared);

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    eprintln!("serving on http://{addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn build_context(file: &PathBuf) -> Result<AppContext, Box<dyn std::error::Error>> {
    let source = fs::read_to_string(file)?;
    let harness = TestHarness::from_source(&source)?;
    let message_tags = discover_message_tags(&source);
    let state_fields = discover_state_fields(&source);
    Ok(AppContext {
        harness,
        source_path: file.clone(),
        message_tags,
        state_fields,
        cycle_log_display: Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn handle_index(State(state): State<SharedState>) -> Html<String> {
    let ctx = state.lock().unwrap();
    Html(render_page(&ctx))
}

#[derive(serde::Deserialize)]
pub struct SendForm {
    tag: String,
    payload: Option<String>,
}

async fn handle_send(
    State(state): State<SharedState>,
    Form(form): Form<SendForm>,
) -> Html<String> {
    let mut ctx = state.lock().unwrap();

    let payload_str = form.payload.as_deref().unwrap_or("0");
    let (_, payload) = parse_message(&format!("_:{payload_str}"));
    let msg = AppMessage::new(&form.tag, payload);

    let entry = match ctx.harness.send(msg) {
        Ok((state_val, effects)) => {
            let effect_strs: Vec<String> = effects.iter()
                .map(|e| e.kind.as_str().to_string())
                .collect();
            let cycle = ctx.harness.cycle();
            let state_after = format_state(&state_val, &ctx.state_fields);
            CycleEntry { cycle, tag: form.tag.clone(), state_after, effects: effect_strs }
        }
        Err(e) => {
            let cycle = ctx.harness.cycle();
            CycleEntry { cycle, tag: format!("ERROR: {}", form.tag), state_after: format!("{e}"), effects: vec![] }
        }
    };
    ctx.cycle_log_display.push(entry);
    // Keep last 20 entries
    if ctx.cycle_log_display.len() > 20 {
        let start = ctx.cycle_log_display.len() - 20;
        ctx.cycle_log_display = ctx.cycle_log_display[start..].to_vec();
    }

    Html(render_page(&ctx))
}

async fn handle_reset(State(state): State<SharedState>) -> Html<String> {
    let mut ctx = state.lock().unwrap();
    match build_context(&ctx.source_path.clone()) {
        Ok(new_ctx) => *ctx = new_ctx,
        Err(e) => {
            ctx.cycle_log_display.push(CycleEntry {
                cycle: 0,
                tag: "RESET ERROR".into(),
                state_after: format!("{e}"),
                effects: vec![],
            });
        }
    }
    Html(render_page(&ctx))
}

async fn handle_api_state(State(state): State<SharedState>) -> axum::Json<serde_json::Value> {
    let ctx = state.lock().unwrap();
    axum::Json(serde_json::json!({
        "cycle": ctx.harness.cycle(),
        "state": format!("{}", ctx.harness.state()),
        "snapshot": ctx.harness.snapshot(),
    }))
}

// ---------------------------------------------------------------------------
// HTML rendering
// ---------------------------------------------------------------------------

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn format_state(val: &Value, field_names: &[String]) -> String {
    match val {
        Value::Record { fields, .. } => {
            let pairs: Vec<String> = fields.iter().enumerate().map(|(i, v)| {
                let label = field_names.get(i)
                    .cloned()
                    .unwrap_or_else(|| format!("[{i}]"));
                format!("{label}: {v}")
            }).collect();
            pairs.join(", ")
        }
        other => format!("{other}"),
    }
}

fn render_page(ctx: &AppContext) -> String {
    let state_html = render_state_table(ctx.harness.state(), &ctx.state_fields);

    let buttons_html: String = ctx.message_tags.iter().map(|tag| {
        format!(
            r#"<form method="post" action="/send" style="display:inline-block;margin:0 4px 4px 0">
  <input type="hidden" name="tag" value="{tag}">
  <input type="hidden" name="payload" value="0">
  <button type="submit">{tag}</button>
</form>"#,
            tag = escape_html(tag),
        )
    }).collect();

    let view_html = match ctx.harness.view() {
        Ok(v) => escape_html(&format!("{v}")),
        Err(e) => format!("<em>view error: {}</em>", escape_html(&e.to_string())),
    };

    let log_html: String = ctx.cycle_log_display.iter().rev().map(|entry| {
        let effects = if entry.effects.is_empty() {
            String::new()
        } else {
            format!(" <span class=\"effects\">[{}]</span>", escape_html(&entry.effects.join(", ")))
        };
        format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            entry.cycle,
            escape_html(&entry.tag),
            escape_html(&entry.state_after),
            effects,
        )
    }).collect();

    format!(r##"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<title>Boruna — {title}</title>
<style>
* {{ box-sizing: border-box; }}
body {{ font-family: system-ui, sans-serif; max-width: 800px; margin: 2rem auto; padding: 0 1rem; color: #1a1a1a; background: #fafafa; }}
h1 {{ font-size: 1.4rem; margin-bottom: 0.2rem; }}
.subtitle {{ color: #666; margin-bottom: 1.5rem; font-size: 0.9rem; }}
section {{ background: #fff; border: 1px solid #ddd; border-radius: 6px; padding: 1rem; margin-bottom: 1rem; }}
section h2 {{ font-size: 1rem; margin: 0 0 0.5rem 0; color: #333; }}
table {{ width: 100%; border-collapse: collapse; font-size: 0.9rem; }}
th, td {{ text-align: left; padding: 4px 8px; border-bottom: 1px solid #eee; }}
th {{ color: #666; font-weight: 600; }}
button {{ background: #2563eb; color: #fff; border: none; padding: 6px 16px; border-radius: 4px; cursor: pointer; font-size: 0.9rem; }}
button:hover {{ background: #1d4ed8; }}
button.secondary {{ background: #6b7280; }}
button.secondary:hover {{ background: #4b5563; }}
.custom-form {{ display: flex; gap: 6px; margin-top: 8px; }}
.custom-form input {{ padding: 6px 8px; border: 1px solid #ccc; border-radius: 4px; font-size: 0.9rem; }}
.custom-form input[name="tag"] {{ width: 120px; }}
.custom-form input[name="payload"] {{ width: 100px; }}
.effects {{ color: #9333ea; font-size: 0.85em; }}
pre {{ background: #f3f4f6; padding: 0.5rem; border-radius: 4px; overflow-x: auto; font-size: 0.85rem; }}
.actions {{ display: flex; gap: 8px; align-items: center; }}
</style>
</head>
<body>
<h1>Boruna Framework</h1>
<p class="subtitle">{file} &mdash; cycle {cycle}</p>

<section>
  <h2>State</h2>
  {state_html}
</section>

<section>
  <h2>Messages</h2>
  <div>{buttons_html}</div>
  <form method="post" action="/send" class="custom-form">
    <input type="text" name="tag" placeholder="tag" required>
    <input type="text" name="payload" placeholder="payload" value="0">
    <button type="submit">Send</button>
  </form>
</section>

<section>
  <h2>View</h2>
  <pre>{view_html}</pre>
</section>

<section>
  <h2>Cycle Log</h2>
  <table>
    <tr><th>#</th><th>Message</th><th>State After</th><th>Effects</th></tr>
    {log_html}
  </table>
</section>

<section>
  <div class="actions">
    <form method="post" action="/reset"><button type="submit" class="secondary">Reset (hot reload)</button></form>
  </div>
</section>
</body>
</html>"##,
        title = escape_html(&ctx.source_path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()),
        file = escape_html(&ctx.source_path.display().to_string()),
        cycle = ctx.harness.cycle(),
        state_html = state_html,
        buttons_html = buttons_html,
        view_html = view_html,
        log_html = log_html,
    )
}

fn render_state_table(val: &Value, field_names: &[String]) -> String {
    match val {
        Value::Record { fields, .. } => {
            let rows: String = fields.iter().enumerate().map(|(i, v)| {
                let label = field_names.get(i)
                    .cloned()
                    .unwrap_or_else(|| format!("[{i}]"));
                format!(
                    "<tr><th>{}</th><td>{}</td></tr>",
                    escape_html(&label),
                    escape_html(&format!("{v}")),
                )
            }).collect();
            format!("<table>{rows}</table>")
        }
        other => format!("<pre>{}</pre>", escape_html(&format!("{other}"))),
    }
}

// ---------------------------------------------------------------------------
// Message tag auto-discovery
// ---------------------------------------------------------------------------

fn discover_message_tags(source: &str) -> Vec<String> {
    let tokens = match boruna_compiler::lexer::lex(source) {
        Ok(t) => t,
        Err(_) => return vec![],
    };
    let program = match boruna_compiler::parser::parse(tokens) {
        Ok(p) => p,
        Err(_) => return vec![],
    };

    let mut tags = Vec::new();

    // Find the update() function
    for item in &program.items {
        if let Item::Function(f) = item {
            if f.name == "update" {
                collect_tags_from_block(&f.body, &mut tags);
            }
        }
    }

    tags.sort();
    tags.dedup();
    tags
}

fn collect_tags_from_block(block: &Block, tags: &mut Vec<String>) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let { value, .. } => collect_tags_from_expr(value, tags),
            Stmt::Assign { value, .. } => collect_tags_from_expr(value, tags),
            Stmt::Expr(e) => collect_tags_from_expr(e, tags),
            Stmt::Return(Some(e)) => collect_tags_from_expr(e, tags),
            Stmt::Return(None) => {}
            Stmt::While { condition, body } => {
                collect_tags_from_expr(condition, tags);
                collect_tags_from_block(body, tags);
            }
        }
    }
}

fn collect_tags_from_expr(expr: &Expr, tags: &mut Vec<String>) {
    match expr {
        // msg.tag == "literal" or "literal" == msg.tag
        Expr::Binary { op: BinOp::Eq, left, right } => {
            if is_msg_tag_access(left) {
                if let Expr::StringLit(s) = right.as_ref() {
                    tags.push(s.clone());
                }
            }
            if is_msg_tag_access(right) {
                if let Expr::StringLit(s) = left.as_ref() {
                    tags.push(s.clone());
                }
            }
            collect_tags_from_expr(left, tags);
            collect_tags_from_expr(right, tags);
        }

        // match msg.tag { "add" => ..., "remove" => ... }
        Expr::Match { value, arms } => {
            let is_tag_match = is_msg_tag_access(value);
            for arm in arms {
                if is_tag_match {
                    if let Pattern::StringLit(s) = &arm.pattern {
                        tags.push(s.clone());
                    }
                }
                collect_tags_from_expr(&arm.body, tags);
            }
            collect_tags_from_expr(value, tags);
        }

        Expr::If { condition, then_block, else_block } => {
            collect_tags_from_expr(condition, tags);
            collect_tags_from_block(then_block, tags);
            if let Some(eb) = else_block {
                collect_tags_from_block(eb, tags);
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_tags_from_expr(left, tags);
            collect_tags_from_expr(right, tags);
        }
        Expr::Unary { expr, .. } => collect_tags_from_expr(expr, tags),
        Expr::Call { func, args } => {
            collect_tags_from_expr(func, tags);
            for a in args { collect_tags_from_expr(a, tags); }
        }
        Expr::FieldAccess { object, .. } => collect_tags_from_expr(object, tags),
        Expr::Record { fields, spread, .. } => {
            for (_, e) in fields { collect_tags_from_expr(e, tags); }
            if let Some(s) = spread { collect_tags_from_expr(s, tags); }
        }
        Expr::List(items) => {
            for e in items { collect_tags_from_expr(e, tags); }
        }
        Expr::SomeExpr(e) | Expr::OkExpr(e) | Expr::ErrExpr(e) | Expr::Spawn(e) | Expr::Emit(e) => {
            collect_tags_from_expr(e, tags);
        }
        Expr::Send { target, message } => {
            collect_tags_from_expr(target, tags);
            collect_tags_from_expr(message, tags);
        }
        Expr::Block(b) => collect_tags_from_block(b, tags),
        _ => {}
    }
}

/// Returns true if the expression is `msg.tag` (or any ident ending in .tag).
fn is_msg_tag_access(expr: &Expr) -> bool {
    matches!(expr, Expr::FieldAccess { field, .. } if field == "tag")
}

// ---------------------------------------------------------------------------
// State field discovery
// ---------------------------------------------------------------------------

fn discover_state_fields(source: &str) -> Vec<String> {
    let tokens = match boruna_compiler::lexer::lex(source) {
        Ok(t) => t,
        Err(_) => return vec![],
    };
    let program = match boruna_compiler::parser::parse(tokens) {
        Ok(p) => p,
        Err(_) => return vec![],
    };

    for item in &program.items {
        if let Item::TypeDef(td) = item {
            if td.name == "State" {
                if let boruna_compiler::ast::TypeDefKind::Record(fields) = &td.kind {
                    return fields.iter().map(|(name, _)| name.clone()).collect();
                }
            }
        }
    }

    vec![]
}
