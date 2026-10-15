# Issue 007 — Galera: root@127.0.0.1 ohne Passwort (lokal)

**Status:** FIXED 2026-10-15 (Operator-Runbook ausgeführt)

## Auflösung
- root@localhost → **unix_socket-Plugin** (Sockel-Peer-Auth; die Liveness/
  Readiness-Probes nutzen genau diesen Pfad und bleiben funktionsfähig!)
- root@127.0.0.1 / root@::1 / root@mariadb-galera-0 → echtes Passwort
  (28 Zeichen, generiert), Platzhalter im Secret `galera-secrets:
  mariadb-root-password` ersetzt (patch)
- Angewendet auf mariadb-galera-0, via wsrep repliziert

**Verifikation:** wsrep_cluster_size=3 + wsrep_ready=ON auf allen 3 Nodes;
TCP-root mit Passwort ✓, ohne Passwort → Access denied ✓; Probes grün
(alle Pods 1/1 Ready). Kuriosum: ALTER USER auf root@mariadb-galera-0
lief 2× in ERROR 1396 trotz existierendem Account → via DROP+CREATE+GRANT
gelöst.

## Remediation
- Harm: Angriffsfläche nur für Prozesse im DB-Pod selbst (127.0.0.1). Trotzdem: root-Passwort setzen
  oder root via unix_socket/auth_socket binden; Rollout über alle 3 Galera-Pods nacheinander
  (kein gleichzeitiger Reindex), danach Health-Check der Cluster-Größe (`wsrep_cluster_size=3`).
- Nicht worktree-tauglich (Live-DB-Op) — vom Operator per Runbook auszuführen.
