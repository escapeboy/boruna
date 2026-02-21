use boruna_compiler::ast::*;

use super::*;

/// Enhance a compiler diagnostic with suggestions based on AST analysis.
pub fn enhance_compiler_diagnostic(
    diag: &mut Diagnostic,
    file: &str,
    source: &str,
    program: &Program,
) {
    if diag.id.as_str() == E003_UNDEFINED_VAR {
        enhance_undefined_var(diag, file, source, program);
    }
}

/// For "undefined variable: X", suggest the closest defined name.
fn enhance_undefined_var(diag: &mut Diagnostic, file: &str, source: &str, program: &Program) {
    let name = diag
        .message
        .strip_prefix("undefined variable: ")
        .unwrap_or("")
        .to_string();
    if name.is_empty() {
        return;
    }

    // Collect all defined names (functions, params, locals, types)
    let mut defined_owned: Vec<String> = Vec::new();
    for item in &program.items {
        match item {
            Item::Function(f) => {
                defined_owned.push(f.name.clone());
                for p in &f.params {
                    defined_owned.push(p.name.clone());
                }
                // Collect local variables from let bindings
                collect_locals_from_block(&f.body, &mut defined_owned);
            }
            Item::TypeDef(t) => {
                defined_owned.push(t.name.clone());
            }
            _ => {}
        }
    }

    // Also add builtins
    for b in &[
        "list_len",
        "list_get",
        "list_push",
        "parse_int",
        "try_parse_int",
        "str_contains",
        "str_starts_with",
    ] {
        defined_owned.push(b.to_string());
    }

    let defined: Vec<&str> = defined_owned.iter().map(|s| s.as_str()).collect();

    if let Some(suggestion) = find_closest_name(&name, &defined) {
        let line = diag.location.as_ref().map(|l| l.line);
        if let Some(patch) = suggest_rename_identifier(file, source, &name, suggestion, line) {
            diag.suggested_patches.push(patch);
        }
    }
}

/// Recursively collect all local variable names from a block.
fn collect_locals_from_block(block: &Block, names: &mut Vec<String>) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let { name, value, .. } => {
                names.push(name.clone());
                collect_locals_from_expr(value, names);
            }
            Stmt::Assign { value, .. } => collect_locals_from_expr(value, names),
            Stmt::Expr(e) | Stmt::Return(Some(e)) => collect_locals_from_expr(e, names),
            Stmt::While { body, .. } => collect_locals_from_block(body, names),
            _ => {}
        }
    }
}

fn collect_locals_from_expr(expr: &Expr, names: &mut Vec<String>) {
    match expr {
        Expr::If {
            then_block,
            else_block,
            ..
        } => {
            collect_locals_from_block(then_block, names);
            if let Some(eb) = else_block {
                collect_locals_from_block(eb, names);
            }
        }
        Expr::Match { arms, .. } => {
            for arm in arms {
                if let Pattern::Ident(n) = &arm.pattern {
                    names.push(n.clone());
                }
                collect_locals_from_expr(&arm.body, names);
            }
        }
        Expr::Block(b) => collect_locals_from_block(b, names),
        _ => {}
    }
}

/// Find the closest name by edit distance (Levenshtein).
/// Returns None if no name is close enough (distance > max(2, name.len()/2)).
pub fn find_closest_name<'a>(target: &str, candidates: &[&'a str]) -> Option<&'a str> {
    let max_distance = std::cmp::max(2, target.len() / 2);
    let mut best: Option<(&str, usize)> = None;

    for &candidate in candidates {
        if candidate == target {
            continue;
        }
        let dist = levenshtein(target, candidate);
        if dist <= max_distance && best.is_none_or(|(_, d)| dist < d) {
            best = Some((candidate, dist));
        }
    }

    best.map(|(name, _)| name)
}

/// Suggest renaming an identifier in the source.
pub fn suggest_rename_identifier(
    file: &str,
    source: &str,
    old_name: &str,
    new_name: &str,
    hint_line: Option<usize>,
) -> Option<SuggestedPatch> {
    let line_num = hint_line.or_else(|| find_usage_line(source, old_name))?;

    let lines: Vec<&str> = source.lines().collect();
    if line_num == 0 || line_num > lines.len() {
        return None;
    }

    let old_line = lines[line_num - 1];
    let new_line = replace_word(old_line, old_name, new_name);

    Some(SuggestedPatch {
        id: format!("{}-rename-{}", E003_UNDEFINED_VAR, old_name),
        description: format!("rename '{}' to '{}'", old_name, new_name),
        confidence: Confidence::Medium,
        rationale: format!(
            "'{}' is not defined; did you mean '{}'? (edit distance: {})",
            old_name,
            new_name,
            levenshtein(old_name, new_name),
        ),
        edits: vec![TextEdit {
            file: file.to_string(),
            start_line: line_num,
            old_text: old_line.to_string(),
            new_text: new_line,
        }],
    })
}

/// Suggest renaming a record field.
pub fn suggest_rename_field(
    file: &str,
    source: &str,
    old_field: &str,
    new_field: &str,
    hint_line: Option<usize>,
) -> Option<SuggestedPatch> {
    let line_num = hint_line.or_else(|| find_usage_line(source, old_field))?;

    let lines: Vec<&str> = source.lines().collect();
    if line_num == 0 || line_num > lines.len() {
        return None;
    }

    let old_line = lines[line_num - 1];
    // Replace "old_field:" with "new_field:"
    let new_line = old_line.replace(&format!("{old_field}:"), &format!("{new_field}:"));

    Some(SuggestedPatch {
        id: format!("{}-rename-{}", E006_UNKNOWN_FIELD, old_field),
        description: format!("rename field '{}' to '{}'", old_field, new_field),
        confidence: Confidence::High,
        rationale: format!(
            "field '{}' does not exist; closest match is '{}'",
            old_field, new_field,
        ),
        edits: vec![TextEdit {
            file: file.to_string(),
            start_line: line_num,
            old_text: old_line.to_string(),
            new_text: new_line,
        }],
    })
}

/// Suggest adding missing match arms.
pub fn suggest_missing_match_arms(
    file: &str,
    source: &str,
    match_start: usize,
    match_end: usize,
    missing: &[&str],
    variants: &[(String, Option<TypeExpr>)],
) -> Option<SuggestedPatch> {
    let lines: Vec<&str> = source.lines().collect();
    if match_end == 0 || match_end > lines.len() {
        return None;
    }

    // The closing brace of the match is at match_end
    let closing_line = lines[match_end - 1];

    // Determine indentation from existing arms
    let arm_indent = detect_arm_indent(source, match_start, match_end);

    // Build new arms
    let mut new_arms = String::new();
    for missing_name in missing {
        // Check if the variant has a payload
        let has_payload = variants
            .iter()
            .find(|(n, _)| n == *missing_name)
            .is_some_and(|(_, payload)| payload.is_some());

        if has_payload {
            new_arms.push_str(&format!(
                "{}{}(_) => {{ /* TODO */ }}\n",
                arm_indent, missing_name,
            ));
        } else {
            new_arms.push_str(&format!(
                "{}{} => {{ /* TODO */ }}\n",
                arm_indent, missing_name,
            ));
        }
    }

    // The edit: insert new arms before the closing brace
    let old_text = closing_line.to_string();
    let new_text = format!("{}{}", new_arms, closing_line);

    Some(SuggestedPatch {
        id: format!("{}-add-arms", E005_NON_EXHAUSTIVE_MATCH),
        description: format!("add missing match arms: {}", missing.join(", ")),
        confidence: Confidence::High,
        rationale: format!(
            "match expression does not cover variants: {}; adding stub arms with TODO markers",
            missing.join(", "),
        ),
        edits: vec![TextEdit {
            file: file.to_string(),
            start_line: match_end,
            old_text,
            new_text,
        }],
    })
}

/// Suggest removing capability annotations from a function.
pub fn suggest_remove_capabilities(
    file: &str,
    source: &str,
    fn_line: usize,
    capabilities: &[String],
) -> Option<SuggestedPatch> {
    let lines: Vec<&str> = source.lines().collect();
    if fn_line == 0 || fn_line > lines.len() {
        return None;
    }

    let old_line = lines[fn_line - 1];
    // Remove the !{...} annotation
    let new_line = remove_capability_annotation(old_line);

    Some(SuggestedPatch {
        id: format!("{}-remove-caps", E007_CAPABILITY_VIOLATION),
        description: format!(
            "remove capability annotation !{{{}}}",
            capabilities.join(", ")
        ),
        confidence: Confidence::High,
        rationale: "update() and view() must be pure functions with no capabilities".to_string(),
        edits: vec![TextEdit {
            file: file.to_string(),
            start_line: fn_line,
            old_text: old_line.to_string(),
            new_text: new_line,
        }],
    })
}

/// Remove `!{...}` from a function definition line.
fn remove_capability_annotation(line: &str) -> String {
    // Find !{ and matching }
    if let Some(start) = line.find("!{") {
        if let Some(end) = line[start..].find('}') {
            let before = line[..start].trim_end();
            let after = &line[start + end + 1..];
            return format!("{before}{after}");
        }
    }
    line.to_string()
}

/// Detect the indentation used for match arms.
fn detect_arm_indent(source: &str, start_line: usize, end_line: usize) -> String {
    let lines: Vec<&str> = source.lines().collect();
    for line in lines
        .iter()
        .take((end_line - 1).min(lines.len()))
        .skip(start_line)
    {
        if line.contains("=>") {
            let indent_len = line.len() - line.trim_start().len();
            return " ".repeat(indent_len);
        }
    }
    "        ".to_string() // default: 8 spaces
}

/// Find the first line where a name is used (not defined).
fn find_usage_line(source: &str, name: &str) -> Option<usize> {
    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") {
            continue;
        }
        if trimmed.starts_with("fn ")
            || trimmed.starts_with("type ")
            || trimmed.starts_with("enum ")
        {
            continue;
        }
        if line.contains(name) {
            return Some(i + 1);
        }
    }
    None
}

/// Replace a word in a line (word-boundary aware).
fn replace_word(line: &str, old: &str, new: &str) -> String {
    let mut result = String::new();
    let mut remaining = line;

    while let Some(pos) = remaining.find(old) {
        let before_ok = pos == 0
            || !remaining.as_bytes()[pos - 1].is_ascii_alphanumeric()
                && remaining.as_bytes()[pos - 1] != b'_';
        let after_pos = pos + old.len();
        let after_ok = after_pos >= remaining.len()
            || !remaining.as_bytes()[after_pos].is_ascii_alphanumeric()
                && remaining.as_bytes()[after_pos] != b'_';

        if before_ok && after_ok {
            result.push_str(&remaining[..pos]);
            result.push_str(new);
            remaining = &remaining[pos + old.len()..];
        } else {
            result.push_str(&remaining[..pos + old.len()]);
            remaining = &remaining[pos + old.len()..];
        }
    }
    result.push_str(remaining);
    result
}

/// Levenshtein edit distance.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a_len = a.len();
    let b_len = b.len();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut prev: Vec<usize> = (0..=b_len).collect();
    let mut curr = vec![0; b_len + 1];

    for (i, ca) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.chars().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            curr[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(curr[j] + 1);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b_len]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_levenshtein() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("abc", "abd"), 1);
        assert_eq!(levenshtein("abc", "abcd"), 1);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
    }

    #[test]
    fn test_find_closest_name() {
        let candidates = &["count", "name", "value", "status"];
        assert_eq!(find_closest_name("countt", candidates), Some("count"));
        assert_eq!(find_closest_name("coun", candidates), Some("count"));
        assert_eq!(
            find_closest_name("xyz_completely_different", candidates),
            None
        );
    }

    #[test]
    fn test_replace_word() {
        assert_eq!(
            replace_word("let x = countt + 1", "countt", "count"),
            "let x = count + 1"
        );
        assert_eq!(replace_word("foobar", "foo", "baz"), "foobar"); // not a word boundary
    }

    #[test]
    fn test_remove_capability_annotation() {
        assert_eq!(
            remove_capability_annotation("fn update(s: State, m: Msg) -> State !{fs.read} {"),
            "fn update(s: State, m: Msg) -> State {"
        );
        assert_eq!(
            remove_capability_annotation("fn pure() -> Int {"),
            "fn pure() -> Int {"
        );
    }

    #[test]
    fn test_suggest_missing_match_arms() {
        let source = "\
fn update(state: State, action: Action) -> State {
    match action {
        Add => state
    }
}
";
        let variants = vec![
            ("Add".to_string(), None),
            ("Remove".to_string(), None),
            (
                "Clear".to_string(),
                Some(TypeExpr::Named("String".to_string())),
            ),
        ];
        let patch =
            suggest_missing_match_arms("test.ax", source, 2, 4, &["Clear", "Remove"], &variants);
        assert!(patch.is_some());
        let p = patch.unwrap();
        assert!(p.edits[0].new_text.contains("Remove => { /* TODO */ }"));
        assert!(p.edits[0].new_text.contains("Clear(_) => { /* TODO */ }"));
    }

    #[test]
    fn test_suggest_rename_field() {
        let source = "    State { countt: 0, name: \"test\" }\n";
        let patch = suggest_rename_field("test.ax", source, "countt", "count", Some(1));
        assert!(patch.is_some());
        let p = patch.unwrap();
        assert!(p.edits[0].new_text.contains("count:"));
        assert!(!p.edits[0].new_text.contains("countt:"));
    }
}
