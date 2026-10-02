//! `boruna skills` — embedded, agent-curated documentation.
//!
//! Skill documents are compiled into the binary via `include_str!`, so an AI
//! agent can learn how to write `.ax` and drive the toolchain from the
//! installed `boruna` binary alone — no repository checkout required.

use serde::Serialize;

/// One embedded skill document.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Skill {
    /// Lookup name, e.g. `"ax-language"`.
    pub name: &'static str,
    /// One-line description shown by `skills list`.
    pub summary: &'static str,
    /// Full markdown body. Skipped in `list` output; served by `get`.
    #[serde(skip)]
    pub body: &'static str,
}

/// All embedded skill documents.
pub const SKILLS: &[Skill] = &[
    Skill {
        name: "ax-language",
        summary: "Syntax, types, and capabilities of the .ax language.",
        body: include_str!("skills/ax-language.md"),
    },
    Skill {
        name: "cli",
        summary: "The boruna CLI command surface, grouped by task.",
        body: include_str!("skills/cli.md"),
    },
    Skill {
        name: "workflows",
        summary: "Authoring DAG workflows and reading workflow output.",
        body: include_str!("skills/workflows.md"),
    },
    Skill {
        name: "diagnostics",
        summary: "Diagnostic codes and the check/repair loop for agents.",
        body: include_str!("skills/diagnostics.md"),
    },
];

/// Find a skill by exact name.
pub fn lookup(name: &str) -> Option<&'static Skill> {
    SKILLS.iter().find(|s| s.name == name)
}

/// Print the list of available skills.
pub fn run_list(json: bool) {
    if json {
        let payload = serde_json::json!({
            "version": 1,
            "skills": SKILLS,
        });
        match serde_json::to_string_pretty(&payload) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("failed to serialize skills: {e}"),
        }
    } else {
        println!("available skills (boruna skills get <name>):");
        for s in SKILLS {
            println!("  {:<14} {}", s.name, s.summary);
        }
    }
}

/// Print one skill document. Returns `false` if `name` is unknown.
pub fn run_get(name: &str, json: bool, root: &clap::Command) -> bool {
    let Some(skill) = lookup(name) else {
        let names: Vec<&str> = SKILLS.iter().map(|s| s.name).collect();
        eprintln!("unknown skill '{name}'. available: {}", names.join(", "));
        return false;
    };
    let body = render(skill, root);
    if json {
        let payload = serde_json::json!({
            "name": skill.name,
            "summary": skill.summary,
            "content": body,
        });
        match serde_json::to_string_pretty(&payload) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("failed to serialize skill: {e}"),
        }
    } else {
        print!("{body}");
        if !body.ends_with('\n') {
            println!();
        }
    }
    true
}

/// Full body of a skill as served to agents. The `cli` skill gets a command
/// reference generated from the clap tree appended, so it lists exactly the
/// commands and flags the installed binary has.
pub fn render(skill: &Skill, root: &clap::Command) -> String {
    let mut out = skill.body.to_string();
    if skill.name == "cli" {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&cli_reference(root));
    }
    out
}

/// Markdown reference of every (sub)command and its own flags, in declaration order.
pub fn cli_reference(root: &clap::Command) -> String {
    let mut out = String::from("\n## Command reference (generated from the installed binary)\n\n");
    for top in root.get_subcommands().filter(|s| s.get_name() != "help") {
        out.push_str(&format!("### {} {}\n\n", root.get_name(), top.get_name()));
        let mut path = vec![root.get_name().to_string()];
        walk_command(top, &mut path, &mut out);
        out.push('\n');
    }
    out
}

fn walk_command(sub: &clap::Command, path: &mut Vec<String>, out: &mut String) {
    path.push(sub.get_name().to_string());
    let mut line = format!("- `{}", path.join(" "));
    for arg in sub.get_arguments() {
        let id = arg.get_id().as_str();
        if id == "help" || id == "version" {
            continue;
        }
        if arg.is_positional() {
            line.push_str(&format!(" <{}>", id.to_uppercase()));
        } else if let Some(long) = arg.get_long() {
            line.push_str(&format!(" [--{long}]"));
        }
    }
    line.push('`');
    if let Some(about) = sub.get_about() {
        let first = about.to_string();
        let first = first.lines().next().unwrap_or("");
        line.push_str(&format!(" — {first}"));
    }
    out.push_str(&line);
    out.push('\n');
    for child in sub.get_subcommands().filter(|s| s.get_name() != "help") {
        walk_command(child, path, out);
    }
    path.pop();
}

/// Write every skill as `<dir>/<name>/SKILL.md` with agent-loadable frontmatter.
/// Returns the written paths. Overwrites existing files.
pub fn emit(
    dir: &std::path::Path,
    root: &clap::Command,
) -> std::io::Result<Vec<std::path::PathBuf>> {
    let mut written = Vec::new();
    for skill in SKILLS {
        let folder = dir.join(skill.name);
        std::fs::create_dir_all(&folder)?;
        let path = folder.join("SKILL.md");
        if path.is_symlink() {
            return Err(std::io::Error::other(format!(
                "{} is a symlink; refusing to write through it",
                path.display()
            )));
        }
        let doc = format!(
            "---\nname: boruna-{}\ndescription: {}\n---\n\n{}",
            skill.name,
            skill.summary,
            render(skill, root)
        );
        std::fs::write(&path, doc)?;
        written.push(path);
    }
    Ok(written)
}

/// One retrievable section of a skill.
#[derive(Debug, Clone, Serialize)]
pub struct Chunk {
    pub skill: &'static str,
    pub heading: String,
    pub text: String,
}

/// Split every rendered skill into `## `-headed chunks (fenced code is never split).
pub fn chunks(root: &clap::Command) -> Vec<Chunk> {
    let mut all = Vec::new();
    for skill in SKILLS {
        let body = render(skill, root);
        let mut heading = String::from("(intro)");
        let mut text = String::new();
        let mut in_fence = false;
        let mut flush = |heading: &str, text: &mut String| {
            if !text.trim().is_empty() {
                all.push(Chunk {
                    skill: skill.name,
                    heading: heading.to_string(),
                    text: std::mem::take(text),
                });
            }
            text.clear();
        };
        for line in body.lines() {
            if line.trim_start().starts_with("```") {
                in_fence = !in_fence;
            }
            if !in_fence {
                if let Some(h) = line
                    .strip_prefix("## ")
                    .or_else(|| line.strip_prefix("### "))
                {
                    flush(&heading, &mut text);
                    heading = h.trim().to_string();
                }
            }
            text.push_str(line);
            text.push('\n');
        }
        flush(&heading, &mut text);
    }
    all
}

fn words(s: &str) -> std::collections::BTreeSet<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3)
        .map(|w| w.to_lowercase())
        .collect()
}

/// Estimated token count (`chars / 4`, rounded up). An estimate, not a tokenizer.
pub fn estimate_tokens(s: &str) -> usize {
    s.chars().count().div_ceil(4)
}

/// Pick the chunks most relevant to `query` that fit in `budget` estimated tokens.
/// Deterministic: order is (score desc, skill name, position). Returns an empty
/// list when nothing matches. If the best chunk alone exceeds the budget it is
/// truncated at a line boundary so the caller always gets something.
pub fn pack(query: &str, budget: usize, root: &clap::Command) -> Vec<Chunk> {
    let q = words(query);
    let mut scored: Vec<(usize, usize, Chunk)> = chunks(root)
        .into_iter()
        .enumerate()
        .filter_map(|(pos, c)| {
            let body = words(&c.text);
            let head = words(&c.heading);
            let score: usize = q
                .iter()
                .map(|w| usize::from(body.contains(w)) + usize::from(head.contains(w)))
                .sum();
            (score > 0).then_some((score, pos, c))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.2.skill.cmp(b.2.skill))
            .then_with(|| a.1.cmp(&b.1))
    });

    let mut picked: Vec<Chunk> = Vec::new();
    let mut used = 0usize;
    for (_, _, c) in scored {
        let cost = estimate_tokens(&c.text);
        if used + cost <= budget {
            used += cost;
            picked.push(c);
        } else if picked.is_empty() {
            picked.push(truncate_chunk(c, budget));
            break;
        }
    }
    picked
}

fn truncate_chunk(mut c: Chunk, budget: usize) -> Chunk {
    let max_chars = budget.saturating_mul(4).max(1);
    let mut kept = String::new();
    for line in c.text.lines() {
        if kept.chars().count() + line.chars().count() + 1 > max_chars {
            break;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    if kept.is_empty() {
        kept = c.text.chars().take(max_chars).collect();
    }
    c.text = kept;
    c
}

/// CLI entry for `skills emit`. Returns `false` on failure.
pub fn run_emit(dir: &std::path::Path, root: &clap::Command) -> bool {
    match emit(dir, root) {
        Ok(paths) => {
            for p in paths {
                println!("{}", p.display());
            }
            true
        }
        Err(e) => {
            eprintln!("failed to emit skills into {}: {e}", dir.display());
            false
        }
    }
}

/// CLI entry for `skills pack`. Returns `false` when nothing matches.
pub fn run_pack(query: &str, budget: usize, json: bool, root: &clap::Command) -> bool {
    let picked = pack(query, budget, root);
    if picked.is_empty() {
        eprintln!("no skill section matches '{query}'");
        return false;
    }
    let tokens: usize = picked.iter().map(|c| estimate_tokens(&c.text)).sum();
    if json {
        let payload = serde_json::json!({
            "query": query,
            "budget": budget,
            "tokens_estimated": tokens,
            "chunks": picked,
        });
        match serde_json::to_string_pretty(&payload) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("failed to serialize pack: {e}"),
        }
    } else {
        for c in &picked {
            println!("<!-- {} / {} -->", c.skill, c.heading);
            print!("{}", c.text);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_skill_bodies_are_populated() {
        for s in SKILLS {
            assert!(!s.body.trim().is_empty(), "skill {} has empty body", s.name);
            assert!(!s.summary.is_empty(), "skill {} has empty summary", s.name);
        }
    }

    #[test]
    fn skill_names_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for s in SKILLS {
            assert!(seen.insert(s.name), "duplicate skill name {}", s.name);
        }
    }

    #[test]
    fn lookup_finds_and_misses() {
        assert!(lookup("ax-language").is_some());
        assert!(lookup("does-not-exist").is_none());
    }

    fn root() -> clap::Command {
        use clap::CommandFactory;
        crate::Cli::command()
    }

    #[test]
    fn cli_skill_lists_every_top_level_command() {
        let root = root();
        let body = render(lookup("cli").unwrap(), &root);
        for sub in root.get_subcommands().filter(|s| s.get_name() != "help") {
            let needle = format!("- `boruna {}", sub.get_name());
            assert!(
                body.contains(&needle),
                "cli skill misses `{}`",
                sub.get_name()
            );
        }
    }

    /// Every `boruna <cmd> [<sub>]` named inside a code span of a hand-written
    /// skill must exist in the real CLI.
    #[test]
    fn hand_written_skills_name_only_real_commands() {
        let root = root();
        let mut bad = Vec::new();
        for skill in SKILLS {
            let mut in_fence = false;
            for (n, line) in skill.body.lines().enumerate() {
                if line.trim_start().starts_with("```") {
                    in_fence = !in_fence;
                    continue;
                }
                let spans: Vec<&str> = if in_fence {
                    vec![line]
                } else {
                    line.split('`').skip(1).step_by(2).collect()
                };
                for span in spans {
                    let toks: Vec<&str> = span.split_whitespace().collect();
                    for (i, t) in toks.iter().enumerate() {
                        if *t != "boruna" {
                            continue;
                        }
                        let Some(top) = toks.get(i + 1).filter(|w| is_word(w)) else {
                            continue;
                        };
                        let Some(top_cmd) = root.find_subcommand(top) else {
                            bad.push(format!("{}:{} `boruna {top}`", skill.name, n + 1));
                            continue;
                        };
                        if top_cmd.has_subcommands() {
                            if let Some(sub) = toks.get(i + 2).filter(|w| is_word(w)) {
                                if top_cmd.find_subcommand(sub).is_none() {
                                    bad.push(format!(
                                        "{}:{} `boruna {top} {sub}`",
                                        skill.name,
                                        n + 1
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(
            bad.is_empty(),
            "skills name commands that do not exist: {bad:#?}"
        );
    }

    fn is_word(w: &str) -> bool {
        !w.is_empty()
            && w.chars().next().is_some_and(|c| c.is_ascii_lowercase())
            && w.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    }

    #[test]
    fn emit_writes_loadable_skills_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let root = root();
        let first = emit(dir.path(), &root).unwrap();
        assert_eq!(first.len(), SKILLS.len());
        let bytes: Vec<Vec<u8>> = first.iter().map(|p| std::fs::read(p).unwrap()).collect();
        for b in &bytes {
            let s = String::from_utf8(b.clone()).unwrap();
            assert!(
                s.starts_with("---\nname: boruna-"),
                "bad frontmatter: {s:.40}"
            );
            assert!(s.contains("\ndescription: "));
        }
        emit(dir.path(), &root).unwrap();
        for (p, b) in first.iter().zip(&bytes) {
            assert_eq!(&std::fs::read(p).unwrap(), b);
        }
    }

    #[test]
    fn pack_prefers_relevant_section_and_is_deterministic() {
        let root = root();
        let a = pack("approval gate", 2000, &root);
        let b = pack("approval gate", 2000, &root);
        assert!(!a.is_empty());
        assert_eq!(a.len(), b.len());
        assert_eq!(a[0].text, b[0].text);
        let tokens: usize = a.iter().map(|c| estimate_tokens(&c.text)).sum();
        assert!(tokens <= 2000);
    }

    #[test]
    fn pack_tiny_budget_still_returns_something() {
        let picked = pack("approval gate", 1, &root());
        assert_eq!(picked.len(), 1);
        assert!(!picked[0].text.is_empty());
    }

    #[test]
    fn pack_without_match_is_empty() {
        assert!(pack("zzzqqq", 500, &root()).is_empty());
        assert!(pack("a b", 500, &root()).is_empty());
    }
}
