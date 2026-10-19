# Stage 1 (T3): accurate baseline and decision gate

Date: 2026-10-18. Implements Stage 1 of
[.agents/artifacts/T3-improvement-recommendations.md](../.agents/artifacts/T3-improvement-recommendations.md)
on the frozen 14 T3 scenarios (56 required-path instances, pinned `view:1`,
T3 DB at `t3-eval-work/db`, scratch root outside any Git repo).
Raw results: [stage1-t3-results.json](stage1-t3-results.json)
(method `ranked-task-context-v5`; evaluator exit 1 from labelled misses, 0 errors).

## Changes

| Area | Change |
| --- | --- |
| Investigation driver | [task_investigation.py](../scripts/task_investigation.py) gains explicit `manifest_first` (default) and `exhaustive` modes. Manifest-first enqueues the anchor, then supplemental files in manifest order, before coverage/gap/candidate traversal; exhaustive preserves the prior candidate-first queue. `--mode` is exposed by [task-investigate.py](../scripts/task-investigate.py). |
| Driver robustness | Control-plane (`status`/`worktree`/inventory page) reads use at least 64 KiB so T3-scale gap pages cannot stall at zero rows under the 8 KB manifest budget. Unknown worktree alignment (no Git in the frozen export) no longer invalidates traversal; scope stays `open`, while `changed` still invalidates. |
| Timing | [task-evaluate.py](../scripts/task-evaluate.py) reports `brief_ms` (legacy `first_packet_ms`), standalone `manifest_latency_ms`, `first_relevant_passage_ms`, `investigation_latency_ms`, and separate offline `integrity_verification_ms`, with cold (first scenario) and warm medians split in `timing_summary`. Framing/transport bytes were already accounted and remain so. |
| Controls | True lexical-only control: `--lexical-only` (also `--ablations lexical-only`) disables every non-lexical facet together, unlike the one-facet-at-a-time ablations. |
| Arm B | `--investigation-modes manifest-first,exhaustive` runs bounded investigations at 300 calls / 2 MiB / 180 s, reporting `limit_reached`. |
| Baseline | Predeclared `rg`/read baseline ([task_baseline.py](../scripts/task_baseline.py), protocol v3: one `rg -c` search piped to a ranked top-32, bounded 64 KiB reads). v1/v2 protocols exhausted the byte budget before ranking and were replaced before scoring; their runs are not scored. |

## Results (56 required-path instances)

| Condition | Recovered | Notes |
| --- | ---: | --- |
| Existing Arm A (brief + 8 KB manifest) | 40/56 | Reproduces the prior 40/56 exactly, per-scenario identical misses. |
| Lexical-only control | 33/56 | Structured facets contribute +7 net (H6 is better lexical-only, 4 vs 3). |
| Manifest-first Arm B (inspected) | 38/56 | `recovered_beyond_manifest` is empty in all 14 scenarios. |
| Exhaustive Arm B (inspected) | 39/56 | Recovers exactly 1 initially missed instance beyond the manifest (H1 `clientRpcPermissions.ts`, via neighbors). |
| `rg`/read baseline v3 | 10/56 | Single ranked search, no import following; a stronger agent would do better, so this is a floor, not a ceiling. |

Timing (medians): brief 1,189 ms cold / 1,163 ms warm; standalone manifest
1,198 ms cold / 1,157 ms warm; first relevant passage 2,659 ms cold /
2,621 ms warm. The previously quoted 1,232 ms was brief latency, not
manifest latency. Integrity verification remains a separate offline cost.

Arm B cost: every scenario stopped on the call (275–300) or byte (~2 MiB)
limit in 44–95 s; the 180 s limit was never reached. One transient
manifest-first error on H9 was corrected by a single-scenario rerun (4/4,
bytes limit) and patched into the results file with a note.

## Decision gate

Of the 16 initially missed instances, follow-up traversal under a realistic
budget recovers **1** (exhaustive) or **0 beyond the manifest**
(manifest-first). Full-file digest reading of selected/candidate sources
consumes the budget before deeper candidates are inspected. Selection —
not traversal breadth and not ranking-weight guessing — is therefore the
lever for Stage 2, as the recommendations require. No ranking weights were
changed in Stage 1.
