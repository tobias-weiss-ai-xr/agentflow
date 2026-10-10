# Learning-Worlds VR — Design (PicoLearningRooms)

**Date:** 2026-10-09 · **Status:** approved (user: "A via agentflow") · **Executor:** agentflow campaign (`lw-*` tasks)

## Goal

Turn PicoLearningRooms (Unity 6000.3.25f1, PICO 4 Enterprise, current = PicoExperiments framework) into a
**learning-focused VR app**: a landing hub ("everything about learning") with portal doors to learning worlds.

## Aspects (ported into Unity)

1. **3D PSE** — port of `~/git/periodic-table` (walkable periodic table, live on GitHub Pages).
   Gallery with 118 Bohr atoms (xpos/ypos grid, category colours), search + family filters, detail panel,
   and a **data-driven ElementRoom engine**: one scene builder parameterized by element, ring of 5 learning
   stations (① crystal/molecular structure ② where you meet it ③ who found it ④ classic experiment
   ⑤ self-check quiz). All content from `docs/assets/elements.json` (copied into
   `Assets/LearningWorlds/Data/elements.json`).
2. **Arachnophobia VRET** — port of the WebXR exposure-therapy app (`pse.chemie-lernen.org/arachnophobia/`,
   research: "Overcoming Arachnophobia with VR"). Procedural bedroom, graduated exposure controller
   (levels = spider distance/size/activity/realism tiers), SUDS 0–100 rating prompts, JSONL session logging
   to `Application.persistentDataPath`.
3. **Research labs** — existing PE scenes (AOI visual search etc.) stay reachable from the hub, unchanged.

## Architecture decisions

- **Everything procedural, code-only.** No hand-authored `.unity`/`.prefab` YAML (GUID/meta hazard, not
  compile-verifiable). Worlds are built at runtime from C# (`GameObject.CreatePrimitive`, legacy `TextMesh`
  with built-in `LegacyRuntime.ttf` fallback, `UnityEngine.UI` canvases, URP-Lit shader with `Standard`
  fallback). Entry: `[RuntimeInitializeOnLoadMethod]` in `LearningWorldsApp.cs`, active only when the active
  scene is a LearningWorlds scene; worlds switch via `SceneManager.CreateScene`/unload.
- **Data-driven.** `ElementData.cs` parses the actual JSON keys (agent inspects file first); graceful
  fallbacks: quiz missing → generated fact question; lattice missing → skip station content.
- **Decoupled from PE framework.** No edits to existing PE scripts; Arachno/SUDS logging is standalone
  JSONL (PE questionnaire/eye-tracking integration is a later, separate change).
- **German hub signage, English learning content** (JSON is English) in v1.

## Folder layout (all new, additive)

```
Assets/LearningWorlds/
├── Data/elements.json
├── Scripts/
│   ├── LearningWorldsApp.cs      (bootstrap, world state machine)
│   ├── Pse/   ElementData.cs BohrAtomBuilder.cs ElementGallery.cs ElementInfoPanel.cs
│   │          ElementRoom.cs LatticeBuilder.cs QuizStation.cs
│   ├── Arachno/ BedroomBuilder.cs SpiderFactory.cs ExposureController.cs SudsRating.cs
│   └── Hub/   LandingHub.cs Portal.cs LearningSignage.cs
└── Editor/    LearningWorldsMenus.cs  (Open-Hub menu, build-settings registration)
ci/unity-compile.sh              (Unity batchmode compile gate, fail-fast on license errors)
```

## agentflow campaign

Workers: glm-4.6 (zai) + a second OpenAI-compatible slot; `pi` CLI; base branch `main`; scope globs disjoint
per task. DAG: `lw-0-scaffold → {lw-1-gallery, lw-1-room, lw-1-arachno, lw-1-hub} → lw-2-wire → lw-2-compile
→ lw-3-docs`.

- Gates: static (`test`/`grep`/`python -c` JSON check, `bash -n`) for all tasks except **`lw-2-compile`**,
  whose gate is `ci/unity-compile.sh` — a real Unity `-batchmode` compile of the worktree (first run pays the
  full Library import; `accept_timeout_s` 5400). One real compile gate at the merge-critical point; full
  per-task Unity gates were rejected (4 parallel first-imports ≈ hours).
- `lw-3-docs` rewrites README (learning-app framing, ports, old-code note: pre-sync history preserved at
  `init`/`checkpoint` commits + `D:\unity\pico\PLR-pre-purge-backup.bundle`).

## Non-goals (v1)

Multiplayer/Photon, Convai NPCs, localization, eye-tracking analytics in element rooms, realistic spider art
(stylized low-poly, 3 tiers), device build (editor play + compile gate only).

## Old code note

Pre-sync PicoLearningRooms (stock VR template + PICO SDK 250) is preserved in git history
(`init unity project`, `checkpoint: …`) and in `D:\unity\pico\PLR-pre-purge-backup.bundle`; the 78 MB SDK zip
was purged from history during housekeeping.
