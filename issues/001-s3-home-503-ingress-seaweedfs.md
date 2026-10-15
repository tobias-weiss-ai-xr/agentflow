# Issue 001 — s3.home.openedusuite.org antwortet 503 (Ingress ↔ SeaweedFS-S3 blockiert)

**Status:** FIXED 2026-10-15

## Auflösung
Root Cause: Label-Mismatch in der NetworkPolicy `seaweedfs` (ns backup) — die
Regel erlaubte kube-system-Pods mit `app.kubernetes.io/name: haproxy-ingress`,
die Pods tragen aber `app.kubernetes.io/name: kubernetes-ingress`. Die Regel
matchte NIE; der k3s-NetPol-REJECT antwortet mit RST → haproxy sah
"connection refused" (deshalb 503 statt Timeout). Fix im GitOps-Repo
ansible-baremetal-k3s, Commit 76bdb65; ArgoCD selfHeal übernommen.

**Verifikation:** curl auf s3.home.openedusuite.org → 403 (reachable) via
HTTP und HTTPS; Layer-6-Suite ohne s3-XFAIL: XFAIL=0.

## Symptom
- Blackbox-Probe + Workstation-Curl: `https://s3.home.openedusuite.org/` → 503 (haproxy)
- haproxy-ingress Log: `Server backup_svc_seaweedfs-s3_http/SRV_1 is DOWN, reason: Layer4 connection problem, "Connection refused"` → `backend has no server available`

## Diagnose (2026-10-15)
| Vantage | Ziel | Ergebnis |
|---|---|---|
| kube-system / haproxy-ingress-Pod | Pod-IP 172.17.128.163:8333 | **connection refused** |
| openedusuite (invoiceninja-Pod) | dieselbe Pod-IP | TCP-OK (403 vom Server = erreichbar) |
| monitoring (grafana-Pod) | dieselbe Pod-IP | HTTP 403 (erreichbar) |

- SeaweedFS-S3-Pod lauscht auf `:::8333` (tcp6, `bindv6only=0`), Endpoint ready, Service-Regeln ok.
- NetworkPolicy `backup/seaweedfs` allowlistet kube-system ausdrücklich — verhindert aber trotzdem den Pfad vom Ingress-Pod.
- k3s-Bord-NetPol (kube-router-artig) implementiert Deny offenbar als RST (refused statt drop).
- Verdacht: Quell-Traffic des Ingress-Controllers wird policy-mäßig nicht der kube-system-Identität zugeordnet (SNAT/hostNetz-Äquivalent), oder v6-only-Binding + v4-Pfad-Besonderheit im kube-system-Netzwerksegment.

## Remediation (Vorschläge, aufsteigend invasiv)
1. NetworkPolicy `backup/seaweedfs` um `ipBlock` für die Pod-CIDR-Quelle des Ingress-Pod erweitern (Test: `kubectl exec -n kube-system deploy/haproxy-ingress-kubernetes-ingress -- wget -T3 http://172.17.128.163:8333/`).
2. Alternativ: SeaweedFS-S3-Deployment `AF_INET`-Binding erzwingen (env/args) oder hostPort.
3. Alternativ: eigenes Ingress-Backend in ns `backup` betreiben.

## Verknüpfungen
- Erklärt vermutlich Teil der k8up-Failures (Issue 002) — S3 ist das restic-Backend.
- Suite-Entry: `EXPECTED_FAILURES="vhost-https://s3.home.openedusuite.org/"` entfernen, sobald grün.
