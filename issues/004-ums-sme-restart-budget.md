# Issue 004 — opendesk-sme: 7 Pods über Restart-Budget (bis 20 Restarts)

**Status:** FIXED 2026-10-15 (Baseline-Reset nach Root-Cause)

## Auflösung
Alle Crashes historisch (25d+; letzter 2026-10-13): Univention-UMS-Consumer
(upsream-Design) sterben auf transiente 500er von ums-provisioning-api statt
mit Backoff zu retryen; außerdem alte Rollout-Events. Aktuell: 0 Fehler in
48h, Long-Poll-Subscriptions sauber 200. Nach Root-Cause Counter-Reset
(rollout restart der 5 Deployments + Pod-Delete der 2 STS-Pods) → alle
ums-sme-Counter ≤ 2 (Budget 5). Monitoring-Signal wieder scharf.

**Offen (upstream):** Crash-on-5xx-Verhalten der Consumer — nur via
Univention upstream behebbar, hier nur symptomlos umgangen.

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
