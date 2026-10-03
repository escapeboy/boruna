# Boruna website and docs site — requirements

Status: requirements (brainstorm, 2026-10-03). No design or implementation yet.

## Decisions already made

| Question | Decision |
|---|---|
| Hosting | Own domain on Cloudflare Pages (domain still to be chosen) |
| Which docs | Only user-facing docs, from an explicit list; internal docs stay in the repo only |
| Versions | Latest release (default) and `master` ("next", with a banner) |
| Main message | "Tamper-evident record for AI decisions" |
| Domain | `boruna.fleetq.net` |
| Cloudflare account | the one owned by katsarov@gmail.com |
| Older upper-case specs | Review each one first; publish only what is accurate |
| Analytics | Cloudflare Web Analytics (cookie-free) |
| Language | English only |
| Visual identity | None beyond `docs/BRANDING.md`; no logo work |

## Why

- There is no site today: GitHub Pages is off and the repo has no homepage URL.
- The GitHub repo description is out of date ("programming language and framework for
  LLM-native applications"); the README already positions Boruna as a deterministic,
  auditable execution platform.
- `docs/` holds 167 Markdown files outside the archive, and about half are internal
  (`design-*`, `architecture-*`, `test-plan-*`, retros, security-review notes). A reader on
  GitHub cannot tell which ones are meant for them.
- Docs have drifted from the code before (`limitations.md` said there were no loops or mutable
  variables; the README listed a macOS Intel binary that was not shipped). A site must not add a
  second copy that can drift too.

## Audiences

1. **Auditor / compliance** — wants to know what a bundle proves, how to verify it without Rust,
   and what the EU AI Act / NIST / ISO 42001 reports cover.
2. **Developer evaluating Boruna** — wants a one-minute pitch, install, a working example and the
   limits stated honestly.
3. **Team embedding it in CI** — wants CLI reference, JSON output, versions, release assets.
4. **AI agents** — want `llms.txt`, `llms-full.txt` and the MCP tool reference at stable URLs.

## Functional requirements

### Landing page

- F1. Headline built on "tamper-evident record for AI decisions", with a short plain-language
  explanation under it.
- F2. Three proof points, each linking to the doc page that backs it and to the command that shows
  it: hash-chained evidence bundles (`evidence verify`), human approval with calibrated confidence
  gates, and deterministic replay.
- F3. Compliance section: what `evidence report --framework eu-ai-act|nist|iso42001` produces, and
  what it does not claim.
- F4. Install section with the two one-line installers (`install.sh`, `install.ps1`) and the
  supported-platform table.
- F5. A real end-to-end example (`workflow run --record` then `evidence verify`). The terminal
  output shown on the page is captured by CI from the released binary, not typed by hand.
- F6. A second, smaller entry for developers and agents: `.ax`, the MCP server, `llms.txt`.
- F7. Every factual claim on the landing page links to a doc page or a command. No claim without a
  source in the repo.

### Docs

- F8. The Markdown files in the GitHub repository are the only source. The site is generated from
  them; nobody edits site copies.
- F9. A single list file in the repo names every published page and the navigation order. A file
  not on the list is not published. A listed file that does not exist fails the build.
- F10. Links between Markdown files (`../reference/cli.md`, `#anchors`) work on the site and on
  GitHub alike.
- F11. Each page has an "Edit on GitHub" link to its source file and shows the version it belongs
  to.
- F12. Two versions: the latest release at the default path, `master` under a separate path with a
  "development version" banner. The version switcher only offers these two.
- F13. `CHANGELOG.md` is published as a page; release notes stay generated from it as today.
- F14. Client-side search over the published pages.
- F15. `llms.txt`, `llms-full.txt` and the JSON policy schema are served at stable URLs on the
  site root.

### Sync and publishing

- F16. Pushing to `master` rebuilds and deploys the "next" docs. Pushing a release tag rebuilds and
  deploys the stable docs and the landing page.
- F17. Pull requests that touch docs get a preview deployment, and the build runs as a CI check.
- F18. CI fails on broken internal links and anchors in published pages.
- F19. `.ax` code blocks marked as complete programs are compiled (and, if marked, run) in CI, so
  examples in the docs cannot drift from the compiler. This also covers the open "docs match the
  code" item.
- F20. The GitHub repo description and homepage field are updated to match the site.

## Non-functional requirements

- N1. Static site; pages are readable without JavaScript (search may need it).
- N2. Works on phones; light and dark themes.
- N3. No cookies and no third-party trackers. Cloudflare Web Analytics (cookie-free) is the only analytics.
- N4. Build is reproducible from the repo alone and runs in under a few minutes in CI.
- N5. The Cloudflare token used by CI is scoped to this Pages project only, stored in the
  1Password `AI Agent` vault and as a GitHub Actions secret, never in the repo.
- N6. Accessible: headings in order, alt text for images, sufficient contrast.

## Initial page list (proposal, to confirm)

Publish:
- Start: README (as intro), `QUICKSTART.md`, `faq.md`, `limitations.md`
- Concepts: `concepts/*` (determinism, capabilities, evidence bundles, bundle storage,
  runtime provenance, threat model)
- Guides: `first-workflow`, `llm-integration`, `model-eval`, `migration`, `lsp`,
  `bundle-storage-{s3,gcs,azure}`, `kek-rotation`
- Reference: `reference/*` including `stdlib/*`, `DIAGNOSTICS_AND_REPAIR.md`
- Specifications: `spec/*`
- Trust: `COMPLIANCE_EVIDENCE.md`, `SECURITY_MODEL.md`, `stability.md`, `lts.md`,
  `roadmap.md`, `CHANGELOG.md`, `SECURITY.md`

Keep in the repo only: `design-*`, `architecture-*`, `test-plan-*`, `archive/`, retros,
`security-review-*`, `release-smoke-tests/`, `AGENT-PROMPT-TEMPLATE.md`, `BRANDING.md`,
`stdlib-graduation-tracker.md`, `releasing.md`, `post-1.0/`, `adr/`.

Needs a decision (older upper-case specs that may be stale): `FRAMEWORK_SPEC.md`,
`FRAMEWORK_API.md`, `ORCHESTRATOR_SPEC.md`, `PACKAGE_SPEC.md`, `LLM_EFFECT_SPEC.md`,
`EFFECTS_GUIDE.md`, `ACTORS_GUIDE.md`, `INTEGRATION_GUIDE.md`, `OPERATIONS.md`,
`PERFORMANCE.md`, `PLATFORM_GOVERNANCE.md`, `ENTERPRISE_PLATFORM_OVERVIEW.md`,
`DETERMINISM_CONTRACT.md`, `TESTING_GUIDE.md`, `TRACE_TO_TESTS.md`, `APP_TEMPLATE.md`,
`language-guide.md`.

## Acceptance criteria

- A1. Opening the domain shows the landing page; every proof point and the compliance section link
  to a published doc page.
- A2. The install commands on the landing page are the same text as in the README and are run by
  CI.
- A3. Changing a published `.md` file on `master` changes the "next" site within one CI run, with
  no other manual step.
- A4. Tagging a release updates the stable docs and the version shown on every page.
- A5. A page not in the list file is not reachable on the site; a missing listed file fails CI.
- A6. A broken internal link or a non-compiling marked `.ax` block fails CI on the pull request.
- A7. `/llms.txt` and `/llms-full.txt` return the files from the repo at that version.

## Open questions

All six questions from the brainstorm are answered in "Decisions already made".

## Design (first PR)

- **Generator: mdBook 0.5.4.** Checked by hand: a page listed in `SUMMARY.md` that does not exist
  fails the build (exit 101) with `create-missing = false`; links between `.md` files become
  `.html` links with anchors kept; search is built in. One static binary, no Node.
- **`site/SUMMARY.md`** is the page list (F9), using repository paths.
- **`scripts/build-site.py`** copies only the listed pages, drops the leading `docs/` from paths
  (so `/docs/concepts/x.html`, not `/docs/docs/...`), maps `README.md` to `index.md`, recomputes
  links between published pages, points links to unpublished repository files at GitHub (images at
  raw.githubusercontent.com), skips fenced code, adds an "Edit this page on GitHub" link and, for
  `master`, a development banner. It builds `/docs/` (latest release) and `/next/` (`master`), the
  landing page at `/`, and copies `llms.txt`, `llms-full.txt` and `policy.schema.json` to the root.
- **Generated command reference** (`docs/reference/cli-commands.md`, not a file in the repo) comes
  from `boruna skills get cli` of the same version's binary, so it lists exactly the commands that
  version has. The hand-written `cli.md` stays as the guide.
- **Landing example output** is produced at build time by running the release binary.
- **`scripts/check-site-links.py`** fails on any broken internal link or anchor in the built HTML.
- **CI job "Docs site"** builds the site from `master` and checks links on every pull request.

Left for later PRs:
- Deployment to Cloudflare Pages (`boruna.fleetq.net`), the API token, DNS and Web Analytics
  (needs explicit approval).
- Compiling `.ax` examples in the docs (F19).
- Pages held back until fixed: the 10 FIX documents from the review, `concepts/threat-model.md`
  (says Rekor anchoring is not implemented; `evidence anchor` exists since v3.2.0),
  `roadmap.md` ("Current: 1.4.0"), and the unreviewed `faq.md`, `SECURITY_MODEL.md`,
  `COMPLIANCE_EVIDENCE.md`. `LLM_EFFECT_SPEC.md`, `language-guide.md` and `INTEGRATION_GUIDE.md`
  move to `docs/archive/`.

