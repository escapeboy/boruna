//! Thin CLI wrapper around `boruna_tooling::format`.
//!
//! Two modes:
//!
//! - `boruna fmt <file>` — rewrite `<file>` in place with the canonical
//!   formatting. Exits 0 on success.
//! - `boruna fmt --check <file>` — exit 0 if the file is already
//!   canonically formatted, exit 1 otherwise (prints a short diff
//!   summary to stderr). Designed as a CI gate.
//!
//! Parse errors are surfaced with exit code 2 so CI can distinguish
//! "needs formatting" (1) from "broken file" (2).

use std::fs;
use std::path::Path;
use std::process;

use boruna_tooling::format::{format_source, FormatError};

pub fn run_fmt(file: &Path, check: bool) -> Result<(), Box<dyn std::error::Error>> {
    let source = fs::read_to_string(file)?;

    let formatted = match format_source(&source) {
        Ok(s) => s,
        Err(FormatError::ParseFailed { line, col, message }) => {
            match col {
                Some(c) => eprintln!(
                    "boruna fmt: parse failed in {} at line {line}, col {c}: {message}",
                    file.display()
                ),
                None => eprintln!(
                    "boruna fmt: parse failed in {} at line {line}: {message}",
                    file.display()
                ),
            }
            process::exit(2);
        }
    };

    if check {
        if formatted == source {
            // Already formatted — no output, exit 0.
            return Ok(());
        }
        eprintln!(
            "boruna fmt: {} is not formatted. Run `boruna fmt {0}` to fix.",
            file.display()
        );
        // Print a minimal diff hint: the first differing line.
        if let Some((lineno, src_line, fmt_line)) = first_diff_line(&source, &formatted) {
            eprintln!("  first diff at line {lineno}:");
            eprintln!("    -{src_line}");
            eprintln!("    +{fmt_line}");
        }
        process::exit(1);
    }

    if formatted != source {
        fs::write(file, &formatted)?;
        println!("formatted {}", file.display());
    } else {
        println!("{} already formatted", file.display());
    }
    Ok(())
}

fn first_diff_line<'a>(a: &'a str, b: &'a str) -> Option<(usize, &'a str, &'a str)> {
    for (i, (la, lb)) in a.lines().zip(b.lines()).enumerate() {
        if la != lb {
            return Some((i + 1, la, lb));
        }
    }
    // One side is a prefix of the other.
    let acount = a.lines().count();
    let bcount = b.lines().count();
    if acount != bcount {
        return Some((acount.min(bcount) + 1, "", ""));
    }
    None
}
