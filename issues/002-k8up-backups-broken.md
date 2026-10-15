# Issue 002 — k8up-Backups: fehlende Backup-Quellen (PreBackupPods/Annotations), teils Failed-Läufe

**Status:** OPEN · **Schwere:** hoch · **Fund:** Layer-6 `backups` (XFAIL-entry aktiv), Session-Diagnose 2026-10-14

## Symptom
- 14/16 Namespaces mit erwarteten Backups: letzter Backup-Lauf `Completed=True`
- opendesk-staff + opendesk-students: letzte Läufe `Failed` (`NoPreBackupPodsFound`)
- Clusterweit existieren (fast) keine `k8up.io/v1` Backup-Annotationen/PreBackupPods → nur PVC-only-NS sichern überhaupt
- `check`-Jobs gegen das Restic-Repo: keine erfolgreichen (Repo-Integrität ungeprüft)

## Ursachenkette
1. Schedules (k8up v2, `k8up.io/v1`) laufen, aber ohne `k8up.io/backup`-Annotation an Pods oder PreBackupPods gibt es nichts zu sichern.
2. Das Restic-Backend liegt auf SeaweedFS-S3 (`s3.home.openedusuite.org`, ns backup) — dort aktuell 503 am Ingress-Pfad (Issue 001). Direkter Svc-Pfad aus ns backup ist von der Policy erlaubt; ob k8up den Ingress- oder Svc-Pfad nutzt, ist zu verifizieren.

## Remediation
- Repo-Seitig (Campaign): `k8up.io/backup`-Annotationen + ggf. PreBackupPods für die datentragenden Deployments/StatefulSets je Namespace (opendesk*, openedusuite, idm, sogo5, …) in `openEduSuite-hrz/k8s/…` ergänzen; `kustomize build` als Gate.
- Live-Seitig (Operator): ArgoCD-Sync abwarten, manuellen Backup-CR anstoßen, Snapshot im Repo verifizieren; danach Suite-Entry `EXPECTED_FAILURES="backups"` entfernen.
- k8up check-Job gegen das Repo einmal manuell laufen lassen.

## Verknüpfungen
- Issue 001 (S3 503) — Backend-Verfügbarkeit.
