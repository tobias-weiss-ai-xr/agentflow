# Issue 005 — 36 stale *.suite.graphwiz.ai redirectUris an sogo5/sogo6-Keycloak-Clients

**Status:** OPEN · **Schwere:** niedrig · **Fund:** Session 2026-10-14 (create-keycloak-clients-sso.sh kennt den Arg-Filter, implementiert ihn aber nicht)

## Symptom
sogo5/sogo6-Clients (anderer Provisionierungspfad als SERVICES-Liste) tragen noch
`https://*.suite.graphwiz.ai/...`-Callback-URIs (alte Domain).

## Remediation
- Script-seitig (Campaign): `create-keycloak-clients-sso.sh` um `--prune-redirect-uris REGEX`-Modus
  erweitern (List+PUT via Admin-REST, dry-run default).
- Live-seitig (Operator): Modus gegen realm opendesk ausführen, via Keycloak-Admin-Token
  (keycloak-db secret, master-realm), danach `pw-us46-sogo6mailapi.mjs` (7/7) als Regression.
