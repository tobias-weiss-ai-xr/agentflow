# Issue 005 — 36 stale *.suite.graphwiz.ai redirectUris an sogo5/sogo6-Keycloak-Clients

**Status:** FIXED 2026-10-15

## Auflösung
- Script-Modus `--prune-redirect-uris REGEX [--apply]` via Campaign-Task `oeu-kc-prune-redirects` implementiert (Commit 9d34d282, dry-run default).
- Live: `--prune-redirect-uris 'suite\.graphwiz\.ai' --apply` → 26 Clients bereinigt; Kontrolle dry-run: 0 Restfunde.
- Regression: `pw-us46-sogo6mailapi.mjs` 7/7 passed. · **Schwere:** niedrig · **Fund:** Session 2026-10-14 (create-keycloak-clients-sso.sh kennt den Arg-Filter, implementiert ihn aber nicht)

## Symptom
sogo5/sogo6-Clients (anderer Provisionierungspfad als SERVICES-Liste) tragen noch
`https://*.suite.graphwiz.ai/...`-Callback-URIs (alte Domain).

## Remediation
- Script-seitig (Campaign): `create-keycloak-clients-sso.sh` um `--prune-redirect-uris REGEX`-Modus
  erweitern (List+PUT via Admin-REST, dry-run default).
- Live-seitig (Operator): Modus gegen realm opendesk ausführen, via Keycloak-Admin-Token
  (keycloak-db secret, master-realm), danach `pw-us46-sogo6mailapi.mjs` (7/7) als Regression.
