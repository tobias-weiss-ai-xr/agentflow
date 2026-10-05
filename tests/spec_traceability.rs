//! Spec↔test traceability checker: `openspec/specs/<id>/spec.md` is the
//! living contract (see docs/spec-traceability.md). This file turns that
//! contract into a checked one — every `### Requirement:` heading in the
//! spec library must be referenced by at least one test, or the suite goes
//! red naming the gap.
//!
//! The convention: a test that verifies a requirement carries a comment line
//!
//!     // spec: <spec-id>/<requirement-slug>
//!
//! immediately above its `#[test]` fn. The slug is the requirement title
//! lowercased with every run of non-alphanumeric characters replaced by a
//! single `-` and trimmed — e.g. `### Requirement: Cost receipts` maps to
//! `state/cost-receipts` and `### Requirement: Scope contention avoidance`
//! maps to `scheduling/scope-contention-avoidance`.
//!
//! Adding (or removing) a requirement to the spec library therefore fails
//! this suite until a test references it (or the gap is explicitly allow-
//! listed in `UNMAPPED` below). See docs/spec-traceability.md for the full
//! convention, including how to place markers.

use std::fs;
use std::path::{Path, PathBuf};

/// Marker prefix recognized in test sources. Must start a comment line on
/// its own (see `markers_in_file`).
const MARKER: &str = "// spec:";

/// Requirements that genuinely cannot be tested yet, with a reason each.
/// This is an explicit, shrinking allowlist — NOT a way to paper over
/// requirements that have an obvious test. `traceability_allowlist_stays_small`
/// caps it at 3 entries so the gap can only shrink.
const UNMAPPED: &[(&str, &str)] = &[
    // (currently empty: every requirement in openspec/specs has at least
    //  one referencing test)
];

/// One `### Requirement:` heading parsed from a spec file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Requirement {
    /// `<spec-id>/<requirement-slug>`, e.g. `state/cost-receipts`.
    id: String,
    /// The heading text as written, e.g. `Cost receipts`.
    title: String,
    /// The spec file the requirement was parsed from.
    file: PathBuf,
}

/// Slug of a requirement title: lowercase, every run of non-alphanumeric
/// characters collapsed to a single `-`, trimmed. See the module docs.
fn slugify(title: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for ch in title.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            pending_dash = true;
        } else if pending_dash {
            out.push('-');
            pending_dash = false;
        }
    }
    out.trim_matches('-').to_string()
}

/// Full marker id (`<spec-id>/<slug>`) for a requirement title.
fn requirement_id(spec_id: &str, title: &str) -> String {
    format!("{spec_id}/{}", slugify(title))
}

/// Parse one spec markdown text into its `### Requirement:` entries.
fn parse_spec(spec_id: &str, text: &str, file: PathBuf) -> Vec<Requirement> {
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some(title) = line.strip_prefix("### Requirement:") {
            let title = title.trim();
            out.push(Requirement {
                id: requirement_id(spec_id, title),
                title: title.to_string(),
                file: file.clone(),
            });
        }
    }
    out
}

/// Repo root (the crate under test).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every requirement in `openspec/specs/<id>/spec.md`, sorted by id so
/// failure output is stable. Panics with an actionable message if the spec
/// library is missing (that itself is a broken contract).
fn requirements_from_specs_dir(specs: &Path) -> Vec<Requirement> {
    let mut out = Vec::new();
    let entries = fs::read_dir(specs)
        .unwrap_or_else(|e| panic!("cannot read spec library at {}: {e}", specs.display()));
    for entry in entries.flatten() {
        let spec_md = entry.path().join("spec.md");
        if !spec_md.is_file() {
            continue;
        }
        let spec_id = entry.file_name().to_string_lossy().to_string();
        let text = fs::read_to_string(&spec_md)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", spec_md.display()));
        out.extend(parse_spec(&spec_id, &text, spec_md));
    }
    assert!(
        !out.is_empty(),
        "spec library at {} parsed to zero requirements — did the spec \
         format change?",
        specs.display()
    );
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Marker ids on the comment lines of one source file. A marker is a line
/// that, trimmed, starts with the `MARKER` prefix; the id is the next
/// whitespace-delimited token after it. Only whole-line comments count, so
/// prose or string literals mentioning the convention never register.
fn markers_in_file(path: &Path) -> Vec<String> {
    let text = fs::read_to_string(path).unwrap_or_else(|_| String::new());
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix(MARKER) {
            if let Some(id) = rest.split_whitespace().next() {
                if !id.is_empty() {
                    out.push(id.to_string());
                }
            }
        }
    }
    out
}

/// Marker ids in every `*.rs` file under `dir` (subdirectories only when
/// `recursive`). Unreadable/missing dir → empty.
fn markers_under(dir: &Path, recursive: bool) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if recursive {
                out.extend(markers_under(&path, true));
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.extend(markers_in_file(&path));
        }
    }
    out
}

/// Collect every marker id from the repo's test sources: `tests/*.rs` and
/// `src/**/*.rs` (integration and unit tests alike). Sorted for stability.
fn collect_markers(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    out.extend(markers_under(&root.join("tests"), false));
    out.extend(markers_under(&root.join("src"), true));
    out.sort();
    out
}

/// Requirements with no referencing marker and no allowlist entry — the
/// traceability gap. Pure so the sanity test can drive it directly.
fn unmapped<'a>(
    reqs: &'a [Requirement],
    markers: &[String],
    allowlist: &[(&str, &str)],
) -> Vec<&'a Requirement> {
    reqs.iter()
        .filter(|r| !markers.iter().any(|m| m == &r.id))
        .filter(|r| !allowlist.iter().any(|(id, _)| *id == r.id))
        .collect()
}

/// Actionable failure text for a set of unmapped requirements: each names
/// the requirement, its spec file, and the exact marker line to add.
fn failure_message(missing: &[&Requirement]) -> String {
    use std::fmt::Write;
    let mut msg = format!(
        "spec<->test traceability: {} requirement(s) have no referencing test:",
        missing.len()
    );
    for r in missing {
        let _ = write!(
            msg,
            "\n\n  \"{}\" ({})\n      no test carries its marker. Add this comment \
             line immediately above the #[test] that verifies it (or write \
             that test):\n          {MARKER} {}",
            r.title,
            r.file.display(),
            r.id
        );
    }
    msg
}

/// Every `### Requirement:` in the living spec library is verified by at
/// least one test carrying its `// spec: <id>/<slug>` marker.
#[test]
fn every_requirement_has_at_least_one_referencing_test() {
    let reqs = requirements_from_specs_dir(&repo_root().join("openspec").join("specs"));
    let markers = collect_markers(&repo_root());
    let missing = unmapped(&reqs, &markers, UNMAPPED);
    assert!(missing.is_empty(), "{}", failure_message(&missing));
}

/// The `UNMAPPED` allowlist is small, documented, and only names real
/// requirements — so the traceability gap can only shrink.
#[test]
fn traceability_allowlist_stays_small() {
    assert!(
        UNMAPPED.len() <= 3,
        "UNMAPPED holds {} entries; the cap is 3 so the gap can only \
         shrink. Give the unmapped requirement a test instead.",
        UNMAPPED.len()
    );
    let reqs = requirements_from_specs_dir(&repo_root().join("openspec").join("specs"));
    for (id, reason) in UNMAPPED {
        assert!(
            !reason.trim().is_empty(),
            "UNMAPPED entry {id} must document WHY it cannot be tested yet"
        );
        assert!(
            reqs.iter().any(|r| &r.id == id),
            "UNMAPPED names {id}, which matches no requirement in \
             openspec/specs — a stale entry"
        );
    }
}

/// Sanity-check the checker itself: a deliberately-bogus requirement (one
/// that exists in no real spec) must fail the matcher logic, and the
/// documented slug examples must hold.
#[test]
fn a_bogus_requirement_fails_the_matcher() {
    // The documented slug examples from docs/spec-traceability.md.
    assert_eq!(slugify("Cost receipts"), "cost-receipts");
    assert_eq!(
        slugify("Scope contention avoidance"),
        "scope-contention-avoidance"
    );
    assert_eq!(
        slugify("Worktrees target the task's repository"),
        "worktrees-target-the-task-s-repository"
    );

    // A synthetic spec library with one requirement nothing references.
    let dir = std::env::temp_dir().join(format!("af-spec-trace-{}-bogus", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let bogus_spec_dir = dir.join("openspec").join("specs").join("bogus");
    fs::create_dir_all(&bogus_spec_dir).unwrap();
    fs::write(
        bogus_spec_dir.join("spec.md"),
        "# bogus Specification\n\n## Requirements\n\n### Requirement: Never shipped\n\nNothing.\n",
    )
    .unwrap();

    let reqs = requirements_from_specs_dir(&dir.join("openspec").join("specs"));
    assert_eq!(reqs.len(), 1, "synthetic library parses one requirement");
    assert_eq!(reqs[0].id, "bogus/never-shipped");

    // No markers (and no allowlist entry) → the matcher must report it.
    let missing = unmapped(&reqs, &[], &[]);
    assert_eq!(
        missing.len(),
        1,
        "a bogus/unreferenced requirement must fail the matcher"
    );
    let msg = failure_message(&missing);
    assert!(
        msg.contains("bogus/never-shipped"),
        "failure message names the expected marker: {msg}"
    );
    assert!(
        msg.contains("Never shipped"),
        "failure message names the requirement title: {msg}"
    );

    // And the same requirement with a marker (or an allowlist entry with a
    // reason) is NOT reported — the matcher only flags true gaps.
    assert!(unmapped(&reqs, &["bogus/never-shipped".to_string()], &[]).is_empty());
    assert!(unmapped(&reqs, &[], &[("bogus/never-shipped", "not shipped")]).is_empty());

    let _ = fs::remove_dir_all(&dir);
}
