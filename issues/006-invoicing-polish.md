# Issue 006 — Invoicing-Polish: Locale/TZ, DMARC-rua-Mailbox

**Status:** FIXED 2026-10-15

## Auflösung
- TZ-Wiring: Diagnose im Campaign-Task `oeu-invoicing-polish` präzisiert — APP_LOCALE/APP_TIMEZONE standen bereits richtig, es fehlte das **OS-level TZ** (Image defaultet UTC) → `TZ=Europe/Berlin` auf beide Container (invoice-ninja + scheduler), Commit 7c8a5e91. Live verifiziert: `date` = CEST, `TZ=Europe/Berlin APP_LOCALE=de` im Container.
- postmaster@: Bootstrap-Abschnitt 4/4 (JMAP-Account + AppPassword + Secret `postmaster-secrets:app-password`, skip-if-exists, --check-Integration) — live gelaufen, `--check` grün. DMARC-rua Zustellung damit möglich. · **Schwere:** kosmetisch/niedrig · **Fund:** Session 2026-10-14

1. `deploy-invoiceninja.yaml`: `APP_LOCALE=en`, `TZ=UTC` wirken im Container trotz env-Intention de/Berlin
   (Laravel-Cache oder fehlende env-Verkabelung — via tinker verifizieren, Cache-Clear ergänzen).
2. DMARC `rua=postmaster@openedusuite.org`: Mailbox existiert nicht → Reports laufen ins Leere.
   Reproduzierbar lösen: `bootstrap-mail-stack.sh` um Abschnitt „postmaster@-Mailbox (DMARC-rua)" erweitern
   (Pattern wie invoicing@: JMAP-Account + AppPassword + optional k8s-Secret), live anwenden.

## Remediation
- Repo-Anteil (Campaign): TZ/Locale-Fix im Deployment + Bootstrap-Abschnitt postmaster@.
- Live-Anteil (Operator): Bootstrap-Lauf + `bootstrap-mail-stack.sh --check` grün + Layer-6 `mail-config` PASS.
