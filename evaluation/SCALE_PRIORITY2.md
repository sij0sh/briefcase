# Scale Priority 2: relationships and candidate generation

**Reviewed:** 2026-10-20. **Evaluation verdict: promotion gate not met.**
Keep the 48-file default and import corroboration; leave experimental
retrieval variants opt-in. Retain the overflow-aware traversal fix as a
correctness repair. Exact contract/state-key joins and richer implementation binding
resolution remain follow-up work; this evaluation does not establish that
all Priority 2 recommendations have been implemented.

The original gate is: **a proposed relation demonstrably adds correct
witnesses on held-out tasks without converting association into behavioral
authority.** No tested relation meets it. The latest discriminating held-out
set gives every arm **22/28 selected and manifest-delivered required paths,
8/10 scenarios passing, zero errors**. Precise binding spans did not produce
additional required paths. These are path-retrieval results, with no measured
claim-witness or answer-quality gain.

## Session handoff and evidence

Both original sessions were inspected for this review:

| Session | Work completed | Handoff |
| --- | --- | --- |
| Claude Code `500380cf-4333-4e53-b442-34462437ecba` | Implemented identifier indexing, workspace binding references, fusion/reach variants, tests and audits; froze the first evaluation and ran its text/joined arms | Usage limit after the reference rescan completed, before the reference-DB evaluation/write-up |
| Muse `01a15a41-281c-7260-bc7f-d354adedaf40` | Completed reference measurements and binding audit; wrote round-1 results; then implemented overflow-aware traversal and measured rounds 2 and 3 | Usage limit during final documentation; measurements and the overflow fix were already present |

The local transcripts are
[Claude Code](/home/joshsimon/.claude/projects/-home-joshsimon-Projects-briefcase/500380cf-4333-4e53-b442-34462437ecba.jsonl)
and [Muse](/home/joshsimon/.local/share/muse/sessions/2026/10/20/01a15a41-281c-7260-bc7f-d354adedaf40/session.jsonl).
The opt-in engine work is already in `87f43bc`, regression tests in
`f50dc11`, and audit/evaluation tooling in `3bd3112`. The working-tree
overflow fix belongs to Muse's second round. Session summaries were checked
against raw files rather than used as measurement evidence themselves.

[priority2-results.json](priority2-results.json) records counts and full
SHA-256 hashes. The raw runs are host-local, outside this repository:

- Round 1: `/home/joshsimon/Projects/t3-eval-work/p2-1020/`, including
  `results/`, `repro/s1.json`, frozen `tree/`, binaries and runner scripts.
- Rounds 2 and 3: `/home/joshsimon/Projects/t3-eval-work/p2r2-1020/`, including
  `results/`, frozen `tree/`, binary and `run-r2.sh` / `run-r3.sh`.

This review recomputed the 14 result-file hashes, three binary hashes and
three scenario-file hashes; all match the recorded values. It also
recomputed aggregate selected/manifest totals from the raw rows. It did
not rerun the full T3 experiment. The raw directories must be preserved
or archived with the hashes for another machine to reproduce this audit.

## Scope and experimental controls

The frozen baseline retains R1/R2/R4 behavior: import corroboration,
48 selected files and the hooks/bin role correction. Manifest recall uses
the actual 8,000-byte projection. A selected path can still be lost during
manifest delivery. Passing scenarios use the evaluator's authored path/gap
checks; a pass is not a correct answer or executed verification.

| Change | Implemented behavior | Limit of this experiment |
| --- | --- | --- |
| `identifier_terms` | Auxiliary FTS representation indexes original text plus CamelCase/acronym/digit components; query terms and admission scoring stay fixed | Changes lexical ordering as well as admission; final path recall alone does not isolate candidate-generation gains |
| Binding `REFERENCES` | Opt-in workspace scan resolves imported binding uses through import/re-export chains, including value, type, extends and implements uses; retains a separate syntax-only run and `symbol_binding` basis | Static association; no resolved workspace calls, general method dispatch or complete implementation hierarchy |
| `structural_reach` | One-hop binding signals can carry the origin file's lexical/anchor relevance beyond the candidate's own relevance ceiling | Relevance does not compound across further symbol hops; this is not a separate protected exploration queue |
| `origin_fusion` | Structural signals for one file pair share an origin; a wholly correlated facet adds only its bonus | File-pair grouping is a coarse proxy for shared derivation, not full occurrence-level provenance |
| Exact contract/configuration/state keys | Audited shared literals against known dev misses | No new general exact-key relation was implemented or evaluated; the existing config-key join is separate |

Variants were measured individually, not as every possible combination.
All three T3 held-out sets have empty `required_facts`; “witness” counts in
their retrieval tables mean authored required paths.

## Round 1: initial measurements

[t3-heldout-p2-scenarios.json](t3-heldout-p2-scenarios.json) contains
10 scenarios and 29 required paths, frozen before its first Priority 2
measurement. Its provenance acknowledges that the author knew the proposed
relations. It is now an observed regression set.

Cells below are selected / manifest-delivered required paths.

| Database/arm | Baseline | Identifier | Origin fusion | Structural reach |
| --- | --- | --- | --- | --- |
| Text-only | 22 / 21 | 23 / 22 | 22 / 21 | 22 / 21 |
| Joined, without bindings | 25 / 24 | 25 / 25 | 25 / 24 | 25 / 24 |
| Joined, with bindings | 25 / 23 | 25 / 24 | 25 / 24 | 24 / 23 |

- Identifier indexing gains `PermissionUpdateNotice.tsx` in all three
  arms, plus two paths text-only. It loses two `snapShot*` paths text-only
  and one in each joined arm: net +1 text-only and 0 joined. Candidate
  presence was preserved for those displaced paths. Dev totals fall by
  1 path text-only and 3 in each joined arm.
- Fusion changes no selected set. Manifest gains include +2 on joined
  dev without bindings and +1 in each reference arm; these are delivery
  improvements for paths already selected.
- Reach has no useful effect without bindings. With bindings it loses
  `webhookRoute.ts` and `AttachmentClaims.ts` on dev, and the HO10 test
  on held-out. Reference held-out falls from 6 to 5 passing scenarios.
- Bindings alone preserve 25/29 selected on held-out but reduce delivery
  from 24 to 23. Dev changes from 44/42 and 9 passes without bindings to
  43/41 and 8 passes with bindings.

The Briefcase original and held-out corpora are neutral across variants:
32/36 selected, 31 delivered, 5/8 passing; and 18/19 selected/delivered,
4/5 passing, respectively.

### Index eviction and the traversal repair

The binding scan adds 66,661 `REFERENCES` assertions. The derived reference
index has **243,567 relationships**, versus 159,582 before bindings.
Its 80-weight tier (`IMPORTS`, `DEPENDS_ON`, `REFERENCES`) alone contains
125,998 links, exceeding the 100,000 hot limit. Tie-breaking by link ID
evicts 15,813 imports and 15,261 dependencies, along with every lower
65–70-weight artifact/config link. Round-1 ranking expansion traversed
only hot links even though Priority 1 retained queryable overflow.

Muse repaired traversal by loading the retained overflow report into
non-serialized `Index.overflow_links`. Expansion, the existing config-key
join and fusion's link lookup traverse hot plus overflow. Hot-index bytes,
IDs, `neighbors` scopes and adjacency counts stay as retained. Historical
indexes without overflow retain their explicit rebuild boundary.
`overflow_only_binding_link_reaches_a_candidate` verifies that an
overflow-only edge reaches its target and that removing it removes that
route.

Retain this repair: automatic traversal should reach retained overflow.
It restores reachability without demonstrating improved relevance on
held-out tasks. It eagerly loads all overflow and scans it during traversal;
memory and latency remain unmeasured scale costs. A bounded lazy adjacency
implementation is a separate performance task.

## Rounds 2 and 3: overflow-aware measurements

Frozen binary `f7254031…` ran serially (`--workers 1`, serve mode,
diagnostics and source verification) on private old/reference DB copies.
The pre-registered rule requires a variant to beat **both** its same-DB
baseline and the old-DB baseline by at least two net selected required
paths, with at least one newly passing scenario, zero errors and zero
false-complete outcomes. For the binding baseline itself, comparison is
against the old-DB baseline. The stronger rule was registered for these
rounds; it should not be applied retroactively to round 1.

[Round 2](t3-heldout-p2-r2-scenarios.json), seed 20261021, has 10 tasks
and 20 required paths. All arms on both DBs select and deliver 20/20,
10/10 pass, with zero errors. This is saturation evidence and cannot
demonstrate an incremental gain.

[Round 3](t3-heldout-p2-r3-scenarios.json), seed 20261022, has 10 tasks
and 28 required paths. It adds import-adjacent collaborators to eight
module/test groups. Every arm on both DBs selects and delivers **22/28**,
**8/10 pass, zero errors and zero false-complete outcomes**. No arm adds a
required path or passing scenario. The gate fails even under the original
qualitative rule. Collaborator selection favors the structural hypothesis,
so this neutral result is useful, although the sample is small and
author-informed.

Dev on the reference DB improves from round-1 **43/41, 8 passes** to
**45/43, 9 passes** after the overflow fix. Relative to that new baseline,
identifier gives 42/40, fusion 45/44 and reach 44/42; reach also loses a
passing scenario. The round-2 old-DB baseline has one process error and
cannot supply a complete paired 56-path comparison. Its error-free
fusion/reach controls give 44/42 and 9 passes. These dev observations do
not meet a held-out promotion gate.

**The first held-out set was not rerun with the overflow fix.** The available
post-fix held-out evidence consists of the saturated round-2 set and the
neutral round-3 set. Neither establishes post-fix neutrality on round 1.

### Where the six round-3 paths were lost

These are the baseline diagnostics on both DBs; every variant also misses
all six paths. Candidate ranks refer to evidence ranking.

| Scenario/path | Earliest observed loss | Score / rank |
| --- | --- | --- |
| SSH `auth.ts` | Below minimum score 24 | 12 / 444–446 |
| SSH `auth.test.ts` | Below minimum score 24 | 6 / 789–802 |
| SSH `errors.ts` | Never became a candidate | No score/rank |
| Azure provider implementation | Candidate excluded by selection | 51 / 73–74 |
| Azure provider test | Candidate excluded by selection | 47 / 109–110 |
| Azure CLI wrapper | Candidate excluded by selection | 38 / 184 |

The five scored candidates have lexical signals only. The Azure files
are above the score floor. A larger selected-file cap alone cannot recover
the absent SSH error path or the two below-floor SSH candidates. Missing
graph signals do not establish that relevant relationships were never
produced: package coverage, retained endpoints, starters and traversal
eligibility still need separate inspection.

The Azure task asks for “one hosted source-control provider integration”
without identifying the vendor. Its labels require Azure specifically.
This leaves an ambiguity in the benchmark: other provider groups may be
valid evidence for the request. Retain the frozen result, and define
explicit vendor constraints or alternate valid witness sets before the
next evaluation.

## Audits, authority and measurement limitations

- **Binding spans:** 400/400 sampled spans correct (399 exact names,
  one alias; seed 20261020). This measures sampled witness localization,
  not complete binding resolution, relationship recall or runtime behavior.
- **Exact keys:** 10 of 14 dev paths absent from the manifest share a
  literal occurring in at most eight repository files with a selected or
  anchor path. Two of the ten were already selected and lost only at
  delivery. The audit therefore suggests at most eight earlier-stage path
  opportunities in this sample, before checking identity and semantics.
  Four paths lack this route, including `clientRpcPermissions.ts`, both
  `RunFinalizationService` paths and `use-thread-pr.test.ts`. No held-out
  gain has been measured for a new key relation.
- **Authority:** binding links preserve predicate, syntax-only basis and
  provenance. Findings use them as consumer candidates; derived test
  associations explicitly say static association, not executed coverage.
  All measured T3 baseline/variant rows have zero complete outcomes as
  well as zero false-complete outcomes. The latter confirms conservative
  reporting in these runs; it does not demonstrate useful sufficiency or
  answer correctness.
- **Errors and denominators:** round-1 joined held-out has six baseline
  errors during the serial latency pass, including allocator failures and
  invalid manifest/case errors. Its previously captured selections remain
  available; a clean serial baseline (`repro/s1.json`) reproduces 25/29,
  24 delivered, 6/10 pass and zero errors. Round-1 joined dev reach has
  one error and reports 40/39 over only 50 represented labels of the
  authored 56. Round-2 old dev reports 40/38 over 51 represented labels
  of 56, with one `briefcase serve exited` error. Neither partial
  denominator is a comparable full-set recall result. No missing rows
  were imputed and no process failure was counted as a relevance gain.
- **Scope:** these measurements do not score required relationships,
  passage coverage, answers, inspected evidence or executed behavior.
  Latency comparisons are invalid across the different worker settings.
  The intermittent serving failures remain unresolved; their shared root
  cause has not been established.

## Validation

The original round-1 record reports lib 163, context 62, stage2 3,
stage4 4, p0p1 7 and p2 10 passing; Python 104 + 8 and Node 17 pass.
Stage3 passes serially, with the already documented parallel lock flake.
Historical conformance records 57 passing and 11 missing-archive failures;
the workspace binding-reference test passes. Clippy reports zero errors.
The overflow repair adds the 164th library test.

This review reran `scripts/verify-context -- --test-threads=1` successfully,
including the overflow regression, Python suites and Node adapters. The
focused workspace binding-reference conformance test also passes (1/1),
and `git diff --check` is clean. These results are recorded in the review
entry in [priority2-results.json](priority2-results.json). The full T3
results above remain pinned to their original binaries and inputs.

## Recommended next steps

1. **Close this evaluation with a failed promotion gate.** Preserve the
   opt-in experiments and overflow repair as separate reviewable changes,
   archive raw results/inputs, and retain all three observed held-out sets
   as regression evidence. Keep defaults unchanged. Fix and reproduce the
   intermittent serving/manifest failures before another expensive
   promotion run; require complete authored denominators and report
   per-variant errors explicitly.
2. **Run one scoped exact-key experiment next.** Start with the audited
   event/contract identities such as `thread.meta.update` and relay
   delivery headers. Distinguish declarations, consumers and incidental
   string occurrences; scope by protocol/package and retain exact spans,
   origins and unresolved cases. Same text is a candidate association.
   Measure admission, selection and delivery separately so recovery of
   already-selected paths is credited to delivery. Add negative controls
   for unrelated uses of the same literal, then freeze a new independent
   held-out set before measuring the implementation.
3. **Test protected exploration through existing continuations.** Use
   filtered inventories, scoped queries, adjacency and explicit case
   expansion to inspect low-score or unselected structural candidates in
   a bounded side queue. Compare equal calls/bytes/time with the current
   driver; record gains and displaced inspection. Audit producer coverage
   and graph starters for the SSH/Azure regression cases before choosing
   package-aware query decomposition or richer binding work. These cases
   are now development evidence, not a new promotion test.
4. **Diagnose identifier admission independently of fusion.** Report
   required-path candidate recall, newly admitted paths, lexical rank,
   score and cap displacement for the auxiliary index. Preserve original
   identifiers and compare path/symbol/body fields separately. The mixed
   selected-path result does not answer whether indexing fixes token
   mismatch before ranking. Keep file-pair fusion and global reach opt-in
   until they improve held-out witnesses without new losses.
5. **Use Priority 3 to measure supported conclusions.** Prototype task
   obligations with implementation, contract, conditions, consumers and
   counterevidence groups, using the existing immutable case revisions.
   Author passage/relationship witnesses and alternate valid evidence
   groups, measure micro/macro claim coverage and useful scoped sufficiency,
   and compare investigation yield at equal effort. Priorities 0 and 1
   provide the continuation substrate; Priority 2 provides no promoted
   relevance improvement to assume. A larger universal cap or broader
   default binding scan is not justified by these results.
