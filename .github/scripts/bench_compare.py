#!/usr/bin/env python3
"""Render a criterion benchmark comparison as a markdown table.

Reads `target/criterion/<group>/<bench>/change/estimates.json` files
produced by `cargo bench -- --baseline <name>` and emits a markdown
table summarising the mean change per benchmark. A benchmark is
flagged as a regression if its mean point-estimate is at least
`--threshold` (default 0.10 = 10%) slower.

Output is two files:
- `--out`            markdown comment body
- `--regressed-out`  one line per regressed benchmark, empty if none

This script intentionally avoids non-stdlib deps so it runs on any
self-hosted runner with python3 installed.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def find_change_files(criterion_dir: Path):
    """Yield (bench_label, change_estimates_path) tuples."""
    for change in sorted(criterion_dir.rglob("change/estimates.json")):
        bench_dir = change.parent.parent
        # Construct a relative bench label: <group>/<bench>
        rel = bench_dir.relative_to(criterion_dir)
        yield (str(rel), change)


def fmt_pct(x: float) -> str:
    sign = "+" if x >= 0 else ""
    return f"{sign}{x * 100:.2f}%"


def render_row(label: str, change: dict) -> tuple[str, float]:
    mean = change["mean"]["point_estimate"]
    ci = change["mean"]["confidence_interval"]
    lo = ci["lower_bound"]
    hi = ci["upper_bound"]
    return (
        f"| `{label}` | {fmt_pct(mean)} | [{fmt_pct(lo)}, {fmt_pct(hi)}] |",
        mean,
    )


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--criterion-dir", type=Path, required=True)
    p.add_argument("--threshold", type=float, default=0.10)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--regressed-out", type=Path, required=True)
    args = p.parse_args()

    if not args.criterion_dir.exists():
        args.out.write_text(
            "**Bench compare:** no criterion output found "
            f"(`{args.criterion_dir}` missing).\n"
        )
        args.regressed_out.write_text("")
        return 0

    rows = []
    regressed: list[str] = []
    for label, change_path in find_change_files(args.criterion_dir):
        try:
            data = json.loads(change_path.read_text())
        except (OSError, json.JSONDecodeError) as e:
            rows.append((f"| `{label}` | (error: {e}) | — |", 0.0))
            continue
        row, mean = render_row(label, data)
        rows.append((row, mean))
        if mean >= args.threshold:
            regressed.append(label)

    lines = [
        "**Bench compare**",
        "",
        f"Threshold for regression: ≥ {args.threshold * 100:.0f}% slower mean.",
        "",
        "| Benchmark | Mean change | 99% CI |",
        "|-----------|-------------|--------|",
    ]
    if not rows:
        lines.append("| _(no benchmarks ran)_ | — | — |")
    else:
        lines.extend(row for row, _ in rows)

    if regressed:
        lines.append("")
        lines.append(
            f"⚠️ {len(regressed)} benchmark(s) regressed past threshold:"
        )
        for label in regressed:
            lines.append(f"- `{label}`")
    else:
        lines.append("")
        lines.append("✅ No benchmark regressed past threshold.")

    args.out.write_text("\n".join(lines) + "\n")
    args.regressed_out.write_text("\n".join(regressed))
    return 0


if __name__ == "__main__":
    sys.exit(main())
