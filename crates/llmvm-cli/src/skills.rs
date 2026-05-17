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
pub fn run_get(name: &str, json: bool) -> bool {
    let Some(skill) = lookup(name) else {
        let names: Vec<&str> = SKILLS.iter().map(|s| s.name).collect();
        eprintln!("unknown skill '{name}'. available: {}", names.join(", "));
        return false;
    };
    if json {
        let payload = serde_json::json!({
            "name": skill.name,
            "summary": skill.summary,
            "content": skill.body,
        });
        match serde_json::to_string_pretty(&payload) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("failed to serialize skill: {e}"),
        }
    } else {
        print!("{}", skill.body);
        if !skill.body.ends_with('\n') {
            println!();
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
}
