#!/usr/bin/env python3
"""Generate docs/index.md (the Backstage TechDocs landing page) from README.md.

README.md is the single source of truth for the project overview. TechDocs
serves docs/ as its docs_dir, so the landing page must live *inside* docs/ with
links rewritten relative to that directory:

  * `docs/...`        -> stripped (docs/ is the TechDocs root)
  * repo-root dirs    -> absolute GitHub blob URLs (they aren't shipped to TechDocs)
  * http(s)/#/mailto  -> left untouched

Run locally (or in CI) after editing README.md so the GitHub README and the
Backstage TechDocs home stay in sync. Idempotent: re-running with an unchanged
README produces an identical docs/index.md.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
README = REPO_ROOT / "README.md"
INDEX = REPO_ROOT / "docs" / "index.md"

GITHUB_BLOB = "https://github.com/olafkfreund/skill_pool/blob/main/"

# Top-level repo directories/files that are NOT part of the TechDocs docs_dir.
# Links to these must point at GitHub, not at a (non-existent) TechDocs page.
REPO_PATHS = {
    "deploy", ".github", "server", "web", "cli", "scripts", "packaging",
    "nix", "ops", "site", "enterprise", "direnv", "skills", "CHANGELOG.md",
    "LICENSE", "flake.nix", "Cargo.toml",
}

GENERATED_BANNER = (
    "<!-- DO NOT EDIT. Generated from README.md by scripts/sync-techdocs.py. "
    "Edit README.md and re-run the script (CI does this on every push). -->\n\n"
)


def rewrite_target(target: str) -> str:
    """Rewrite a single link/image/href target for the TechDocs context."""
    stripped = target.strip()
    if stripped.startswith(("http://", "https://", "#", "mailto:", "//")):
        return target

    # Normalise a leading ./
    norm = stripped[2:] if stripped.startswith("./") else stripped

    if norm.startswith("docs/"):
        inner = norm[len("docs/"):]
        # A bare directory link (e.g. "docs/wiki/") has no TechDocs page; point
        # it at the directory's README.md/index.md when one exists.
        if inner.endswith("/"):
            for candidate in ("README.md", "index.md"):
                if (REPO_ROOT / "docs" / inner / candidate).exists():
                    return inner + candidate
        return inner

    first = norm.split("/", 1)[0].split("#", 1)[0]
    if first in REPO_PATHS:
        return GITHUB_BLOB + norm

    return target


# Markdown: ](target)  and  ](target "title")
_MD_LINK = re.compile(r"(\]\()([^)\s]+)(\s+\"[^\"]*\")?(\))")
# HTML: src="target" / href="target" / src='target' / href='target'
_HTML_ATTR = re.compile(r"((?:src|href)=)([\"'])([^\"']+)([\"'])")


def rewrite(markdown: str) -> str:
    def md_sub(m: re.Match) -> str:
        return f"{m.group(1)}{rewrite_target(m.group(2))}{m.group(3) or ''}{m.group(4)}"

    def html_sub(m: re.Match) -> str:
        return f"{m.group(1)}{m.group(2)}{rewrite_target(m.group(3))}{m.group(4)}"

    out = _MD_LINK.sub(md_sub, markdown)
    out = _HTML_ATTR.sub(html_sub, out)
    return out


def main() -> int:
    if not README.exists():
        print(f"error: {README} not found", file=sys.stderr)
        return 1

    content = README.read_text(encoding="utf-8")
    rendered = GENERATED_BANNER + rewrite(content)

    check = "--check" in sys.argv
    current = INDEX.read_text(encoding="utf-8") if INDEX.exists() else None

    if check:
        if current != rendered:
            print(
                "docs/index.md is out of sync with README.md.\n"
                "Run: python3 scripts/sync-techdocs.py",
                file=sys.stderr,
            )
            return 1
        print("docs/index.md is in sync with README.md.")
        return 0

    INDEX.parent.mkdir(parents=True, exist_ok=True)
    INDEX.write_text(rendered, encoding="utf-8")
    print(f"wrote {INDEX.relative_to(REPO_ROOT)} ({len(rendered)} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
