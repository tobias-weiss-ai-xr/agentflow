# Issue 006 — Invoicing-Polish: Locale/TZ, DMARC-rua-Mailbox

**Status:** OPEN · **Schwere:** kosmetisch/niedrig · **Fund:** Session 2026-10-14

1. `deploy-invoiceninja.yaml`: `APP_LOCALE=en`, `TZ=UTC` wirken im Container trotz env-Intention de/Berlin
   (Laravel-Cache oder fehlende env-Verkabelung — via tinker verifizieren, Cache-Clear ergänzen).
2. DMARC `rua=postmaster@openedusuite.org`: Mailbox existiert nicht → Reports laufen ins Leere.
   Reproduzierbar lösen: `bootstrap-mail-stack.sh` um Abschnitt „postmaster@-Mailbox (DMARC-rua)" erweitern
   (Pattern wie invoicing@: JMAP-Account + AppPassword + optional k8s-Secret), live anwenden.

## Remediation
- Repo-Anteil (Campaign): TZ/Locale-Fix im Deployment + Bootstrap-Abschnitt postmaster@.
- Live-Anteil (Operator): Bootstrap-Lauf + `bootstrap-mail-stack.sh --check` grün + Layer-6 `mail-config` PASS.
