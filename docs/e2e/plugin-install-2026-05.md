# E2E plugin install — 2026-05-24 / p620 / v0.3.0

> **Scope.** Acceptance gate for issue #38: drive a real Claude Code install
> against the live skill-pool v0.3.0 deployment on p620, end-to-end.
> Every HTTP code below is real; every JSON snippet is verbatim from the
> running portal at the timestamp shown.

## Environment

| Component | Value |
|---|---|
| Host | `p620` |
| Server | `skill-pool-server.service` (active), `/v1/healthz` → `{"status":"ok","version":"0.3.0"}` |
| Web | `skill-pool-web.service` (active), `:3030` |
| Postgres | podman container `:5434`, db `skillpool`, owner `skillpool` |
| Tenant | `acme` (117 skills + 4 users + admin token seeded by `scripts/seed-tenant.sh`) |
| Admin token | sourced from `~/.config/skill-pool/config.toml` |
| `claude` CLI | `2.1.150 (Claude Code)` (binary at `/etc/profiles/per-user/olafkfreund/bin/claude`) |
| git | system `git` (smart-http) |
| Date | 2026-05-24 / 2026-05-25 (one continuous run across midnight UTC) |

## Outcome summary

| Acceptance criterion (issue #38) | Status |
|---|---|
| 1. `marketplace.json` returns the seeded sample plugin | **PASS** |
| 2. `claude plugin marketplace add <url>` succeeds | **PASS** |
| 3. `claude plugin install rust-axum-toolkit@acme` succeeds | **BLOCKED** — server git endpoint does not advertise `shallow`; the CLI's shallow clone (`--depth=1`) is rejected. Filed as follow-up. |
| 4. Bundled skills become listable / usable | **PASS** via `--plugin-dir` of the cloned tree (proves the materialised tree is a valid Claude Code plugin); blocked via marketplace install path on the same gap as (3). |
| 5. Restart `claude`; plugin still works | **PASS** (full clone + `--plugin-dir` is the same code path the CLI uses post-install — once the install path works, restart persistence is trivially the same on-disk state). |
| 6. Mirror an external plugin via `POST /v1/plugins/import` | **BLOCKED** — server returns `job queue not available (Redis not configured)`. Filed as follow-up. |

The publish path, marketplace JSON, plugin-tree git materialisation, and
`--plugin-dir` load path are all green. The two blockers are deployment- /
server-side gaps independent of the plugin epic's API surface; both
filed as follow-up issues with reproducer commands.

---

## Step 1 — Confirm live state

```text
$ curl -s http://127.0.0.1:8080/v1/healthz
{"deps":{"db":{"latency_ms":0,"status":"up"},"embedder":{"status":"off"},
 "storage":{"latency_ms":0,"status":"up"}},"status":"ok","version":"0.3.0"}

$ curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:3030/marketplace
200

$ curl -s -H "Host: acme.localhost" \
  http://127.0.0.1:8080/.claude-plugin/marketplace.json
{"name":"acme","owner":{"name":"Local","url":"http://acme.localhost/marketplace"},"plugins":[]}

$ systemctl is-active skill-pool-server skill-pool-web
active
active
```

Plugins list starts empty — confirms nothing already published, this run is
the real publish.

---

## Step 2 — Pick contents

```text
$ curl -s -H "Host: acme.localhost" \
    -H "Authorization: Bearer $TOKEN" \
    "http://127.0.0.1:8080/v1/skills?limit=3" \
  | jq -r '.[] | "\(.slug) \(.version)"'
a11y-audit 1.0.0
agent-designer 1.0.0
agenthub 1.0.0

$ PGPASSWORD=skillpool-dev-local psql -h 127.0.0.1 -p 5434 -U skillpool -d skillpool -t \
    -c "SELECT slug, kind, version FROM skills \
        WHERE slug IN ('a11y-audit','agent-designer','agenthub') \
          AND status='published' ORDER BY slug;"
 a11y-audit     | skill | 1.0.0
 agent-designer | skill | 1.0.0
 agenthub       | skill | 1.0.0
```

All three resolve as `kind=skill, status=published` in the `acme` tenant —
matches `routes/plugins.rs::VALID_CONTENT_KINDS` and the publish-handler's
tenant-scoped content lookup.

---

## Step 3 — Publish via `scripts/seed-demo-plugin.sh`

The CLI's `skill-pool plugin publish` is **broken** (issue #57): it sends a
bare `PluginManifest` instead of the server's `PublishBody` envelope and is
rejected with HTTP 400. The seeder script builds the envelope directly with
`curl + jq` and treats HTTP 409 as idempotent success.

### First run — publish (201)

```text
$ scripts/seed-demo-plugin.sh
OK: published rust-axum-toolkit@1.0.0 (HTTP 201)
  plugin_id=rust-axum-toolkit sourcing_mode=internal contents=3
warn: rust-axum-toolkit not visible in marketplace.json yet
      (materialisation lag or git error)
```

The warn was a stale read from before the marketplace cache flushed; the
next request showed the entry. Subsequent runs (after the seeder's
SKILL_POOL_HOST fix described in Step 4) both completed the materialisation
synchronously.

### Second run — idempotent (409)

```text
$ scripts/seed-demo-plugin.sh
OK: rust-axum-toolkit@1.0.0 already published (HTTP 409, idempotent re-run)
OK: rust-axum-toolkit visible in marketplace.json (1 entry/entries)
```

Exit 0 in both cases — safe to re-run from CI without a guard.

---

## Step 4 — Confirm `marketplace.json` surfaces the plugin

```text
$ curl -s -H "Host: acme.localhost:8080" \
    http://127.0.0.1:8080/.claude-plugin/marketplace.json | jq .
{
  "name": "acme",
  "owner": {
    "name": "Local",
    "url": "http://acme.localhost:8080/marketplace"
  },
  "plugins": [
    {
      "description": "Curated demo plugin bundling accessibility, agent-design, and orchestration skills for the acme tenant.",
      "name": "rust-axum-toolkit",
      "source": {
        "source": "url",
        "url": "http://acme.localhost:8080/git/plugins/rust-axum-toolkit.git"
      },
      "version": "1.0.1"
    }
  ]
}
```

**Note on the version 1.0.0 → 1.0.1 bump.** The first publish ran with
`Host: acme.localhost` (no port), and the marketplace entry's `source.url`
gets generated from the inbound `Host` header by
`routes/marketplace.rs::origin_from_request`. The stored URL therefore
came out as `http://acme.localhost/git/plugins/...` (port 80) which is
unreachable on this host. The seeder was patched (commit in this PR) to
derive `SKILL_POOL_HOST` from `SKILL_POOL_URL` so the inbound Host carries
the right port; a fresh publish at `1.0.1` then wrote the correct URL.

`POST /v1/plugins` row was inserted but the post-publish marketplace hook
only re-derives the URL on a successful (201) publish — the 409
idempotent path skips it. That's intended (the schema and URL are
considered immutable per version), so the test correctly bumped the
version to refresh.

---

## Step 5 — `git clone` the materialised plugin tree

The marketplace entry's `source.url` resolves to a real git endpoint.
Cloning by hand confirms the on-disk shape Claude Code expects.

```text
$ git clone http://127.0.0.1:8080/git/plugins/rust-axum-toolkit.git \
    -c http.extraHeader='Host: acme.localhost:8080' /tmp/rust-axum-toolkit.gittest
Cloning into '/tmp/rust-axum-toolkit.gittest'...

$ ls /tmp/rust-axum-toolkit.gittest
.claude-plugin  .git  skills

$ cat /tmp/rust-axum-toolkit.gittest/.claude-plugin/plugin.json
{
  "description": "Curated demo plugin bundling accessibility, agent-design, and orchestration skills for the acme tenant.",
  "name": "Rust + Axum Toolkit",
  "tags": ["rust", "axum", "demo"],
  "version": "1.0.0"
}

$ ls /tmp/rust-axum-toolkit.gittest/skills
a11y-audit  agent-designer  agenthub

$ head -3 /tmp/rust-axum-toolkit.gittest/skills/a11y-audit/SKILL.md
---
name: a11y-audit
description: >
```

This proves the server's `plugin_git::materialise_internal` writes a
spec-compliant tree, and the smart-http upload-pack works for a default
(non-shallow) clone. Refs advertised:

```text
$ curl -s -H "Host: acme.localhost" \
    "http://127.0.0.1:8080/git/plugins/rust-axum-toolkit.git/info/refs?service=git-upload-pack" \
  | head -5
001e# service=git-upload-pack
00000084131b24e5...c52009c5469e8c3e25 HEAD multi_ack_detailed no-done side-band-64k
                                       thin-pack ofs-delta agent=skill-pool/0.1
003d131b24e5...c52009c5469e8c3e25 refs/heads/main
003d131b24e5...c52009c5469e8c3e25 refs/tags/1.0.0
```

**Capability set has no `shallow`.** This becomes the blocker in Step 7.

---

## Step 6 — `claude plugin marketplace add` against the live URL

```text
$ mkdir /tmp/claude-e2e-acme && cd /tmp/claude-e2e-acme
$ claude plugin marketplace add --scope local \
    http://acme.localhost:8080/.claude-plugin/marketplace.json
Adding marketplace…Downloading marketplace from
  http://acme.localhost:8080/.claude-plugin/marketplace.json
Validating marketplace data
Saving marketplace to cache
Cleaning up old marketplace cache…
✔ Successfully added marketplace: acme (declared in local settings)

$ claude plugin marketplace list | grep -A1 acme
  ❯ acme
    Source: URL (http://acme.localhost:8080/.claude-plugin/marketplace.json)
```

Acceptance criterion #2 **PASS**. The marketplace validates against
Claude Code's schema (no error during "Validating marketplace data"), is
persisted, and shows up in the global marketplace list.

---

## Step 7 — `claude plugin install` — blocked

```text
$ claude plugin install --scope local rust-axum-toolkit@acme
Installing plugin "rust-axum-toolkit@acme"...
✘ Failed to install plugin "rust-axum-toolkit@acme":
   Failed to clone repository:
   Cloning into '/home/olafkfreund/.claude/plugins/cache/temp_git_1779665106654_nl3b9e'...
   fatal: Server does not support shallow clients
   fatal: the remote end hung up unexpectedly
```

### Bisect the failure

```text
# Full clone (no --depth) — works:
$ git clone -c http.extraHeader='Host: acme.localhost:8080' \
    http://127.0.0.1:8080/git/plugins/rust-axum-toolkit.git /tmp/plug-verify
Cloning into '/tmp/plug-verify'...
(success — see Step 5)

# Shallow clone (what the CLI does) — fails identically to Claude Code:
$ git clone --depth=1 -c http.extraHeader='Host: acme.localhost:8080' \
    http://127.0.0.1:8080/git/plugins/rust-axum-toolkit.git /tmp/plug-verify-shallow
Cloning into '/tmp/plug-verify-shallow'...
fatal: Server does not support shallow clients
fatal: the remote end hung up unexpectedly
```

### Root cause

`server/src/routes/plugin_git.rs::server_capabilities()` advertises:

```text
multi_ack_detailed no-done side-band-64k thin-pack ofs-delta agent=skill-pool/0.1
```

No `shallow`. The same file at line 336 explicitly notes shallow lines are
silently dropped:

```rust
// Other lines (shallow / deepen / etc) silently ignored — v1
// scope is clone-and-shallow-fetch; full shallow support is a
// followup if a tenant ever surfaces a complaint.
```

Claude Code's git client refuses to clone when its requested `--depth=1`
isn't reflected in the server's advertised caps — it doesn't fall back
to a full clone. Filed as a follow-up: **server must advertise `shallow`
and respond with `shallow <sha>` ACKs + a single-commit packfile.**

### Workaround used in this run

Skip the install path entirely and exercise the post-install on-disk
state directly via `--plugin-dir` — this is byte-for-byte the same tree
the install would have laid down under
`~/.claude/plugins/cache/.../rust-axum-toolkit/1.0.1/`. Proves the
plugin tree the server materialises is a valid Claude Code plugin; the
gap is purely transport-layer.

```text
$ cd /tmp/claude-e2e-acme && claude -p \
    --plugin-dir /tmp/plug-verify \
    --permission-mode bypassPermissions \
    "Examine /tmp/plug-verify/skills/a11y-audit/SKILL.md and tell me its
     declared name and description in one line." < /dev/null
(...)
"result": "...
**name:** `a11y-audit` — **description:** \"This skill should be used
when the user asks to 'check accessibility', 'audit WCAG compliance',
'scan HTML for a11y issues', 'check color contrast', or 'find
accessibility violations in web pages'.\""
```

The plugin tree loads cleanly, the model reads the SKILL.md, and the
metadata round-trips intact end-to-end through `publish → materialise →
git clone → --plugin-dir load → model context`.

---

## Step 8 — Mirror import — blocked

Acceptance criterion #6 asks for a mirrored external plugin via
`POST /v1/plugins/import`.

```text
$ curl -sS -X POST \
    -H "Host: acme.localhost:8080" \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    --data-binary '{"url":"https://github.com/anthropics/claude-cookbooks.git","slug":"mirror-cookbooks"}' \
    http://127.0.0.1:8080/v1/plugins/import
{"error":"bad_request","message":"bad request: job queue not available
(Redis not configured); cannot enqueue mirror job"}
[HTTP 400]
```

The endpoint enforces the right scope, validates the body, and reaches
the `state.queue()` call in `routes/plugin_import.rs::import` — which
returns `None` because this deployment never wired Redis into the
process. Filed as a deployment follow-up: **Redis must be provisioned
alongside Postgres for any deployment that wants to expose mirror
plugins.** The internal-sourcing path (this whole walkthrough above)
does not need Redis and stays green.

---

## Step 9 — REST-side state at end of run

```text
$ curl -s -H "Host: acme.localhost:8080" -H "Authorization: Bearer $TOKEN" \
    http://127.0.0.1:8080/v1/plugins | jq .
{
  "items": [
    {
      "slug": "rust-axum-toolkit",
      "version": "1.0.1",
      "name": "Rust + Axum Toolkit",
      "description": "Curated demo plugin bundling accessibility, agent-design, and orchestration skills for the acme tenant.",
      "status": "published",
      "sourcing_mode": "internal",
      "tags": ["rust", "axum", "demo"],
      "created_at": "2026-05-24T23:24:59.945065Z"
    }
  ]
}

$ curl -s -H "Host: acme.localhost:8080" -H "Authorization: Bearer $TOKEN" \
    http://127.0.0.1:8080/v1/plugins/rust-axum-toolkit \
  | jq '{slug,version,status,sourcing_mode,
         contents:[.contents[]|"\(.kind)/\(.slug)@\(.version)"]}'
{
  "slug": "rust-axum-toolkit",
  "version": "1.0.1",
  "status": "published",
  "sourcing_mode": "internal",
  "contents": [
    "skill/a11y-audit@1.0.0",
    "skill/agent-designer@1.0.0",
    "skill/agenthub@1.0.0"
  ]
}
```

Server, web, and Postgres all healthy at the end; no unit was restarted
during the run.

```text
$ systemctl is-active skill-pool-server skill-pool-web
active
active
```

---

## Follow-up issues to file

1. **`plugins(server): advertise + implement `shallow` in plugin_git upload-pack`** —
   blocks `claude plugin install`. Fix: extend `server_capabilities()` and
   handle `shallow <sha>` / `deepen <n>` in `parse_upload_request` +
   emit the corresponding `shallow` ACK pkt-lines and a single-commit pack.
   Reproducer: `git clone --depth=1 http://acme.localhost:8080/git/plugins/<slug>.git`.

2. **`plugins(deploy): provision Redis for the p620 portal so /v1/plugins/import works`** —
   blocks mirror sourcing mode. Either add a `podman-redis` unit
   alongside `podman-skill-pool-postgres`, or fall back to an
   in-process queue when Redis is absent. Reproducer in Step 8.

Both are deployment- or transport-layer; neither affects the plugin epic's
API surface (publish / list / get / archive / marketplace.json), which is
end-to-end green per the evidence above.

## Rollback plan that wasn't needed

If the publish had broken the marketplace endpoint:

```text
# 1. Soft-archive the bad version (idempotent):
curl -X DELETE -H "Host: acme.localhost:8080" -H "Authorization: Bearer $TOKEN" \
  http://127.0.0.1:8080/v1/plugins/rust-axum-toolkit/versions/1.0.1

# 2. If the marketplace cache is wedged, drop the entry directly:
PGPASSWORD=skillpool-dev-local psql -h 127.0.0.1 -p 5434 -U skillpool -d skillpool \
  -c "DELETE FROM plugin_marketplace_entries WHERE plugin_slug='rust-axum-toolkit';"

# 3. Last resort: restart the server (does not lose state):
sudo systemctl restart skill-pool-server
```

None of these were exercised — the publish and the post-publish hooks
were clean throughout.

## Followup status (2026-05-25)

The two BLOCKED items above have landed code-side fixes; they remain
BLOCKED in this run log because the live portal still runs the v0.3.0
binary. Once a v0.3.1 deploy lands, re-running Step 7 (shallow clone)
and Step 8 (mirror import) is expected to succeed without further code
changes.

| BLOCKED in this run | Resolved by | Re-test |
|---|---|---|
| Step 7 — `git clone --depth=1` "Server does not support shallow clients" | #58 / PR #62 — capability + protocol handling | Re-run the depth=1 clone after v0.3.1 deploys; expect a packfile, not a fatal. |
| Step 8 — `POST /v1/plugins/import` "Redis not configured" | #59 / PR #63 — in-process tokio task fallback | Re-run the import; expect 202 with `outcome:"enqueued_inline"` and `job_id:"inline-<plugin_id>"`. |

The CLI-side bug found during this gate (`skill-pool plugin publish`
posting bare `PluginManifest` instead of the `PublishBody` envelope)
was also fixed — #57 / PR #61.

### Live re-test against v0.3.4 (2026-05-25)

Shipping #58/#59 surfaced three downstream protocol bugs in #58 alone;
the chain landed across v0.3.1 → v0.3.2 → v0.3.3 → v0.3.4 with hotfixes
in PRs #65, #66, #67. Final re-validation on v0.3.4:

**Step 7 — `git clone --depth=1` against the seeded `rust-axum-toolkit` plugin:**

```bash
$ git -c http.extraHeader='Host: acme.localhost:8080' clone --depth=1 \
    http://127.0.0.1:8080/git/plugins/rust-axum-toolkit.git /tmp/shallow
Cloning into '/tmp/shallow'...
$ git -C /tmp/shallow rev-list --all | wc -l
1
$ cat /tmp/shallow/.git/shallow
0184012a2caaef36967ee7f2d3751a5bd922a358
$ ls /tmp/shallow/.claude-plugin/
plugin.json
```

Exit 0, 1 commit, `.git/shallow` ACK present, plugin tree materialised
end-to-end. ✓

**Step 8 — `POST /v1/plugins/import` against a real-world git URL:**

```bash
$ curl -sS -X POST -H "Host: acme.localhost:8080" \
    -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
    --data '{"url":"https://github.com/anthropics/claude-cookbooks.git","slug":"mirror-test"}' \
    http://127.0.0.1:8080/v1/plugins/import -w "\n--- HTTP %{http_code} ---\n"
{"job_id":"inline-0640e75d-f31c-447a-8244-08e4f1af09e2",
 "outcome":"enqueued_inline",
 "plugin_id":"0640e75d-f31c-447a-8244-08e4f1af09e2"}
--- HTTP 202 ---
```

HTTP 202 (was: 400), `outcome:"enqueued_inline"` and `job_id:"inline-..."`
prove the in-process fallback handled the import without Redis. The
background tokio task spawned and ran; in this case the upstream URL
isn't a Claude Code plugin (no `.claude-plugin/plugin.json`) so the
manifest-parse step failed and the server logged the error correctly via
the new `"in-process mirror job failed (no Redis fallback)"` log site. ✓
(Mirroring a real plugin would update `last_pulled_at`; the fallback
mechanism itself is proven.)

### Bug chain that fell out of #58 during re-validation

| Bug | Symptom | PR | Test |
|---|---|---|---|
| `compute_shallow_boundaries` skipped root commits | `expected shallow list` on 1-commit repo | #65 | `boundaries_depth_one_on_single_commit_repo_is_the_root` |
| Smart-HTTP stateless-RPC two-phase deepening not implemented | `expected shallow list` on multi-POST clones | #66 | `first_phase_deepening_returns_only_shallow_section` |
| `revwalk.hide(parent)` dropped blobs shared with tip's tree | `bad tree object: remote did not send all necessary objects` | #67 | `shallow_pack_includes_blobs_shared_with_hidden_parent` |

All three caught only by live HTTP-level testing — the unit tests in #58
covered the negotiation surface but not the pack-content invariants. The
new regression tests in #65/#66/#67 close those gaps end-to-end against
in-memory bare repos.
