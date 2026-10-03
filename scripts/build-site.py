#!/usr/bin/env python3
"""Build the Boruna website: landing page + docs generated from the repository's Markdown.

The Markdown files in the repository are the only source. This script copies the pages listed
in site/SUMMARY.md (nothing else), rewrites links that point at repository files which are not
published so they open on GitHub, and runs mdBook.

    scripts/build-site.py --out public \
        --next-root . --next-ref master \
        [--stable-root ../stable --stable-ref v3.5.0] \
        [--boruna /path/to/boruna]

Output layout:
    /            landing page (site/landing/), with example output captured from `--boruna`
    /docs/       docs of the latest release (--stable-*), or of master when no release is given
    /next/       docs of master, with a "development version" banner
    /llms.txt, /llms-full.txt, /policy.schema.json   from the stable source

Pages keep their repository layout minus the leading `docs/`, so `docs/concepts/x.md` is served
at `/docs/concepts/x.html` (not `/docs/docs/...`). Links are recomputed for that layout.

Requires `mdbook` on PATH. Python 3.10+, standard library only.
"""

import argparse
import html
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_URL = "https://github.com/escapeboy/boruna"
RAW_URL = "https://raw.githubusercontent.com/escapeboy/boruna"
SITE_DIR = Path(__file__).resolve().parent.parent / "site"

INLINE_LINK = re.compile(r"(!?)\[((?:[^\[\]]|\[[^\]]*\])*)\]\(([^)\s]+)((?:\s+\"[^\"]*\")?)\)")
REF_LINK = re.compile(r"^(\s{0,3}\[[^\]]+\]:\s*)(\S+)(.*)$")
SUMMARY_LINK = re.compile(r"\]\(([^)]*)\)")


def summary_pages(summary: Path) -> list[str]:
    """Repository paths listed in SUMMARY.md, in order (empty draft links skipped)."""
    text = re.sub(r"<!--.*?-->", "", summary.read_text(), flags=re.S)
    return [p for p in SUMMARY_LINK.findall(text) if p]


GENERATED_CLI = "docs/reference/cli-commands.md"


def site_path(repo_path: str) -> str:
    """Where a published repository file lives inside the book.

    `docs/` is dropped, and README.md becomes index.md: mdBook renders README.md as index.html
    but does not rewrite links that point at README.md.
    """
    path = repo_path[len("docs/"):] if repo_path.startswith("docs/") else repo_path
    head, _, name = path.rpartition("/")
    if name == "README.md":
        path = (head + "/" if head else "") + "index.md"
    return path


def rewrite_target(target: str, page: str, published: set[str], root: Path, ref: str, image: bool) -> str:
    """Return the link target to use on the site for `target` found in repository file `page`."""
    if re.match(r"^[a-zA-Z][a-zA-Z0-9+.-]*:", target) or target.startswith(("#", "//", "/")):
        return target
    path, sep, anchor = target.partition("#")
    if not path:
        return target
    repo_path = os.path.normpath(os.path.join(os.path.dirname(page), path)).replace(os.sep, "/")
    if repo_path in published:
        rel = os.path.relpath(site_path(repo_path), os.path.dirname(site_path(page)) or ".")
        return rel.replace(os.sep, "/") + (sep + anchor if sep else "")
    if repo_path.startswith("../") or not (root / repo_path).exists():
        return target  # left as is; the link checker reports it
    if image:
        return f"{RAW_URL}/{ref}/{repo_path}"
    kind = "tree" if (root / repo_path).is_dir() else "blob"
    return f"{REPO_URL}/{kind}/{ref}/{repo_path}" + (sep + anchor if sep else "")


def rewrite_links(text: str, page: str, published: set[str], root: Path, ref: str) -> str:
    out, in_fence, fence = [], False, ""
    for line in text.split("\n"):
        stripped = line.lstrip()
        if stripped.startswith(("```", "~~~")):
            marker = stripped[:3]
            if not in_fence:
                in_fence, fence = True, marker
            elif marker == fence:
                in_fence = False
            out.append(line)
            continue
        if in_fence:
            out.append(line)
            continue
        line = INLINE_LINK.sub(
            lambda m: f"{m.group(1)}[{m.group(2)}]("
            f"{rewrite_target(m.group(3), page, published, root, ref, bool(m.group(1)))}{m.group(4)})",
            line,
        )
        m = REF_LINK.match(line)
        if m:
            line = m.group(1) + rewrite_target(m.group(2), page, published, root, ref, False) + m.group(3)
        out.append(line)
    return "\n".join(out)


def generated_cli_page(boruna: str) -> str:
    """The command reference, generated from the binary of this version (cannot drift)."""
    version = subprocess.run([boruna, "--version"], capture_output=True, text=True, check=True).stdout.strip()
    skill = subprocess.run([boruna, "skills", "get", "cli"], capture_output=True, text=True, check=True).stdout
    marker = "## Command reference"
    if marker not in skill:
        sys.exit("build-site: `boruna skills get cli` has no generated command reference")
    body = skill[skill.index(marker):].split("\n", 1)[1]
    body = re.sub(r"^### ", "## ", body, flags=re.M)
    return (f"# Command reference\n\nGenerated from `{version} --help` when this site was built. "
            "For explanations and examples see the [CLI guide](cli.md).\n" + body)


def build_book(root: Path, ref: str, summary: Path, out: Path, site_url: str,
               banner: str | None, strict: bool, boruna: str | None) -> None:
    pages = summary_pages(summary)
    generated = {}
    if GENERATED_CLI in pages:
        if not boruna:
            sys.exit(f"build-site: {GENERATED_CLI} is generated; pass the boruna binary for {ref}")
        generated[GENERATED_CLI] = generated_cli_page(boruna)
    missing = [p for p in pages if p not in generated and not (root / p).is_file()]
    if missing and strict:
        sys.exit(f"build-site: pages listed in {summary} do not exist: {', '.join(missing)}")
    summary_text = re.sub(r"<!--.*?-->", "", summary.read_text(), flags=re.S)
    for p in missing:
        # Only for an older release that predates a page: drop it from that version's nav.
        print(f"build-site: warning: {p} does not exist at {ref}; left out of that version", file=sys.stderr)
        summary_text = re.sub(rf"^.*\]\({re.escape(p)}\).*\n", "", summary_text, flags=re.M)
        pages.remove(p)
    published = set(pages)

    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        src = work / "src"
        src.mkdir()
        summary_text = SUMMARY_LINK.sub(lambda m: f"]({site_path(m.group(1))})" if m.group(1) else m.group(0),
                                        summary_text)
        (src / "SUMMARY.md").write_text(summary_text)
        for page in pages:
            raw = generated.get(page) or (root / page).read_text()
            text = rewrite_links(raw, page, published, root, ref)
            if banner:
                text = f'<div class="version-banner">{banner}</div>\n\n' + text
            if page not in generated:
                text += (f'\n\n<p class="edit-link"><a href="{REPO_URL}/edit/master/{page}">'
                         "Edit this page on GitHub</a></p>\n")
            dest = src / site_path(page)
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_text(text)
        config = (SITE_DIR / "book.toml").read_text().replace("{site_url}", site_url)
        (work / "book.toml").write_text(config)
        shutil.copy(SITE_DIR / "site.css", work / "site.css")
        shutil.copytree(SITE_DIR / "theme", work / "theme")
        subprocess.run(["mdbook", "build", str(work), "--dest-dir", str(out)], check=True)


def example_output(boruna: str, root: Path) -> tuple[str, str]:
    """Run the example the landing page shows and return its terminal output."""
    with tempfile.TemporaryDirectory() as tmp:
        data = Path(tmp)
        run = subprocess.run(
            [boruna, "workflow", "run", "examples/workflows/confidence_gated_review",
             "--policy", "allow-all", "--record", "--data-dir", str(data),
             "--evidence-dir", str(data / "evidence")],
            cwd=root, capture_output=True, text=True, check=True,
        )
        bundle = next((data / "evidence").iterdir())
        verify = subprocess.run([boruna, "evidence", "verify", str(bundle)],
                                capture_output=True, text=True, check=True)
        version = subprocess.run([boruna, "--version"], capture_output=True, text=True, check=True)
        text = (
            "$ boruna workflow run examples/workflows/confidence_gated_review --policy allow-all --record\n"
            + run.stdout.strip() + "\n\n$ boruna evidence verify runs/evidence/" + bundle.name + "\n"
            + verify.stdout.strip()
        )
        text = text.replace(str(data), "runs")
        return html.escape(text), version.stdout.strip()


def build_landing(out: Path, boruna: str | None, root: Path) -> None:
    page = (SITE_DIR / "landing" / "index.html").read_text()
    if boruna:
        output, version = example_output(boruna, root)
        page = page.replace("<!-- EXAMPLE_OUTPUT -->", output)
        page = page.replace("<!-- EXAMPLE_VERSION -->", html.escape(version))
    elif "<!-- EXAMPLE_OUTPUT -->" in page:
        sys.exit("build-site: --boruna is required to fill in the landing page example")
    out.mkdir(parents=True, exist_ok=True)
    (out / "index.html").write_text(page)
    for asset in (SITE_DIR / "landing").iterdir():
        if asset.name != "index.html":
            shutil.copy(asset, out / asset.name)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--next-root", required=True, type=Path)
    ap.add_argument("--next-ref", default="master")
    ap.add_argument("--stable-root", type=Path)
    ap.add_argument("--stable-ref")
    ap.add_argument("--stable-boruna", help="boruna binary of the stable release (generated pages, landing example)")
    ap.add_argument("--next-boruna", help="boruna binary built from master (generated pages)")
    ap.add_argument("--site-url-prefix", default="/")
    args = ap.parse_args()

    out = args.out.resolve()
    if out.exists():
        shutil.rmtree(out)
    next_root = args.next_root.resolve()
    summary = SITE_DIR / "SUMMARY.md"

    if args.stable_root:
        stable_root, stable_ref = args.stable_root.resolve(), args.stable_ref
        # A release cut before the site existed has no site/SUMMARY.md of its own.
        stable_summary = stable_root / "site" / "SUMMARY.md"
        own = stable_summary.is_file()
        build_book(stable_root, stable_ref, stable_summary if own else summary,
                   out / "docs", args.site_url_prefix + "docs/", None, strict=own,
                   boruna=args.stable_boruna)
        stable_boruna = args.stable_boruna
    else:
        stable_root, stable_ref = next_root, args.next_ref
        stable_boruna = args.next_boruna
        build_book(next_root, args.next_ref, summary, out / "docs", args.site_url_prefix + "docs/",
                   None, strict=True, boruna=args.next_boruna)

    banner = (
        "You are reading the development version (<code>master</code>). "
        f'For the latest release ({html.escape(stable_ref)}) see <a href="/docs/">the stable docs</a>.'
    )
    build_book(next_root, args.next_ref, summary, out / "next", args.site_url_prefix + "next/",
               banner, strict=True, boruna=args.next_boruna)

    build_landing(out, stable_boruna, stable_root)
    for name, dest in (("llms.txt", "llms.txt"), ("llms-full.txt", "llms-full.txt"),
                       ("docs/reference/policy.schema.json", "policy.schema.json")):
        if (stable_root / name).is_file():
            shutil.copy(stable_root / name, out / dest)
    print(f"build-site: wrote {out}")


if __name__ == "__main__":
    main()
