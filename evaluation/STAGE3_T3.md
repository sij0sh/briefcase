# Stage 3 (T3): incrementally useful semantic analysis

Date: 2026-10-18/19. Follows the Stage 2 decision ([STAGE2_T3.md](STAGE2_T3.md)):
the default `selection_variant` stays `baseline`, the primary budget stays at
an 8 KB manifest and a 32-file cap, and no ranking weights were retuned to the
T3 scenarios. Stage 3 of
[T3-improvement-recommendations.md](../.agents/artifacts/T3-improvement-recommendations.md)
asks for import and test structure that can be checked, with unsupported
constructs reported explicitly, rather than a complete call graph.

## Summary

| Set (required-path instances) | Stage 2 | Stage 3 (as first run) | Stage 3 (after fixes) |
| --- | ---: | ---: | ---: |
| T3, joined semantic profile | — | 29/56 | 39/56 (anchor fix), **38/56** (+ link-limit fix) |
| T3, text-only capture | 40/56 | — | 40/56 |
| T3, lexical-only control | 33/56 | 30/56 | 33/56 |
| Briefcase task set | 32/36 | — | 28/36 (−3 corpus drift, −1 anchor fix) |
| Held-out set | 16/19 | — | 16/19 |

- **The workspace adapter meets the narrow Stage 3 acceptance criteria.** It
  covers all of T3 in 32 partitions, with no partition failures. Every checked
  relative and workspace import in `packages/contracts`,
  `packages/client-runtime` and `apps/server` resolves to the correct retained
  file. Companion tests are linked to their implementations in 93–100% of
  cases. Call constructs are counted by kind, and none are claimed as
  resolved.
- **Joining the profile first exposed two latent retrieval defects,** each of
  which made the semantic profile harmful:
  1. Plain-word prompt anchors (29/56).
  2. A 100k link cap that dropped links in insertion order.

  Both are now fixed.
- **Selection still doesn't benefit from the import graph.** Even with both
  fixes, the joined profile doesn't beat text-only selection
  (38/56 vs 40/56): once the text-derived links are restored, the import layer mostly pulls barrels and cross-package hubs into the 32-file cut. Joined briefs are also about 5× slower (8.9 s vs 1.6 s
  median).

## Code changes

| Change | Files | Tests |
| --- | --- | --- |
| **Gap aggregation (recommendation E).** Capture exclusions, unlocalized producer boundaries and unavailable facets become one `global_health` row per category with counts and a pinned source pointer. Task-local gaps stay individual and are marked `actionable`. | `task/completeness.rs` | completeness unit tests |
| **Workspace TypeScript adapter (recommendation C, Stage 3).** `--project workspace` treats every retained `tsconfig.json` as an independent partition, and a failing partition fails alone. Each file is owned by the deepest partition that contains it. Imports resolve through compiler resolution per partition, then retained workspace manifests (`exports` conditions), then explicit relative paths, then syntactic re-exports. Call constructs are classified by kind and never resolved. Files outside every partition are reported as `outside_partitions`. | `typescript_workspace.cjs` (new), `typescript.cjs`, `typescript.rs` | `workspace_partitions_resolve_manifests_barrels_and_classify_calls` |
| **Scan performance.** `contracts::add` deduplicates with a set above 256 rows, and `Ledger::publish` replaces a quadratic `inputs.contains` with a set lookup. The first workspace scan took 3,592 s and 5.8 GB RSS. Most of that time was attributed to these two loops; a rescan with the fixes was not timed. | `contracts.rs`, `ledger/publish.rs` | existing suites |
| **Identifier-shaped prose anchors.** When a task gives no explicit target, a declared name found in the prompt text now becomes a 90-point `ambiguous_location` anchor only if it can't be an ordinary word. That means a qualified name (`::` or `.`), an underscore, a letter-digit mix, or an internal capital. Explicit targets still match exactly. | `task/trace.rs` | `only_identifier_shaped_names_anchor_from_prose` |
| **Value-ranked relationship limit.** The task index used to drop *every* link once 100,000 existed, so whatever was inserted first won. Now all links are built, ownership propagation runs over them, and then the lowest-value links are evicted by retrieval facet weight, with ties broken by id. Coverage reports `omitted_relationship_predicates`. | `task/index.rs`, `task/retrieval.rs` (`facet` made `pub(super)`) | `relationship_limit_evicts_structure_before_cross_file_links` |

Test status on the frozen snapshot:
- lib: 155 passed (task module: 54 after the limit fix).
- `tests/context.rs`: 62 passed.
- stage2/3/4, p0p1 and p2 suites: pass.
- Conformance: 56 passed, 11 failed, 111 ignored. All 11 failures come from
  the untracked `evaluation/sources` and `evaluation/fixtures` corpus, the
  same environment failure documented in P0/P1.

## Stage 3 acceptance (workspace adapter)

The full T3 scan produced `view:2` with 115,954 assertions and resolved
30,070 imports:

| Resolution method | Imports |
| --- | ---: |
| Compiler | 17,331 |
| Workspace manifest | 6,631 |
| Re-export | 6,107 |
| Relative | 1 |

The scan also classified 17,340 external imports, 2 as missing, 107 as
non-code and 19 as bundler assets. Of 32 partitions, 32 were analyzed. 67
files sit outside every partition, mostly `apps/desktop/gnome-extension`,
`.github` scripts and root Vite configs, and each is reported.

The adapter was checked against an independent regex parse of the retained
sources (copied into `acceptance` in
[stage3-t3-results.json](stage3-t3-results.json)):

| Scope | Relative imports | Workspace imports | Companion tests linked |
| --- | ---: | ---: | ---: |
| packages/contracts | 345/345 | — | 38/38 |
| packages/client-runtime | 938/938 | 349/349 | 124/124 |
| apps/server | 4114/4114 | 2143/2143 | 322/347 |
| apps/web | 4560/4565 | 1825/1830 | 394/398 |

The two exceptions:
- **apps/web:** the 5+5 misses are all in two test files whose imports are
  absent as a whole (`ThreadStatusIndicators.test.tsx` and
  `chat/OpenInPicker.test.tsx`). That pattern points to a per-file analysis
  failure, not wrong resolution.
- **apps/server:** the 25 unlinked companion tests (e.g.
  `compileCache.test.ts`, `cli/app.test.ts`, and the migration tests) don't
  import their namesake file. A link is reported only when an import exists.
  The adapter makes no claim that a test proves behavior.

Call constructs are reported explicitly and none are resolved:

| Construct | Count |
| --- | ---: |
| `method_or_property_call` | 175,486 |
| `effect_namespace_call` | 171,750 |
| `local_identifier_call` | 125,921 |
| `effect_yield_star` | 68,507 |
| `imported_function_call` | 38,448 |
| `jsx_element` | 18,904 |
| `constructor` | 16,464 |
| `effect_function_call` | 13,857 |
| `computed_callee` | 7,709 |
| `dynamic_import` | 426 |
| `tagged_template` | 1,318 |
| `require` | 67 |

Import resolution is reported as `complete: false`. The old limits that
blocked web and server (4,096 definitions, 16,384 occurrences) no longer apply
in workspace mode. Oversized configurations would fail per partition, not
globally.

## Evaluation protocol and an invalidated run

All Stage 3 numbers come from an isolated setup under
`t3-eval-work/s3/`:

- A frozen source snapshot, built into a private binary (sha256 `e16c8717…`;
  the limit-fix binary is `4ce9a033…`).
- Private DB copies. The joined T3 DB is `view:1` (text) plus `view:2`
  (workspace profile), restored from the clean joined backup.

The first rerun was discarded. Another session sharing this working tree
rebuilt `target/release/briefcase` five minutes into Arm A, which picked up
an unrelated `worktree.rs` edit. That session was also running
`task-outcomes.py` against the same joined DB.

The Briefcase DB is a `sqlite3 .backup` of the retained repo DB, synced
against the real repository in 138 s. `git status` was unchanged before and
after.

## Results

### Pre-fix regression: plain-word anchors (29/56)

The first joined run scored 29/56, and the lexical-only control on the same
DB fell from 33/56 to 30/56. Since the lexical control fell too, the problem had to
be upstream of the ranking weights.

The workspace profile added 45,761 TypeScript declarations. `trace::resolve`
treated any declared name that appears in the prompt text as a 90-point
anchor. Prompt words such as `stop`, `a`, `resource` and `HIGH` matched
declarations and displaced every lexical match. H4, H7, H8 and H11 dropped
to 0.

After the identifier-shape filter:
- Joined Arm A: 39/56.
- Lexical control: back to its Stage 2 value of 33/56.
- Text-only control: 40/56, the same as Stage 2, so the fix doesn't change
  text-only behavior.

Pre-fix runs are kept in `t3-eval-work/results/prefix/`.

### Joined (anchor fix) vs text-only, per scenario

| Scenario | Joined | Text-only | Difference |
| --- | ---: | ---: | --- |
| S2 keybinding owners | 3/3 | 2/3 | + `packages/shared/src/keybindings.ts` |
| H2 checkpoint finalization | 1/4 | 2/4 | − `EffectWorker.ts` |
| H4 stale PR display | 1/4 | 0/4 | + `threadPullRequestCompatibility.test.ts` |
| H7 offline webhook retry | 2/6 | 3/6 | − `HookInboxObject.ts` |
| H12 mobile reveal patch | 3/4 | 4/4 | − `pnpm-workspace.yaml` |
| Other 9 | equal | equal | |

Gap rows per scenario:

| Capture | Range | Median |
| --- | --- | ---: |
| Joined | 13–22 | 16 |
| Text-only | 12–21 | 17 |
| Stage 1 (for reference) | ~207 | — |

The joined DB had 0 errors and 0 false-complete outcomes. `docs_disabled`
moved nothing (+0/−0).

### Link-cap defect and fix

The joined index still lost several documentation-to-code links that the
text-only index had. For example, `docs/internals/overview.md →
RunFinalizationService.ts` was missing, and `RunFinalizationService.ts` fell
from score 35 (rank 94) to 7 (rank 817).

The cause was `insert_link`: once 100,000 links existed, it silently dropped
every new link. In the joined view, roughly 116k profile assertions are
inserted before the text-derived `REFERENCES_ARTIFACT`, `TESTS` and config
links. The joined index therefore omitted 59,471 relationships, essentially
all of the text-derived layer. The text-only index omitted 0, and so did the
Briefcase index, so the fix cannot change the Briefcase or held-out results.

With value-ranked eviction, the joined index omits only:
- `DECLARES`: 43,377
- `CONTAINS`: 15,894
- `IN_CYCLE`: 200

The documentation and test links are back.

**Joined T3 with the link-limit fix: 38/56** (vs 39/56 with only the anchor
fix). Run with binary `4ce9a033…` on a fresh copy of the joined DB; 0 errors,
0 false-complete outcomes, median warm brief 9,194 ms.

| Scenario | Anchor fix only | + link-limit fix | Change |
| --- | ---: | ---: | --- |
| H7 offline webhook retry | 2/6 | 3/6 | + `HookInboxObject.ts` (restored `REFERENCES_ARTIFACT`, same signal as text-only) |
| H12 mobile reveal patch | 3/4 | 4/4 | + `pnpm-workspace.yaml` (was not even a candidate; restored `REFERENCES_ARTIFACT`) |
| H3 huge history item | 3/3 | 2/3 | − `threadHistoryPaging.ts`: score 123 → 141, rank 7 → 11, cut |
| H9 linux portal ordering | 4/4 | 3/4 | − `DesktopLinuxUrlHandler.ts`: score 105 → 114, rank 19 → 32, cut |
| H10 attachment retry | 2/3 | 1/3 | − `AttachmentClaims.test.ts`: score 121 → 121, rank 18 → 49 |

`RunFinalizationService.ts` (H2) recovered its text-only score (35) but
ranks 144 instead of 94, so it is still not selected.

The fix does what it should: the two gains are exactly the restored
text-derived links, with signals identical to the text-only run. The three
losses are not lost evidence. Each file kept or *raised* its score, but more
files now carry both a text-derived signal and an import signal, so scores
inflate around the gold and push it past the cap. This is the same mechanism
that sank Stage 2's `multi_query` variant.

What displaces the gold: 145 of the 448 selected slots across the 14
scenarios hold files that the text-only run did not select. Examples:
- H3: the `packages/contracts/src/index.ts` barrel and
  `packages/contracts/src/orchestrationV2.ts`.
- H9: server and orchestration files reachable by imports (`Orchestrator.ts`,
  `EffectWorker.ts`, `keybindings.ts`) for a desktop portal task.
- H10: mobile, web and client-runtime outbox and RPC files that import
  attachment types.

So with a complete index, the joined profile is now a clean measurement of
the import layer's value under the current weights: **−2 against
text-only**. Undiscriminating `IMPORTS`/`DEPENDS_ON` signals reward barrels
and shared hubs.

The investigation numbers in the Cost section were measured with the
anchor-fix binary. The limit fix leaves brief latency unchanged (9.2 s vs
8.9 s median warm), and the investigations were not rerun with it.

### Briefcase and held-out sets

**Briefcase: 28/36, vs 32/36 in Stage 2.** An A/B with the anchor filter
disabled (`identifier_shaped` returning true, on an identical DB copy) gives
29/36, which splits the loss:

- **Corpus drift: −3.** `contracts/v1/README.md`, `tests/context.rs` and
  `integrations.rs` are pushed out of the top 32 by untracked files created
  after Stage 2 finished:
  - `task/findings.rs`
  - `task/convergence.rs`
  - `tests/briefcase_convergence.rs`
  - `tests/briefcase_findings.rs`
  - `tests/briefcase_session_findings.rs`
  - `scripts/test_task_outcomes.py`
- **Anchor fix: −1.** `task/queries.rs` in `occurrences-untargeted` was
  anchored by the plain word "occurrences", which is a real, unique function
  name. The filter suppresses that anchor along with the false T3 ones.
- **Misses on both binaries:**
  - `command.rs`
  - `tests/briefcase_task.rs`
  - `tests/briefcase_history.rs`
  - `scripts/verify-pi` (score 20, below the minimum of 24)

The Briefcase set had 0 errors, 0 false-complete outcomes and a median first
packet of 4.9 s.

**Held-out: 16/19, the same as Stage 2.** The misses are `sync/sessions.rs`,
`command.rs` and `args.rs`.

### Cost

| Metric | Joined | Text-only |
| --- | ---: | ---: |
| Median warm brief | 8,933 ms | 1,624 ms |
| Median cold brief | 15,362 ms | 1,828 ms |
| Median first relevant passage (warm) | 19.9 s | 3.5 s |

Investigations on the joined DB, under the 180 s limit:

| Mode | Paths recovered (of 39 in manifest) | s/call | Stage 1 equivalent |
| --- | ---: | ---: | --- |
| Manifest-first | 35 | 0.887 | 38, at 0.172 s/call |
| Exhaustive | 32 | 1.399 | 39 + H1 `clientRpcPermissions.ts` beyond the manifest, at 0.306 s/call |

All 14 scenarios hit the time limit. Per call, page and neighbor reads are
about 5× slower on the joined DB, so investigation results are now limited
by time, not coverage.

## Mapping to the Stage 3 criteria

| Criterion | Status |
| --- | --- |
| Known local imports resolve to the correct retained file | Met: contracts, client-runtime and server 100%; web 99.9% (two files with no imports). |
| Known workspace subpath imports resolve where declarations are retained | Met: client-runtime 349/349, server 2143/2143, web 1825/1830. |
| Companion tests associated without claiming they prove behavior | Met where an import exists (878 of 907 checked). Tests that don't import their namesake are not linked. |
| Unsupported methods and Effect constructs reported explicitly | Met: 12 construct kinds counted, 0 resolved, `calls: not_resolved`. |
| Oversized configurations give bounded partial results or clear failures | Met: per-partition failure. 67 files reported `outside_partitions`. Import resolution flagged incomplete. |
| Expand to server and web through controlled partitioning | Done: 32 partitions, all analyzed. |
| Scorecard: discovery maintained, selection improved | Not met: joined selection is 38/56 vs text-only 40/56, at about 5× latency. Discovery is met: 55/56 required paths are candidates with the link-limit fix (text-only 54/56). The losses are all in selection: 13 not selected and 4 below the minimum score. |

## Remaining misses and recommendations

1. **Structural, but unselected.** Three joined misses (anchor-fix run) have
   an `IMPORTS` path to the anchors but rank below the 32-file cut:

   | Scenario | File | Score | Rank |
   | --- | --- | ---: | ---: |
   | H1 | `clientRpcPermissions.ts` | 26 | 145 |
   | H4 | `threadPullRequestCompatibility.ts` | 67 | 41 |
   | H10 | `AttachmentClaims.ts` | 76 | 89 |

   Every importer of a widely used module gains the same 80-point facet, so
   import edges don't discriminate. The link-limit run shows the other side:
   barrels and hubs reached by imports displace gold that already scores
   well. The next lever is *reachability
   weighting*: score an import by how specific it is (inverse importer count,
   distance from the anchor), not by facet alone. It should be validated on
   T3, Briefcase and held-out together. Tuning it on these three files would
   break the Stage 2 protocol.
2. **Rarity-based prose anchors.** Instead of rejecting every plain word,
   allow a plain-word declaration name to anchor when few declarations carry
   it. That would restore `occurrences` (unique in Briefcase) while still
   rejecting `stop` (declared throughout T3). Deferred, because choosing the
   threshold on these runs would be fitting to the scenarios.
3. **Latency.** The joined profile costs about 7 s per brief and 5× per
   investigation call. Profile it before shipping the workspace view as a
   default, starting with the profile join in `Snapshot::source` and index
   construction over roughly 160k candidate links.
4. **Below-floor lexical misses** (e.g. `scripts/verify-pi` at 20,
   `pnpm-workspace.yaml`) are unchanged from Stage 2. Structure can't reach
   them.
5. **Corpus drift.** Briefcase-repo comparisons across stages need a pinned
   snapshot of the corpus. A live working tree that other sessions write to
   is not stable enough.

Raw results: [stage3-t3-results.json](stage3-t3-results.json).
