# Stage 2 (T3): controlled selection variants

Date: 2026-10-18. Runs only after the Stage 1 decision gate
([STAGE1_T3.md](STAGE1_T3.md)): traversal recovers at most 1 of the 16
initially missed instances, so selection is the lever. All conditions keep
the primary 8 KB manifest and 32-file cap fixed; no ranking weights were
retuned to the diagnostic scenarios.

## Implementation

`selection_variant` is a new brief/agent_context argument (catalog enum,
task-case schema, retained `Request`; default `baseline`, omitted from
retained state when baseline). Variants in
[retrieval.rs](../src/architecture/task/retrieval.rs):

| Variant | Change |
| --- | --- |
| A `role_diverse` | Duty role cycle (impl/impl/test/impl/test/support) applied for every duty, not only coding duties. |
| B `reserved_floor` | Up to 4 reserved slots (cap-scaled) for candidates scoring 12–23 with a non-lexical structural signal. |
| C `multi_query` | Companion test/impl pairing by shared evidence base plus task-term path-ownership signals, added at discovery. |
| D `linked_diversity` | Round-robin by package root (first two path segments) after anchors. |
| `docs_disabled` | Secondary ablation: documentation excluded from selection unless it is the exact anchor. |
| `combined` | A+B+C+D together. |

Evaluator: `--selection-variants` in
[task-evaluate.py](../scripts/task-evaluate.py) scores each variant against
the same pinned baseline and reports gained/lost required paths.

## Results (required-path instances recovered)

| Variant | T3 (56) | Briefcase (36) | Heldout (19) | Verdict |
| --- | ---: | ---: | ---: | --- |
| Baseline | 40 | 32 | 16 | — |
| A role_diverse | 40 (+0/−0) | 32 (+0/−0) | — | No effect: baseline already role-cycles coding duties. |
| B reserved_floor | 40 (+0/−0) | 32 (+0/−0) | — | No effect on gold (see below). |
| C multi_query | 31 (+1/−10) | 33 (+1/−0) | 16 (+0/−0) | Rejected: score inflation reorders evidence and crowds out gold on T3. |
| D linked_diversity | 24 (+1/−17) | 27 (+2/−7) | — | Rejected: gold clusters inside one package, so package round-robin pushes it past the cap. |
| docs_disabled | 41 (+1/−0) | 32 (+1/−1) | 16 (+0/−0) | Not promoted: gains H7 `HookForwarder.ts` on T3 but loses a required contract README on Briefcase. Kept as an ablation. |
| Combined | 15 (+0/−25) | 27 (+2/−7) | — | Rejected. |

Raw T3 results: [stage2-t3-results.json](stage2-t3-results.json).
Briefcase/heldout runs used the same binary and protocol; their per-variant
gained/lost lists are summarized above.

## Why the misses resist these variants

Every one of the 16 Stage 1 misses is a lexical-only candidate in the
text-only capture (two are not candidates at all). The five below-floor
misses score 11–23 with only a `lexical:retained_text` signal, so a
structural reserved pool (B) cannot reach them, and there is almost no
graph for C/D to exploit — C's added signals mostly inflate unrelated
files, and D actively disperses the single-package clusters where the
gold lives (notably `apps/server`). Role balance (A) was already the
baseline behavior for the duties these tasks use.

## Decision

No variant is independently beneficial across T3, Briefcase, and the
held-out set, so per the Stage 2 protocol ("combined: only independently
beneficial changes") **the default selection remains `baseline`**. All
variants stay available behind `selection_variant` for further controlled
experiments. The next lever is not another selection permutation on
lexical-only evidence: it is richer retained relationships (Stage 3
import/export/test linkage) that give selection something structural to
select on, plus passage-level evidence for the below-floor lexical pool.
