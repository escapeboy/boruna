#!/usr/bin/env python3
"""Compile the `.ax` examples in the published docs, so they cannot drift from the compiler.

    scripts/check-doc-examples.py --boruna target/release/boruna

Scope: the pages listed in site/SUMMARY.md. A fenced block whose info string starts with `ax`
and that contains `fn main` is a complete program and must compile. Blocks without `fn main`
are fragments and are not checked. A complete-looking block that is deliberately not
standalone (part of a larger app, shows an error on purpose) is marked on the line before the
fence with an HTML comment, which renders as nothing on GitHub and on the site:

    <!-- ax-check: skip (part of the counter app above) -->

Standard library only.
"""

import argparse
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FENCE = re.compile(r"^```(\S*)[^\n]*\n(.*?)^```", re.M | re.S)
SKIP = re.compile(r"<!--\s*ax-check:\s*skip\b[^>]*-->\s*$")


def published_pages() -> list[str]:
    text = re.sub(r"<!--.*?-->", "", (ROOT / "site" / "SUMMARY.md").read_text(), flags=re.S)
    return [p for p in re.findall(r"\]\(([^)]*)\)", text) if p and (ROOT / p).is_file()]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--boruna", required=True)
    args = ap.parse_args()

    checked, failures = 0, []
    with tempfile.TemporaryDirectory() as tmp:
        for page in published_pages():
            text = (ROOT / page).read_text()
            for m in FENCE.finditer(text):
                if m.group(1).split(",")[0] != "ax" or "fn main" not in m.group(2):
                    continue
                before = text[: m.start()].rstrip("\n").rsplit("\n", 1)[-1]
                if SKIP.search(before):
                    continue
                checked += 1
                line = text[: m.start()].count("\n") + 1
                src = Path(tmp) / f"example{checked}.ax"
                src.write_text(m.group(2))
                run = subprocess.run(
                    [args.boruna, "compile", str(src), "--output", str(src.with_suffix(".axbc"))],
                    capture_output=True, text=True,
                )
                if run.returncode != 0:
                    first = (run.stderr or run.stdout).strip().splitlines()[:1]
                    failures.append(f"{page}:{line}: {first[0] if first else 'compile failed'}")

    if failures:
        print(f"{len(failures)} of {checked} doc examples do not compile:")
        print("\n".join(f"  {f}" for f in failures))
        sys.exit(1)
    print(f"check-doc-examples: {checked} complete .ax examples compile")


if __name__ == "__main__":
    main()
