# skill_pool showcase site

Static HTML + JetBrains Mono + Lunr.js + Mermaid. Served on GitHub Pages.

## Live URL

After enabling Pages (one-time, below), the site is at:

  https://olafkfreund.github.io/skill_pool/

## Files

```
site/
├── index.html           home (hero + features + arch preview)
├── architecture.html    components, flows, invariants, stack
├── use-cases.html       six real-world scenarios + "when not to"
├── quickstart.html      Nix / Docker / NixOS / Helm / Terraform / CLI
├── plugins.html         plugin authoring + sourcing modes + manifest
├── api.html             REST + MCP reference
├── demo.html            CLI ⇆ portal tabs + 90s walkthrough
├── css/terminal.css     pure black + phosphor green theme
├── js/site.js           search modal (⌘K), tabs, Mermaid init, TOC
├── search-index.json    Lunr.js index (hand-authored — update on edits)
└── media/               WebM + GIF demo recordings (copied from docs/demo/)
```

## Local preview

```bash
cd site
python3 -m http.server 8000
# → http://localhost:8000/
```

(No build step. Pure static.)

## Deploy

The workflow at [`.github/workflows/pages.yml`](../.github/workflows/pages.yml) deploys on every push to `main` that touches `site/` or `docs/demo/`.

**One-time GitHub setup** (you only do this once for the repo):

1. Repo → Settings → Pages
2. **Source:** GitHub Actions  *(not the "Deploy from a branch" option — the workflow uses the official `actions/deploy-pages` flow)*
3. Push to main. The workflow runs and the URL above goes live in ~60 seconds.

## Updating the search index

The index is hand-authored at `search-index.json`. When you add a section, append an entry with `id`, `url`, `title`, `section`, `body`. Lunr builds the index client-side at search time — no rebuild needed.

## Updating demos

The site reads from `site/media/`. To refresh:

```bash
cp docs/demo/onboarding-{cli,portal}.{webm,gif} site/media/
cp docs/demo.{webm,gif} site/media/
```

Or re-record both halves deterministically:

```bash
./scripts/record-onboarding-demo.sh
```

Then re-copy and push. The workflow handles the rest.
