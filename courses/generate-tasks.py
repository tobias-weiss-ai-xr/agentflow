#!/usr/bin/env python3
"""
generate-courses-tasks.py — Emit tasks.json for courses learning material generation.

Reads: plan metadata (inline)
Writes: ../config/tasks.json (for courses repo)

Each Microsoft exam gets 2 tasks:
  DD-{SLUG}  — Deep dive guide (objective-by-objective with examples)
  PQ-{SLUG}  — Free practice questions (40-60 Q&A with explanations)

PQ depends on DD (practice questions should reference deep-dive content).
"""

import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "config" / "tasks-courses.json"

EXAMS = [
    # Azure exams
    {"id": "AB731",  "slug": "ai-transformation-leader",          "code": "AB-731", "title": "AI Transformation Leader",                      "category": "microsoft/ai",            "level": "Specialty",    "has_pq": False},
    {"id": "AI900",  "slug": "azure-ai-fundamentals",             "code": "AI-900", "title": "Azure AI Fundamentals",                        "category": "azure/guides",            "level": "Fundamentals", "has_pq": False},
    {"id": "AI102",  "slug": "azure-ai-engineer",                 "code": "AI-102", "title": "Azure AI Engineer",                            "category": "azure/guides",            "level": "Associate",   "has_pq": False},
    {"id": "AZ900",  "slug": "azure-fundamentals",                "code": "AZ-900", "title": "Azure Fundamentals",                           "category": "azure/guides",            "level": "Fundamentals", "has_pq": True},
    {"id": "AZ104",  "slug": "azure-administrator",               "code": "AZ-104", "title": "Azure Administrator",                          "category": "azure/guides",            "level": "Associate",   "has_pq": True},
    {"id": "AZ204",  "slug": "azure-developer",                  "code": "AZ-204", "title": "Azure Developer",                              "category": "azure/guides",            "level": "Associate",   "has_pq": False},
    {"id": "AZ305",  "slug": "azure-solutions-architect",         "code": "AZ-305", "title": "Azure Solutions Architect",                     "category": "azure/guides",            "level": "Expert",      "has_pq": True},  # 13 Qs, needs full set
    {"id": "AZ400",  "slug": "azure-devops-engineer",            "code": "AZ-400", "title": "Azure DevOps Engineer",                        "category": "azure/guides",            "level": "Expert",      "has_pq": False},
    {"id": "AZ500",  "slug": "azure-security-engineer",           "code": "AZ-500", "title": "Azure Security Engineer",                       "category": "azure/guides",            "level": "Associate",   "has_pq": False},
    {"id": "AZ700",  "slug": "azure-network-engineer",            "code": "AZ-700", "title": "Azure Network Engineer",                        "category": "azure/guides",            "level": "Associate",   "has_pq": False},
    {"id": "AZ801",  "slug": "windows-server-hybrid-administrator","code":"AZ-801","title": "Windows Server Hybrid Administrator",            "category": "azure/guides",            "level": "Expert",      "has_pq": False},
    {"id": "DP900",  "slug": "azure-data-fundamentals",           "code": "DP-900", "title": "Azure Data Fundamentals",                       "category": "azure/guides",            "level": "Fundamentals", "has_pq": False},
    {"id": "DP203",  "slug": "azure-data-engineer",              "code": "DP-203", "title": "Azure Data Engineer",                           "category": "azure/guides",            "level": "Associate",   "has_pq": False},
    {"id": "DP300",  "slug": "azure-database-administrator",      "code": "DP-300", "title": "Azure Database Administrator",                  "category": "azure/guides",            "level": "Associate",   "has_pq": False},
    {"id": "DP600",  "slug": "fabric-analytics-engineer",         "code": "DP-600", "title": "Fabric Analytics Engineer",                      "category": "azure/guides",            "level": "Associate",   "has_pq": False},
    # Microsoft 365
    {"id": "MS900",  "slug": "microsoft-365-fundamentals",        "code": "MS-900", "title": "Microsoft 365 Fundamentals",                    "category": "microsoft/m365",          "level": "Fundamentals", "has_pq": False},
    {"id": "MS102",  "slug": "microsoft-365-administrator",      "code": "MS-102", "title": "Microsoft 365 Administrator",                    "category": "microsoft/m365",          "level": "Associate",   "has_pq": False},
    {"id": "MS700",  "slug": "managing-microsoft-teams",          "code": "MS-700", "title": "Managing Microsoft Teams",                      "category": "microsoft/m365",          "level": "Associate",   "has_pq": False},
    # Power Platform
    {"id": "PL900",  "slug": "power-platform-fundamentals",      "code": "PL-900", "title": "Power Platform Fundamentals",                   "category": "microsoft/power-platform","level": "Fundamentals","has_pq": False},
    {"id": "PL200",  "slug": "power-platform-functional-consultant","code":"PL-200","title": "Power Platform Functional Consultant",          "category": "microsoft/power-platform","level": "Associate",   "has_pq": False},
    {"id": "PL400",  "slug": "power-platform-developer",         "code": "PL-400", "title": "Power Platform Developer",                      "category": "microsoft/power-platform","level": "Associate",   "has_pq": False},
    {"id": "PL600",  "slug": "power-platform-solution-architect","code": "PL-600", "title": "Power Platform Solution Architect",             "category": "microsoft/power-platform","level": "Expert",      "has_pq": False},
    # Security
    {"id": "SC900",  "slug": "security-compliance-identity-fundamentals","code":"SC-900","title": "Security, Compliance & Identity Fundamentals", "category": "microsoft/security",      "level": "Fundamentals", "has_pq": False},
    {"id": "SC300",  "slug": "microsoft-identity-access-administrator","code":"SC-300","title": "Microsoft Identity & Access Administrator",  "category": "microsoft/security",      "level": "Associate",   "has_pq": False},
    {"id": "SC400",  "slug": "information-protection-compliance-admin","code":"SC-400","title": "Information Protection & Compliance Admin",  "category": "microsoft/security",      "level": "Associate",   "has_pq": False},
    # Dynamics 365
    {"id": "MB210",  "slug": "dynamics-365-sales",                "code": "MB-210", "title": "Dynamics 365 Sales",                            "category": "microsoft/dynamics",     "level": "Associate",   "has_pq": False},
    {"id": "MB230",  "slug": "dynamics-365-customer-service",     "code": "MB-230", "title": "Dynamics 365 Customer Service",                 "category": "microsoft/dynamics",     "level": "Associate",   "has_pq": False},
    {"id": "MB335",  "slug": "dynamics-365-supply-chain-management","code":"MB-335","title": "Dynamics 365 Supply Chain Management",         "category": "microsoft/dynamics",     "level": "Expert",      "has_pq": False},
    {"id": "MB500",  "slug": "dynamics-365-developer",            "code": "MB-500", "title": "Dynamics 365 Finance & Operations Developer",    "category": "microsoft/dynamics",     "level": "Associate",   "has_pq": False},
    {"id": "MB910",  "slug": "dynamics-365-fundamentals-crm",     "code": "MB-910", "title": "Dynamics 365 Fundamentals (CRM)",               "category": "microsoft/dynamics",     "level": "Fundamentals", "has_pq": False},
]

def question_target(level: str) -> int:
    """Minimum question count based on exam level."""
    return {"Fundamentals": 40, "Associate": 50, "Expert": 60, "Specialty": 50}.get(level, 50)

def min_lines_deep_dive(level: str) -> int:
    """Minimum line count for deep dive based on exam complexity."""
    return {"Fundamentals": 400, "Associate": 600, "Expert": 800, "Specialty": 500}.get(level, 600)

def build_tasks() -> list[dict]:
    tasks = []
    for exam in EXAMS:
        eid = exam["id"]
        slug = exam["slug"]
        code = exam["code"]
        title = exam["title"]
        category = exam["category"]
        level = exam["level"]
        dd_file = f"content/{category}/{slug}-deep-dive.md"
        pq_file = f"content/{category}/{slug}-free-practice-questions.md"
        dd_id = f"DD-{eid}"
        pq_id = f"PQ-{eid}"

        min_dd_lines = min_lines_deep_dive(level)
        min_pq_count = question_target(level)

        # Deep Dive task
        tasks.append({
            "id": dd_id,
            "engine": "content-deep-dive",
            "title": f"Deep dive: {title} ({code})",
            "deps": [],
            "scope": [dd_file],
            "accept": (
                f'test -f "{dd_file}" && '
                f'head -1 "{dd_file}" | grep -q "^---" && '
                f'LINES=$(wc -l < "{dd_file}"); [ "$LINES" -ge {min_dd_lines} ] && '
                f'grep -c "```" "{dd_file}" | awk \'$1 >= 15 {{exit 0}} {{exit 1}}\' && '
                f'grep -c "^## " "{dd_file}" | awk \'$1 >= 5 {{exit 0}} {{exit 1}}\''
            ),
            "acceptance_prose": (
                f"Deep dive file exists at {dd_file}, has YAML frontmatter, "
                f"at least {min_dd_lines} lines, 15+ code/example blocks, 5+ H2 sections"
            ),
            "manual": False,
            "exam_code": code,
            "exam_slug": slug,
            "content_type": "deep-dive",
            "min_lines": min_dd_lines,
        })

        # Practice Questions task
        pq_title = f"Practice questions: {title} ({code})"
        if exam["has_pq"]:
            pq_title += " [REPLACE existing]"
        
        tasks.append({
            "id": pq_id,
            "engine": "content-practice-questions",
            "title": pq_title,
            "deps": [dd_id],
            "scope": [pq_file],
            "accept": (
                f'test -f "{pq_file}" && '
                f'head -1 "{pq_file}" | grep -q "^---" && '
                f'grep -c "### Question" "{pq_file}" | awk \'$1 >= {min_pq_count} {{exit 0}} {{exit 1}}\' && '
                f'grep -c "<details>" "{pq_file}" | awk \'$1 >= {min_pq_count} {{exit 0}} {{exit 1}}\''
            ),
            "acceptance_prose": (
                f"Practice questions file exists at {pq_file}, has YAML frontmatter, "
                f"at least {min_pq_count} questions with <details> answer toggles"
            ),
            "manual": False,
            "exam_code": code,
            "exam_slug": slug,
            "content_type": "practice-questions",
            "min_questions": min_pq_count,
        })

    return tasks


def main():
    tasks = build_tasks()
    manifest = {
        "_meta": {
            "project": "courses-microsoft-learning-material",
            "plan": "courses/plan.md",
            "task_count": len(tasks),
            "engines": sorted({t["engine"] for t in tasks}),
            "total_exams": len(EXAMS),
            "deep_dive_tasks": len([t for t in tasks if t["content_type"] == "deep-dive"]),
            "practice_question_tasks": len([t for t in tasks if t["content_type"] == "practice-questions"]),
        },
        "tasks": tasks,
    }

    meta = manifest["_meta"]
    print(f"Generated {meta['task_count']} tasks across {len(meta['engines'])} engines")
    print(f"  Deep dives:       {meta['deep_dive_tasks']}")
    print(f"  Practice Qs:      {meta['practice_question_tasks']}")
    print(f"  Exams covered:    {meta['total_exams']}")

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"Wrote {OUT}")


if __name__ == "__main__":
    main()
