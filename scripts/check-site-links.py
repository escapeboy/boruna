#!/usr/bin/env python3
"""Fail if the built site has a broken internal link or anchor.

    scripts/check-site-links.py public [--warn-prefix docs/]

Checks every href/src in every .html file: a relative or root-relative link must point at a file
that exists in the build (a directory means its index.html), and a `#fragment` must match an
element id in the target page. External links (http:, https:, mailto:, ...) are not fetched.
`--warn-prefix P` reports problems in pages under P without failing: used for the docs of a
release cut before the site existed, whose Markdown can no longer be changed.
Standard library only.
"""

import os
import sys
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urlsplit


class Page(HTMLParser):
    def __init__(self):
        super().__init__()
        self.ids, self.links = set(), []

    def handle_starttag(self, tag, attrs):
        for name, value in attrs:
            if name in ("id", "name") and value:
                self.ids.add(value)
            if name in ("href", "src") and value is not None:
                self.links.append(value)


def main() -> None:
    args = sys.argv[1:]
    warn_prefix = None
    if "--warn-prefix" in args:
        i = args.index("--warn-prefix")
        warn_prefix = args[i + 1]
        del args[i:i + 2]
    root = Path(args[0]).resolve()
    pages = {}
    for path in root.rglob("*.html"):
        parser = Page()
        parser.feed(path.read_text(errors="replace"))
        pages[path] = parser

    broken = []
    for path, page in pages.items():
        for link in page.links:
            parts = urlsplit(link)
            if parts.scheme or link.startswith("//") or link.startswith("javascript:"):
                continue
            if parts.path:
                base = root if parts.path.startswith("/") else path.parent
                target = (base / unquote(parts.path.lstrip("/"))).resolve()
            else:
                target = path
            if target.is_dir():
                target = target / "index.html"
            if not str(target).startswith(str(root)) or not target.exists():
                broken.append(f"{path.relative_to(root)}: {link} (no such file)")
                continue
            if parts.fragment and target.suffix == ".html":
                ids = pages[target].ids if target in pages else set()
                if unquote(parts.fragment) not in ids:
                    broken.append(f"{path.relative_to(root)}: {link} (no such anchor)")

    # print.html repeats every page; report its problems once, through the pages themselves.
    broken = sorted({b for b in broken if not b.split(":", 1)[0].endswith("print.html")})
    if warn_prefix:
        warned = [b for b in broken if b.startswith(warn_prefix)]
        broken = [b for b in broken if not b.startswith(warn_prefix)]
        if warned:
            print(f"warning: {len(warned)} broken internal link(s) under {warn_prefix} (not failing):")
            print("\n".join(f"  {b}" for b in warned))
    if broken:
        print(f"{len(broken)} broken internal link(s):")
        print("\n".join(f"  {b}" for b in broken))
        sys.exit(1)
    print(f"check-site-links: {len(pages)} pages, no broken internal links")


if __name__ == "__main__":
    main()
