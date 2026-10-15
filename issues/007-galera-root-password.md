# Issue 007 — Galera: root@127.0.0.1 ohne Passwort (lokal)

**Status:** OPEN · **Schwere:** niedrig (nur localhost-Pfad im Pod) · **Fund:** Session 2026-10-14

## Remediation
- Harm: Angriffsfläche nur für Prozesse im DB-Pod selbst (127.0.0.1). Trotzdem: root-Passwort setzen
  oder root via unix_socket/auth_socket binden; Rollout über alle 3 Galera-Pods nacheinander
  (kein gleichzeitiger Reindex), danach Health-Check der Cluster-Größe (`wsrep_cluster_size=3`).
- Nicht worktree-tauglich (Live-DB-Op) — vom Operator per Runbook auszuführen.
