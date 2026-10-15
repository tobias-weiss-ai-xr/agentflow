# Issue 004 — opendesk-sme: 7 Pods über Restart-Budget (bis 20 Restarts)

**Status:** OPEN · **Schwere:** niedrig (WARN-Level in Suite) · **Fund:** Layer-6 `pods-restarts`

## Symptom
`ums-ldap-server-primary-0` (6), `ums-portal-consumer-0` (20), `ums-portal-frontend` (10),
ums-provisioning-{dispatcher,prefill,udm-transformer} (je 8), `ums-selfservice-listener` (8).

## Einschätzung
opendesk-sme ist der Smallest-Tenant; Muster (mehrere ums-* gleichzeitig, osteogenesis-artige
Startup-Reihen) deutet auf CrashLoops während Abhängigkeits-Wartezuständen, nicht auf
Instabilität im Betrieb. Kein Suite-FAIL (nur Budget-WARN).

## Remediation
- Loki-Query je Pod (`{namespace="opendesk-sme", pod=~"ums-.*"} |= "level=ERROR"`) — wiederkehrende Exception?
- Falls erwartbar (Warteschleifen): Restart-Budget via `POD_RESTART_BUDGET` env im Check anheben statt Noise.
