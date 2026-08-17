#!/usr/bin/env bash
# Post to opencode.de Discourse: Civitas Core - Small Town Edition
# Usage: ./post-discourse.sh [--dry-run]

set -euo pipefail

THREAD_URL="https://discourse.opencode.de/t/civitas-core-ideas-for-a-small-town-edition-ste/5648"
POST_FILE="/tmp/discourse-post-opencode.md"

# Content to post
cat > "$POST_FILE" << 'EOF'
Hallöchen aus Marburg,

wir nutzen **openDesk Edu** erfolgreich an der **Philipps-Universität** und haben dabei eine **modulare Architektur** entwickelt, die **perfekt für kleinere Kommunen** skalierbar ist.

## Warum openDesk Edu für Kleinstädte passt:

- **Modular**: Nur installieren, was gebraucht wird
- **Kubernetes-Native**: Helmfile/Helm für bessere Wartung
- **SSO-Integration**: Keycloak (SAML 2.0 / OIDC)
- **Automatisierte Deployments**: ArgoCD

## Beispiel: Minimale Installation für 5.000 Einwohner

- 5 Kernservices (Keycloak, Portal, SOGo, OpenCloud, PostgreSQL)
- Gesamt: ~5.5 CPU / 9.5 GB RAM
- Läuft auf 2-3 Servern
- Kosten: ~5.500 €/Jahr (vs. 20.000-50.000 € für proprietäre Lösungen)

## Deployment-Optionen:

- **K3s + helmfile** (empfohlen)
- **Docker Compose** (einfach)
- **Ansible** (Integration)

Wir können eine **vorgefertigte Konfiguration** bereitstellen. Bei Interesse gerne melden!

---
*Tobias Weiß, Philipps-Universität Marburg*
EOF

echo "Post content saved to: $POST_FILE"
echo ""
echo "To post manually:"
echo "1. Visit: $THREAD_URL"
echo "2. Click 'Reply'"
echo "3. Paste the content from $POST_FILE"
echo "4. Click 'Post Reply'"
echo ""
echo "Thread verified: $(curl -s -o /dev/null -w '%{http_code}' "$THREAD_URL" 2>/dev/null)"

if [[ "${1:-}" != "--dry-run" ]]; then
    echo ""
    echo "Opening browser..."
    xdg-open "$THREAD_URL" 2>/dev/null || echo "Browser open failed. Please visit manually: $THREAD_URL"
fi
