# Worker Task: lw-1-gallery — PSE Gallery: Bohr atom builder, grid of 118 elements, detail info panel via ray selection

You are an autonomous worker agent in the agentflow pipeline.
You have been assigned **exactly one task**. Do it well, verify it, commit it.

## Context documents (READ FIRST)

1. **Repo conventions:** `AGENTS.md` in the repo root (if present) — read it
   fully; it defines build/test/lint commands and architecture rules.
2. **Task spec:** the Acceptance Criteria below is the complete contract for
   this task. Where it conflicts with anything else, the task spec wins.

## Your task

**ID:** `lw-1-gallery`
**Title:** PSE Gallery: Bohr atom builder, grid of 118 elements, detail info panel via ray selection

Conventions: see lw-0-scaffold. TASK: BohrAtomBuilder.cs: public static GameObject Build(Element e, float scale=1) - nucleus sphere from cpk_hex color fallback category hash, shell rings via thin cylinder primitives rotated horizontal, electrons small spheres on rings rotating in Update, cap at 2 visual shells for gallery. ElementGallery.cs: Build() populates scene - floor plane 40x40, 118 atoms laid out via xpos/z = -ypos *2.0, handle ypos 8/9 rows below grid, each atom collider + selectable; register LearningWorldsApp.Builders[World.PseGallery]=Build. ElementInfoPanel.cs: raycast selection via LearningWorldsApp rig camera - show world-space canvas panel with name, symbol, number, atomic_mass, category, phase, melt/boil K, density, electronegativity, appearance, config, summary wrapped at 60 chars; buttons: Raum betreten -> SelectedElement=e.number Go(World.ElementRoom); Schliessen -> Hide. No .unity/.prefab/.asset.

## File scope — edit ONLY these paths

```
Assets/LearningWorlds/Scripts/Pse/BohrAtomBuilder.cs
Assets/LearningWorlds/Scripts/Pse/ElementGallery.cs
Assets/LearningWorlds/Scripts/Pse/ElementInfoPanel.cs
Assets/LearningWorlds/Scripts/Pse/ElectronRotator.cs
Assets/LearningWorlds/Scripts/Pse/InitializeSelection.cs
```

Editing files outside this scope risks merge conflicts with parallel tasks
and may fail the verification gate. If you believe a file is missing from
the scope, note it in your summary but **do not edit it** — the orchestrator
will re-scope and re-dispatch.

## Acceptance gate — the orchestrator WILL run this

```sh
test -f Assets/LearningWorlds/Scripts/Pse/BohrAtomBuilder.cs && test -f Assets/LearningWorlds/Scripts/Pse/ElementGallery.cs && test -f Assets/LearningWorlds/Scripts/Pse/ElementInfoPanel.cs && grep -q xpos Assets/LearningWorlds/Scripts/Pse/ElementGallery.cs
```

You MUST run this command yourself before committing. If it fails, fix your
work and re-run. **Never commit code that fails the acceptance gate.** If
you cannot make it pass after a genuine effort, commit nothing and report
the blocker in your summary.

## Working style

1. Read AGENTS.md first. Follow its hard rules exactly (they are project-wide
   invariants — conventions > convenience).
2. Make the minimal correct change that satisfies the acceptance criteria.
3. Run the acceptance gate. If green, commit. If red, fix and re-run.
4. Write your own tests where the task asks for them — never ship logic you
   have not run.
5. Do not invent dependencies. If the repo declares "zero runtime deps",
   honor it.

{{PREVIOUS_ERROR}}
{{EPISODES}}
{{MISSION}}

## If you are a merge-conflict retry

Your previous attempt passed the acceptance gate but failed to merge because
main advanced while you worked (conflicting files are named in the error
above). Your work is **preserved on this branch**. Do this:
1. Run `git rebase main` — resolve any conflict markers in the named files
   (keep BOTH your work and the new main changes where they don't collide).
2. Re-run the acceptance gate; it must pass on the rebased code.
3. Commit the resolution and finish as normal.

## HARD REQUIREMENT: you MUST modify files

Your task is judged ONLY by real file changes in your scope. The
orchestrator checks `git diff` against the base commit before running the
gate.

**If you do not modify at least one in-scope file, the task FAILS
immediately** — regardless of what you write in your summary. Do NOT:
- Claim success without making changes (this is detected and counted as a
  failure)
- "Provide the commit message" or "report completion" as a substitute for
  work
- Stop after reading files / analysis only

If the task is too large, do it in this order and commit progressively:
1. Make the minimal correct change that compiles
2. Run the acceptance gate
3. If green, commit. If red, fix and re-run.
4. Only if you genuinely cannot make it compile after real attempts should
   you commit nothing and report the blocker.

## When finished

1. Run the acceptance gate. It must be green.
2. `git add -A` the files in your scope (and ONLY those).
3. Commit with message: `feat(lw-1-gallery): PSE Gallery: Bohr atom builder, grid of 118 elements, detail info panel via ray selection`
4. Reply with a concise summary:
   - What you implemented (1–4 bullets)
   - Test count added/passed
   - Any deviation from the contract and why
   - Any follow-up needed

Do not push; the orchestrator merges and pushes.
