# Issue 003 — cloud.umr.uni-marburg.de nicht mehr auflösbar

**Status:** OPEN (extern) · **Schwere:** mittel · **Fund:** Layer-6 `vhost-https://cloud.umr.uni-marburg.de/` (Suite-FAIL)

## Symptom
- `getent hosts cloud.umr.uni-marburg.de` → leer (Workstation, 2026-10-15)
- In-cluster Blackbox-Probe: `probe_success=0`, curl `000` (kein Connect — DNS hängt vorgelagert)
- `cloud.openedusuite.org` selbst ist gesund (301 → cloud.umr, Redirect als gesund gewertet)

## Einschätzung
Externer Dienst der HRZ; DNS-Eintrag offenbar gezogen/geändert. Kein openEduSuite-seitiger Fix möglich — Tracking, damit der Probe-Alarm (AppProbeDown-Regel) nicht als Dauer-Rauschen endet.

## Remediation
- HRZ kontaktieren (DNS-Änderung? Migration?).
- Suite: `EXPECTED_FAILURES`-Entry erst setzen, wenn HRZ den Zustand bestätigt (aktuell bewusst NICHT allowlistet — der FAIL ist ehrlich).
