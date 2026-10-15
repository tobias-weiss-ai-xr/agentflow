# Issue 002 — k8up-Backups: fehlende Backup-Quellen (PreBackupPods/Annotations), teils Failed-Läufe

**Status:** FIXED 2026-10-15 (3 kumulierte Ursachen, alle behoben + live verifiziert)

## Auflösung (2026-10-15)
Die Failed/Hanging-Läufe hatten **drei** Ursachen, die sich überlagerten:
1. **Kein Backup-Job-Image**: Der k8up-Operator hatte `BACKUP_IMAGE=172.25.24.36:5002/openedusuite-nix/k8up:2.13.1-nix` — das nix-Image hat die Binary NICHT unter `/usr/local/bin/k8up` → jeder Backup-/Check-/Prune-Pod crashte im RunContainerError (clusterweit, seit ~10.11!). Fix: `BACKUP_IMAGE=172.17.0.6:5001/k8up-io/k8up:v2.13.1` (node-lokaler Registry-Mirror) in `ansible-baremetal-k3s/k8s/backup/k8up-operator.yaml` (Commit 30dd79a). Operator-Container selbst bleibt auf nix-Image (funktioniert).
2. **ResourceQuota ohne resourceRequirementsTemplate**: staff-quota/students-quota erzwingen requests/limits; die live-managed Schedules von opendesk-staff/-students hatten kein `resourceRequirementsTemplate` (die anderen 14 hatten es) → Job-Pods mit "failed quota" rejected, CRs hingen im Progressing. Fix: Live-Patch der beiden Schedules (500m/256Mi limits, 100m/128Mi requests — identisch zum Repo-Template k8s/opendesk-staff/k8up-schedule.yaml).
3. **Fehlende PVC-Annotations**: agentflow-Campaign `oeu-health` (Task oeu-k8up-coverage) hat alle PVCs annotiert (`k8up.io/backup: 'true'`) + `scripts/check-backup-coverage.sh` (241 Zeilen) als Gate/Werkzeug (openEduSuite-hrz Commit 6509a98b). Achtung: STS-volumeClaimTemplate-Annotationen sind nicht updatebar → galera via ignoreDifferences + direkte PVC-Annotationen gelöst.

**Verifikation 2026-10-15:** Manual-Backup-CRs in opendesk-staff + opendesk-students → `Completed=True Succeeded`; Check-CR (restic repo scrub/verify) → Succeeded; Layer-6-Suite: `backups 16/16 Completed — PASS`. · **Schwere:** hoch · **Fund:** Layer-6 `backups` (XFAIL-entry aktiv), Session-Diagnose 2026-10-14

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
