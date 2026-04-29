use std::collections::HashMap;
use std::sync::Mutex;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

struct BurunaLanguageServer {
    client: Client,
    // Tracks the latest text for each open document URI.
    docs: Mutex<HashMap<String, String>>,
}

impl BurunaLanguageServer {
    fn new(client: Client) -> Self {
        BurunaLanguageServer {
            client,
            docs: Mutex::new(HashMap::new()),
        }
    }

    fn store(&self, uri: &Url, text: String) {
        if let Ok(mut map) = self.docs.lock() {
            map.insert(uri.to_string(), text);
        }
    }

    fn load(&self, uri: &Url) -> Option<String> {
        self.docs
            .lock()
            .ok()
            .and_then(|m| m.get(&uri.to_string()).cloned())
    }

    async fn analyze_document(&self, uri: Url, text: String) {
        let diagnostics = compute_diagnostics(&text);
        self.store(&uri, text);
        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;
    }
}

fn compute_diagnostics(text: &str) -> Vec<Diagnostic> {
    match boruna_compiler::compile("lsp", text) {
        Err(err) => {
            let (line, col, message) = match err {
                boruna_compiler::CompileError::Lexer { line, col, msg } => {
                    (line.saturating_sub(1), col.saturating_sub(1), msg)
                }
                boruna_compiler::CompileError::Parse { line, msg } => {
                    (line.saturating_sub(1), 0, msg)
                }
                boruna_compiler::CompileError::Type(msg) => (0, 0, msg),
                boruna_compiler::CompileError::Codegen(msg) => (0, 0, msg),
            };
            let pos = Position::new(line as u32, col as u32);
            vec![Diagnostic {
                range: Range::new(pos, pos),
                severity: Some(DiagnosticSeverity::ERROR),
                message,
                source: Some("boruna-lsp".into()),
                ..Default::default()
            }]
        }
        Ok(_) => {
            // Run AST analyzer for semantic diagnostics.
            let tokens = match boruna_compiler::lexer::lex(text) {
                Ok(t) => t,
                Err(_) => return vec![],
            };
            let program = match boruna_compiler::parser::parse(tokens) {
                Ok(p) => p,
                Err(_) => return vec![],
            };
            let analyzer =
                boruna_tooling::diagnostics::analyzer::Analyzer::new("lsp", text, &program);
            analyzer
                .analyze()
                .into_iter()
                .map(|d| {
                    let severity = match d.severity {
                        boruna_tooling::diagnostics::Severity::Error => DiagnosticSeverity::ERROR,
                        boruna_tooling::diagnostics::Severity::Warning => {
                            DiagnosticSeverity::WARNING
                        }
                        boruna_tooling::diagnostics::Severity::Info => {
                            DiagnosticSeverity::INFORMATION
                        }
                        boruna_tooling::diagnostics::Severity::Hint => DiagnosticSeverity::HINT,
                    };
                    let (line, col) = d
                        .location
                        .as_ref()
                        .map(|loc| {
                            (
                                loc.line.saturating_sub(1),
                                loc.col.unwrap_or(1).saturating_sub(1),
                            )
                        })
                        .unwrap_or((0, 0));
                    let pos = Position::new(line as u32, col as u32);
                    Diagnostic {
                        range: Range::new(pos, pos),
                        severity: Some(severity),
                        message: d.message,
                        source: Some("boruna-lsp".into()),
                        ..Default::default()
                    }
                })
                .collect()
        }
    }
}

fn completion_items(text: &str) -> Vec<CompletionItem> {
    let keywords = [
        "fn", "let", "if", "else", "match", "type", "enum", "import", "return", "true", "false",
        "mut", "while", "for", "in", "export", "module",
    ];
    let builtin_types = [
        "Int", "Float", "String", "Bool", "Unit", "Option", "Result", "List", "Map",
    ];
    let builtin_fns = [
        "parse_int",
        "parse_float",
        "to_string",
        "to_int",
        "to_float",
        "len",
        "append",
        "concat",
        "head",
        "tail",
        "map",
        "filter",
        "fold",
        "get",
        "set",
        "keys",
        "values",
        "contains",
        "print",
        "println",
    ];

    let mut items: Vec<CompletionItem> = keywords
        .iter()
        .map(|kw| CompletionItem {
            label: kw.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            ..Default::default()
        })
        .chain(builtin_types.iter().map(|t| CompletionItem {
            label: t.to_string(),
            kind: Some(CompletionItemKind::CLASS),
            ..Default::default()
        }))
        .chain(builtin_fns.iter().map(|f| CompletionItem {
            label: f.to_string(),
            kind: Some(CompletionItemKind::FUNCTION),
            ..Default::default()
        }))
        .collect();

    // Add user-defined functions from the document.
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("fn ") {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                items.push(CompletionItem {
                    label: name,
                    kind: Some(CompletionItemKind::FUNCTION),
                    ..Default::default()
                });
            }
        }
    }

    items
}

fn hover_info(text: &str, word: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("fn ") {
            let fn_name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if fn_name == word {
                let sig = trimmed
                    .find('{')
                    .map(|i| trimmed[..i].trim_end())
                    .unwrap_or(trimmed);
                return Some(format!("```ax\n{sig}\n```"));
            }
        }
    }
    None
}

/// Extract the word at a given position in a text document.
fn word_at(text: &str, pos: Position) -> String {
    let line_idx = pos.line as usize;
    let col_idx = pos.character as usize;
    let line = text.lines().nth(line_idx).unwrap_or("");
    let chars: Vec<char> = line.chars().collect();
    let start = (0..col_idx.min(chars.len()))
        .rev()
        .find(|&i| !chars[i].is_alphanumeric() && chars[i] != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    let end = (col_idx..chars.len())
        .find(|&i| !chars[i].is_alphanumeric() && chars[i] != '_')
        .unwrap_or(chars.len());
    chars[start..end].iter().collect()
}

/// Build LSP TextEdits that replace the entire document with `new_text`.
fn full_document_edit(old_text: &str, new_text: String) -> Vec<TextEdit> {
    let line_count = old_text.lines().count() as u32;
    let last_col = old_text.lines().last().map(|l| l.len() as u32).unwrap_or(0);
    vec![TextEdit {
        range: Range::new(Position::new(0, 0), Position::new(line_count, last_col)),
        new_text,
    }]
}

#[tower_lsp::async_trait]
impl LanguageServer for BurunaLanguageServer {
    async fn initialize(&self, _params: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![".".into()]),
                    ..Default::default()
                }),
                document_formatting_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "boruna-lsp".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "boruna-lsp initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        self.analyze_document(uri, text).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        if let Some(change) = params.content_changes.into_iter().last() {
            self.analyze_document(uri, change.text).await;
        }
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = &params.text_document_position.text_document.uri;
        let text = self.load(uri).unwrap_or_default();
        let items = completion_items(&text);
        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let uri = &params.text_document.uri;
        let text = match self.load(uri) {
            Some(t) => t,
            None => return Ok(Some(vec![])),
        };
        match boruna_tooling::format::format_source(&text) {
            Ok(formatted) => Ok(Some(full_document_edit(&text, formatted))),
            // Don't corrupt the document on parse failure.
            Err(_) => Ok(Some(vec![])),
        }
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let text = match self.load(uri) {
            Some(t) => t,
            None => return Ok(None),
        };
        let word = word_at(&text, pos);
        match hover_info(&text, &word) {
            Some(content) => Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: content,
                }),
                range: None,
            })),
            None => Ok(None),
        }
    }
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(BurunaLanguageServer::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_from_invalid_source() {
        let bad = "fn main( -> Int { 0 }";
        let diags = compute_diagnostics(bad);
        assert!(!diags.is_empty(), "expected at least one diagnostic");
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
    }

    #[test]
    fn diagnostics_from_valid_source() {
        let good = "fn main() -> Int {\n    42\n}\n";
        let diags = compute_diagnostics(good);
        assert!(
            diags.is_empty(),
            "expected zero diagnostics, got: {diags:?}"
        );
    }

    #[test]
    fn format_valid_source_roundtrip() {
        let src = "fn main() -> Int {\n42\n}\n";
        let formatted = boruna_tooling::format::format_source(src).unwrap();
        assert!(!formatted.is_empty());
        let again = boruna_tooling::format::format_source(&formatted).unwrap();
        assert_eq!(formatted, again, "formatting must be idempotent");
    }

    #[test]
    fn completion_includes_keywords() {
        let items = completion_items("");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"fn"), "missing keyword 'fn'");
        assert!(labels.contains(&"let"), "missing keyword 'let'");
        assert!(labels.contains(&"if"), "missing keyword 'if'");
    }

    #[test]
    fn completion_includes_user_functions() {
        let src = "fn my_helper() -> Int { 1 }\nfn main() -> Int { my_helper() }\n";
        let items = completion_items(src);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"my_helper"),
            "user-defined fn should appear in completions"
        );
    }

    #[test]
    fn hover_finds_function_signature() {
        let src = "fn add(a: Int, b: Int) -> Int {\n    a + b\n}\n";
        let result = hover_info(src, "add");
        assert!(result.is_some(), "expected hover result for 'add'");
        let content = result.unwrap();
        assert!(
            content.contains("add"),
            "hover should include function name"
        );
        assert!(content.contains("Int"), "hover should include type info");
    }

    #[test]
    fn hover_returns_none_for_unknown() {
        let src = "fn main() -> Int { 0 }\n";
        assert!(hover_info(src, "nonexistent").is_none());
    }
}
