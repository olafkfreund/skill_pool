# Capture an insight mid-session

You just spent 40 minutes wrestling with an axum lifetime issue. You
finally understand it. Don't lose it.

Three ways to capture, depending on how busy you are.

## Option 1 — explicit, right now

```bash
mkdir captured-lesson
cd captured-lesson
$EDITOR SKILL.md       # write what you learned

skill-pool capture ./ \
  --notes "Found while wiring SCIM list endpoint — see PR #42"
```

That lands in the **Drafts inbox** under the `acme` tenant. A curator
publishes it (assigning a version) or discards it. You keep working.

## Option 2 — explicit, in the assistant

If Claude is the one who figured it out, ask in-conversation:

> Please remember this for next time.

The Stop hook scorer recognises explicit markers ("remember this",
"TIL:", `/capture-skill`) and bumps the session score to 1000+ — well
past the draft threshold. The next time the capturer daemon runs (every
hour by default), it'll draft a SKILL.md from the transcript and POST
it as a draft.

To install the hook:

```bash
skill-pool hook-install --with-scorer
```

Now every assistant turn writes a deterministic score to
`~/.skill-pool/sessions/<id>.json`. No LLM in this pass — just regex on
explicit markers + counting failed Edits and test-recovery patterns.

## Option 3 — implicit, hands-off

You don't have to say anything. The scorer also fires on signals you
don't notice in the moment:

| Signal | When it fires |
|---|---|
| Test recovery | `cargo test` failed ≥2× then passed |
| Edit retries | >3 failed `Edit`/`Write` on the same file |
| Cross-session recurrence | same failing fingerprint in 3+ sessions |
| Novel command | failed Bash stem not in your shell history |
| Long session | >20 assistant turns on one task |

If your session crosses the threshold (default 100 points), the
capturer daemon runs the two-stage LLM pipeline (Haiku gate → Sonnet
drafter) and a draft lands in the inbox.

Inspect what's been scored:

```bash
skill-pool capture-status
# 12 sessions scored (3 ≥ draft threshold of 100)
#
#   SCORE TURNS          CWD                                      SESSION
#  ★1050  3              /proj/auth                               axum-tip…
#         ↳ explicit_marker: user said `remember this`
#  ★ 130  18             /proj/sso                                scim-debug…
#         ↳ test_recovery: `cargo test` failed 3× then passed
```

## The review side

Whichever capture path fired, a curator opens **Catalog → Drafts** in the
portal, reads the proposed SKILL.md, and either:

- **Publishes** (assigns a version → it joins the catalog)
- **Discards** (it's kept for telemetry, hidden from the default view)

If the new draft is cosine-similar to an existing skill (≥ 0.85), the
inbox shows an amber **"Looks like *foo* (94% match)"** badge — merge
candidate, not a fresh skill.

## Gotchas

- **Empty `~/.bash_history`** means novel-command can't fire — there's
  nothing to compare against. The scorer stays silent rather than
  flagging everything.
- The capturer needs `ANTHROPIC_API_KEY` set. If you forget, it logs the
  failure and continues — your session score still persists.
- **Cost shape.** Haiku gate is cents per session. Sonnet drafter only
  fires on the ~30% Haiku approved. Set `--limit 5` on the capturer
  service if you want a tighter cost cap.
