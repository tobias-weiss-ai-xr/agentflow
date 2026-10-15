# Issue 003 — cloud.umr.uni-marburg.de nicht mehr auflösbar

**Status:** OPEN — extern (HRZ); lokal nichts fixbar

## Befund (2026-10-15)
DNS löst cloud.umr.uni-marburg.de weder von der Workstation noch in-cluster
auf (NXDOMAIN/timeout in blackbox-Probe, code=000). Kein lokaler Resolver-
Fehler: mehrere Quellen bestätigen. Das Vhost ist Teil der Legacy-HRZ-Domain-
Ära. Eskalation: HRZ-Service-Desk ticketen (DNS-Eintrag prüfen). Bleibt als
einziger FAIL in der Layer-6-Suite, bis HRZ es fixt.

## Symptom
- `getent hosts cloud.umr.uni-marburg.de` → leer (Workstation, 2026-10-15)
- In-cluster Blackbox-Probe: `probe_success=0`, curl `000` (kein Connect — DNS hängt vorgelagert)
- `cloud.openedusuite.org` selbst ist gesund (301 → cloud.umr, Redirect als gesund gewertet)

## Einschätzung
Externer Dienst der HRZ; DNS-Eintrag offenbar gezogen/geändert. Kein openEduSuite-seitiger Fix möglich — Tracking, damit der Probe-Alarm (AppProbeDown-Regel) nicht als Dauer-Rauschen endet.

## Remediation
- HRZ kontaktieren (DNS-Änderung? Migration?).
- Suite: `EXPECTED_FAILURES`-Entry erst setzen, wenn HRZ den Zustand bestätigt (aktuell bewusst NICHT allowlistet — der FAIL ist ehrlich).
