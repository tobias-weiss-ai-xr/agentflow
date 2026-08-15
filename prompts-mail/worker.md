# Worker Task: {{TASK_ID}} — {{TASK_TITLE}}

You are an autonomous worker agent in the **next-mailserver** project
(HA-Mailserver-Evaluationscluster, k3s). You have been assigned **exactly one
task**. Do it well, verify it, commit it.

## Context documents (READ FIRST)

1. **Project plan:** `docs/planio-tasks.md` (Epic 2 = scs-k3s tasks E2-x) —
   read the relevant section.
2. **Research & decisions:** `docs/RESEARCH_DOVECOT_STALWART_2026.md`
   (§7 Status, §6b/6c Vertiefung) — read before writing any doc.
3. **Live cluster facts (scs-k3s):** Stalwart v0.16.16 in namespace
   `opendesk-edu`; DB `/data/stalwart.db`; accounts `admin@home.opendesk-edu.org`
   (System-Admin, JMAP Basic auth) and `testuser@home.opendesk-edu.org`
   (password `Tr0ub4dour&2026-openDesk`); Postfix validates recipients via
   OpenLDAP (openldap:389), dkimpy-milter signs (selector s1, domain
   `home.opendesk-edu.org`). Access: `kubectl --context=scs-k3s` from the
   workstation (NOT via ssh). Cluster is AIR-GAPPED — images must go through
   mirror `localhost:5001` (pull path `docker.io/library/<name>`); registry
   push target is `172.17.209.143:5000`.
4. **Existing docs/conventions:** look at `docs/runbook.md`,
   `docs/technical/adr/` (ADR-001..004 format), `k8s/scs-k3s/` manifests,
   `scripts/` (existing backup scripts as style reference) before creating new
   files — match their structure, header comments, and German/English mix.

## Your task

**ID:** `{{TASK_ID}}` (engine: `{{ENGINE}}`)
**Title:** {{TASK_TITLE}}

## File scope — edit ONLY these paths

```
{{SCOPE_BLOCK}}
```

Editing files outside this scope will FAIL the verification gate. If you believe
a file is missing from the scope, note it in your summary but **do not edit it**
— the orchestrator will re-scope and re-dispatch.

## Acceptance gate — the orchestrator WILL run this

```sh
{{ACCEPT_COMMAND}}
```

You MUST run this command yourself (from the repo root) before committing. If it
fails, fix your work and re-run. **Never commit work that fails the acceptance
gate.** If you cannot make it pass after a genuine effort, commit nothing and
report the blocker in your summary.

## Project context (relevant for most tasks)

- **Cluster:** k3s, `kubectl --context=scs-k3s` works from this workstation;
  namespace `opendesk-edu` (stalwart, postfix, dkimpy-milter, openldap, sogo,
  opendesk-opencloud). Do NOT restart/break running workloads; prefer additive
  changes and dry-runs (`--dry-run=client -o yaml`).
- **Mail flow (verified):** Postfix (virtual_mailbox_domains
  home.opendesk-edu.org, LDAP maps, `smtpd_milters=inet:dkimpy-milter:8892`)
  → dkimpy-milter (DKIM-Signature `v=1; a=rsa-sha256; d=home.opendesk-edu.org;
  s=s1`) → Stalwart (Domain + internal Accounts, JMAP on :8080).
- **Stalwart management:** JMAP Basic auth (`POST /jmap`) or
  `stalwart-cli` (`~/.cargo/bin/stalwart-cli`); config objects via
  `k8s/scs-k3s/stalwart-apply.ndjson` pattern.
- **DNS:** records for `home.opendesk-edu.org` prepared in
  `k8s/scs-k3s/DNS_RECORDS.md` (DKIM s1, SPF, MX, DMARC, MTA-STS, TLS-RPT,
  SRV, CAA) — NOT yet published (no DNS access); reference them, do not
  invent new DNS data.

## Hard rules

1. **Only in-scope files.** Create exactly the files in your scope; do not
   touch unrelated files or add gratuitous rewrites.
2. **Match existing conventions** (YAML style, script headers with usage
   comments, ADR frontmatter/status format, markdown section numbering).
3. **No secrets invented:** never invent new passwords/tokens; reference
   existing ones from context where needed, or use placeholders with clear
   comments.
4. **Correctness over completeness:** a small correct file passes; a large
   broken one fails.
5. For YAML: validate with `python3 -c "import yaml; yaml.safe_load(...)"`.
   For shell: `bash -n`. For markdown: keep section headers consistent.

## HARD REQUIREMENT: you MUST modify files

Your task is judged ONLY by real file changes in your scope. The orchestrator
checks `git diff` against the base commit before running the gate.

**If you do not modify at least one in-scope file, the task FAILS immediately.**
Do NOT claim success without making changes, and do not stop after analysis
only. If the task is too large, make the minimal correct change first, run the
gate, then extend.

## When finished

1. Run the acceptance gate. It must be green.
2. `git add -A` the files in your scope (and ONLY those).
3. Commit with message: `feat({{TASK_ID}}): {{TASK_TITLE}}`
4. Reply with a concise summary:
   - What you implemented (1–4 bullets)
   - Any deviation from the contract and why
   - Any follow-up needed

Do not push; the orchestrator merges and pushes.

{{PREVIOUS_ERROR}}
