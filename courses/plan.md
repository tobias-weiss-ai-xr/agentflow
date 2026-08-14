# Courses Learning Material Generation Plan

**Date:** 2026-08-15
**Project:** courses (https://github.com/tobias-weiss-ai-xr/courses)
**Goal:** Generate comprehensive learning material for all 27 Microsoft certification exams

## Current State

| Content Type | Azure | M365 | Power Platform | Security | Dynamics | AI | Total |
|---|---|---|---|---|---|---|---|
| Exam Guides ✅ | 13 | 3 | 4 | 3 | 5 | 1 | **29** (includes pre-existing AZ-900/104/305) |
| Deep Dives ❌ | 0 | 0 | 0 | 0 | 0 | 0 | **0** |
| Practice Questions | 3* | 0 | 0 | 0 | 0 | 0 | **3** |

*AZ-900, AZ-104, AZ-305 already have practice questions. AZ-305 only has 13 questions.

## Content Model (based on LPIC gold standard)

Each certification exam gets 3 content types:

### 1. Exam Guide (`*-exam-guide.md`) — ✅ ALREADY DONE
- Frontmatter: title, description, date, tags, slug, categories, draft
- Exam overview table (code, level, questions, length, passing score, price, prerequisites)
- Audience profile
- Domain breakdown with weights
- Skills measured bullets
- Study resources + official link
- Study tips

### 2. Deep Dive (`*-deep-dive.md`) — ❌ NEEDS CREATION (27 tasks)
- Objective-by-objective breakdown with **concrete examples**
- CLI commands / PowerShell commands / portal instructions
- Configuration file excerpts
- Architecture diagrams (described in text/ASCII)
- Comparison tables (e.g., "Storage replication options")
- Exam tips and "gotchas" per objective
- Target: **500–1200 lines** depending on exam complexity

### 3. Free Practice Questions (`*-free-practice-questions.md`) — ❌ NEEDS CREATION (24 tasks)
- **50 questions** minimum (for Fundamentals: 40, for Expert: 60)
- 4 answer choices per question (a/b/c/d)
- Detailed explanations with correct answer highlighted
- `<details><summary>Show Answer</summary>` HTML toggle format
- Questions distributed proportionally across exam domains
- Target: **800–1200 lines**

## Exam Inventory (27 Microsoft exams)

### Azure (13 exams)

| # | Code | Title | Level | Deep Dive | Practice Qs |
|---|---|---|---|---|---|
| 1 | AB-731 | AI Transformation Leader | Specialty | ❌ | ❌ |
| 2 | AI-900 | Azure AI Fundamentals | Fundamentals | ❌ | ❌ |
| 3 | AI-102 | Azure AI Engineer | Associate | ❌ | ❌ |
| 4 | AZ-900 | Azure Fundamentals | Fundamentals | ❌ | ✅ (50) |
| 5 | AZ-104 | Azure Administrator | Associate | ❌ | ✅ (50) |
| 6 | AZ-204 | Azure Developer | Associate | ❌ | ❌ |
| 7 | AZ-305 | Azure Solutions Architect | Expert | ❌ | ✅ (13, needs more) |
| 8 | AZ-400 | Azure DevOps Engineer | Expert | ❌ | ❌ |
| 9 | AZ-500 | Azure Security Engineer | Associate | ❌ | ❌ |
| 10 | AZ-700 | Azure Network Engineer | Associate | ❌ | ❌ |
| 11 | AZ-801 | Windows Server Hybrid Admin | Expert | ❌ | ❌ |
| 12 | DP-900 | Azure Data Fundamentals | Fundamentals | ❌ | ❌ |
| 13 | DP-203 | Azure Data Engineer | Associate | ❌ | ❌ |
| 14 | DP-300 | Azure Database Admin | Associate | ❌ | ❌ |
| 15 | DP-600 | Fabric Analytics Engineer | Associate | ❌ | ❌ |

### Microsoft 365 (3 exams)

| # | Code | Title | Level |
|---|---|---|---|
| 16 | MS-900 | Microsoft 365 Fundamentals | Fundamentals |
| 17 | MS-102 | Microsoft 365 Administrator | Associate |
| 18 | MS-700 | Managing Microsoft Teams | Associate |

### Power Platform (4 exams)

| # | Code | Title | Level |
|---|---|---|---|
| 19 | PL-900 | Power Platform Fundamentals | Fundamentals |
| 20 | PL-200 | Power Platform Functional Consultant | Associate |
| 21 | PL-400 | Power Platform Developer | Associate |
| 22 | PL-600 | Power Platform Solution Architect | Expert |

### Security (3 exams)

| # | Code | Title | Level |
|---|---|---|---|
| 23 | SC-900 | Security, Compliance & Identity Fundamentals | Fundamentals |
| 24 | SC-300 | Identity & Access Administrator | Associate |
| 25 | SC-400 | Information Protection & Compliance Admin | Associate |

### Dynamics 365 (5 exams)

| # | Code | Title | Level |
|---|---|---|---|
| 26 | MB-210 | Dynamics 365 Sales | Associate |
| 27 | MB-230 | Dynamics 365 Customer Service | Associate |
| 28 | MB-335 | Dynamics 365 Supply Chain Management | Expert |
| 29 | MB-500 | Dynamics 365 Finance & Operations Developer | Associate |
| 30 | MB-910 | Dynamics 365 Fundamentals (CRM) | Fundamentals |

## Task Dependency Graph

```
                    ┌─── DD-AB731 ─── PQ-AB731
                    ├─── DD-AI900 ─── PQ-AI900
                    ├─── DD-AI102 ─── PQ-AI102
                    ├─── DD-AZ900 ─── PQ-AZ900  (practice exists, still generate for completeness?)
                    ├─── DD-AZ104 ─── PQ-AZ104  (practice exists)
                    ├─── DD-AZ204 ─── PQ-AZ204
                    ├─── DD-AZ305 ─── PQ-AZ305  (practice exists, only 13 questions → generate full 60)
Azure ──────────────├─── DD-AZ400 ─── PQ-AZ400
(skin: azure)       ├─── DD-AZ500 ─── PQ-AZ500
                    ├─── DD-AZ700 ─── PQ-AZ700
                    ├─── DD-AZ801 ─── PQ-AZ801
                    ├─── DD-DP900 ─── PQ-DP900
                    ├─── DD-DP203 ─── PQ-DP203
                    ├─── DD-DP300 ─── PQ-DP300
                    └─── DD-DP600 ─── PQ-DP600

M365 ───────────────├─── DD-MS900 ─── PQ-MS900
                     ├─── DD-MS102 ─── PQ-MS102
                     └─── DD-MS700 ─── PQ-MS700

Power Platform ─────├─── DD-PL900 ─── PQ-PL900
                     ├─── DD-PL200 ─── PQ-PL200
                     ├─── DD-PL400 ─── PQ-PL400
                     └─── DD-PL600 ─── PQ-PL600

Security ────────────├─── DD-SC900 ─── PQ-SC900
                     ├─── DD-SC300 ─── PQ-SC300
                     └─── DD-SC400 ─── PQ-SC400

Dynamics ────────────├─── DD-MB210 ─── PQ-MB210
                     ├─── DD-MB230 ─── PQ-MB230
                     ├─── DD-MB335 ─── PQ-MB335
                     ├─── DD-MB500 ─── PQ-MB500
                     └─── DD-MB910 ─── PQ-MB910
```

**Rule:** PQ-{exam} depends on DD-{exam} (practice questions reference deep-dive material).
**Parallelism:** All DD tasks within the same category can run simultaneously.
All PQ tasks can run simultaneously once their DD dependency is met.

## Task Count Summary

| Task Type | Count |
|---|---|
| Deep Dives (DD-*) | 27 |
| Practice Questions (PQ-*) | 27 |
| **Total** | **54** |

(Exams AZ-900, AZ-104, AZ-305 already have practice questions but the deep-dive will improve them; PQ tasks for these will be marked `manual: true` to avoid overwriting existing good content, or we generate fresh and replace.)

## Acceptance Gates

### Deep Dive Gate
```sh
# File must exist, have proper frontmatter, be 500+ lines, and contain code blocks
FILE="content/{category}/{subcategory}/{slug}-deep-dive.md"
test -f "$FILE" && \
  head -1 "$FILE" | grep -q '---' && \
  wc -l < "$FILE" | grep -qP '^\s*[5-9]\d{2}\b|^1\d{3}\b' && \
  grep -c '```' "$FILE" | awk '$1 >= 20 {exit 0} {exit 1}'
```

### Practice Questions Gate
```sh
# File must exist, have frontmatter, contain 40+ questions
FILE="content/{category}/{subcategory}/{slug}-free-practice-questions.md"
test -f "$FILE" && \
  head -1 "$FILE" | grep -q '---' && \
  grep -c '### Question' "$FILE" | awk '$1 >= 40 {exit 0} {exit 1}' && \
  grep -c '<details>' "$FILE" | awk '$1 >= 40 {exit 0} {exit 1}'
```

## File Naming Convention

```
content/{category}/{subcategory}/{slug}-{content-type}.md
```

Examples:
- `content/azure/guides/azure-ai-engineer-deep-dive.md`
- `content/azure/guides/azure-ai-engineer-free-practice-questions.md`
- `content/microsoft/m365/microsoft-365-fundamentals-deep-dive.md`
- `content/microsoft/security/security-compliance-identity-fundamentals-deep-dive.md`
