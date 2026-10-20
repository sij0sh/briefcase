# Scale Priority 1: retained evidence continuations

Priority 1 makes retained evidence queryable beyond initial ranking and reading
bounds. It adds `inventory` and `expand_case` to the shared CLI/MCP operation
catalog, retains exact relationship overflow, and persists complete discovery
work rather than only example follow-ups. Initial ranking still uses the
100,000-link hot index, the existing scores, and the 48-file default.

## Relationships beyond the hot index

New indexes retain every removed relationship in a shared immutable overflow
report. The index's `/coverage/adjacency` records hot, overflow, and total counts
and the overflow report ID. Relationships retain their original IDs, predicates,
endpoints, assertion origins, witnesses, class, and basis.

`neighbors` defaults to `relationship_scope: "all"`. Use `"hot"` to reproduce
the bounded adjacency or `"overflow"` to inspect only the additional records:

```json
{"operation":"neighbors","arguments":{"index_id":"<task-index-id>","target":"src/service.ts","relationship_scope":"all","direction":"in","predicate":"IMPORTS","limit":50}}
```

Direction and predicate filters apply before paging. `explain` accepts overflow
link IDs through the same index. A large derivation's provenance arrays stay in
a retained proof report; `provenance_pages` gives exact counts and executable
page reads instead of returning an explanation that cannot fit the protocol.

Historical indexes are preserved. If an old index discarded overflow, its
coverage says `legacy_overflow_unavailable`; paging that index cannot restore
it. Rebuild from its retained source with `neighbors`, specifying `view` and
omitting `index_id`, or build a new brief. The resulting index has its own ID.

## Compact selective inventories

```json
{"operation":"inventory","arguments":{"case_id":"<task-case-id>","kind":"candidates","path":"apps/server/src","role":"impl","selected":false,"max_score":23,"limit":20,"max_bytes":8000}}
```

Kinds are `candidates`, `lexical`, `files`, and `work`. `files` enumerates the
entire pinned captured source inventory, including files outside task discovery.
Candidate filters include an
exact file/directory `path`, `role`, `selected`, `graph_only`, `min_score`, and
`max_score`. `work_kind` filters work entries; `tail_only` filters lexical
matches that have not received their lexical admission. Filters combine and
apply before the page limit. Scores remain reading hints.

Each filtered result is retained with its source case, view, index, filter,
and deterministic order. `page` follows its immutable cursor, advancing by
rows actually delivered after byte trimming. Changing a filter starts a new
result. Compact candidates preserve their original `source_ref`, independent
of their position in the filtered page. Combine it and `source_offset` with the
response's shared `fetch_source.arguments` to inspect retained source.

Work inventories contain every retained candidate, omitted graph/join starter,
frontier file and entity key, lexical-tail path, bounded session publication or
path, relationship overflow, and relevant producer/evidence boundary. Discovery
work is persisted during assembly; the unified filtered work report is retained
on first access. Stable work IDs distinguish overlapping kinds.

Work entries include a reason, availability status, pinned action or remediation
boundary. Producer/capture omissions and required external verification remain
explicit. The server's static inventory cannot know which passages a client
inspected: candidate entries say `not_recorded_by_server`. The investigation
record tracks that progress independently. Legacy cases without a full frontier
have an explicit rebuild boundary instead of a fabricated exhaustive queue.

## Explicit admission and case assembly

Reading a candidate or lexical inventory does not select it or recompute findings.
Promote paths explicitly:

```json
{"operation":"expand_case","arguments":{"case_id":"<parent-case-id>","paths":["apps/server/src/service.ts"],"max_bytes":8000}}
```

Paths must belong to that case's pinned retained source inventory. This also
admits a captured file found through adjacency or another supported query even
when it never entered task ranking. Such a path is labelled as explicit scope
selection; it receives no fabricated relevance signal.
To admit a batch from the lexical tail, use its exact report ID:

```json
{"operation":"expand_case","arguments":{"case_id":"<parent-case-id>","lexical_report_id":"<lexical-report-id>","lexical_offset":1000,"lexical_limit":20}}
```

The operation scores lexical promotions with the existing lexical method,
preserves the parent's selected paths, and assembles the enlarged scope using
all available retained adjacency. The original `max_files` remains the initial
reading-plan budget; explicit assembly can grow beyond it. Each call accepts
up to 1,000 promoted paths. A lexical batch's `expansion_continue_with` references
the child and the next exact lexical offset.

The compact `case_revision` response identifies parent and child and points to
the selected inventory. The child retains `/coverage/retrieval/revision`,
including the explicit paths and assembly count. Parent bodies, source refs,
case IDs, and existing page cursors remain unchanged. Revisions preserve prior
lexical promotions and discovery work. New promoted paths have explicit graph
continuations as well as source reads.

Expansion rejects pending capture, changed producer profiles, and changed
retraction state. Source and producer identities must match the parent scope;
rebuild a fresh case when those inputs change. Findings and existing conservative
completeness dimensions retain their analytical limits. Expanding a slice does
not establish behavioral completeness, execute tests, or fill producer gaps.

## Independent investigation stages

The reference CLI supports opt-in compact inventory and excerpt inspection:

```sh
python3 scripts/task-investigate.py --root /path/to/repo \
  --state-dir /path/to/investigation --task 'review service rollback' \
  --mode excerpt-first --compact-inventory --inspection-only --steps 20
```

`--inspection-only` records pinned excerpts without automatically queuing
whole-file verification. Resume with `--verify-source src/service.ts` to request
the existing contiguous UTF-8 and SHA-256 verification independently. Small
excerpts covering an entire file can naturally verify its full digest.

Pass an `expand_case` JSON query through `--query` to advance assembly. The
workflow records parent/child identities and retains earlier source references.
Prior supported task assessments are saved with the revision and reopened for
the expanded scope; file digest verification remains valid for the same pin.
Inventory queries for candidates, lexical matches, captured files, and work are also supported.

The source record distinguishes `inspection_state` and `verification_state`;
`case_revisions` records assembly. Reading source does not advance assembly,
and assembly does not assert that source was inspected or verified. The default
reading mode remains unchanged: Priority 0 found that one excerpt per file
improved path reach while reducing claim-witness coverage.

## Validation

The [recorded T3 check](evaluation/priority1-results.json), dated 2026-10-20,
uses an isolated copy of the frozen joined database and the checkpoint-finalization
scenario. Initial selected paths match the R1/R2/R4 baseline. The new index retains
100,000 hot and **59,582 overflow relationships**, for 159,582 total. An overflow
link is queryable and explainable, including a **29,267-item** derivation array
read through a bounded provenance page. Explicit candidate and lexical-tail
promotion and a captured file outside discovery grow assembly
**48 → 49 → 50 → 51 files**, preserving both parent and
historical report bodies. The scenario's 46 discovery-work entries remain
retained. Raw RPCs are under `.agents/artifacts/priority1-validation/rpc/`.

This is functional acceptance using known paths, not a blind retrieval or
answer-quality evaluation. It establishes continuations through the measured
loss boundary without asserting better conclusions. The underlying JSON report
storage, full graph query work, and existing finding limits remain; this phase
does not claim a latency or memory improvement.

Self-contained regression tests cover overflow adjacency and explanations,
complete starter inventories beyond sixteen suggestions, filtering before
limits, byte-bounded cursor progress, original source refs, explicit lexical
promotion across multiple revisions, unchanged parent rows, and independent
inspection/verification/assembly. All ten P2 tests pass, including legacy cases
without candidate arrays and corruption rejection for newly selected bytes.
The P2 tests now run in `scripts/verify-context`. Python scripts (102 tests),
Python tools (8), Node adapters (17), owned schemas (10), and Clippy pass.

Historical archive-based checks still require the missing `evaluation/fixtures`
and `evaluation/sources` archives; those omissions are independent of this change.
The available core suite passes 47 tests. A parallel Stage 3 preflight test also
reports a writer lock instead of invalid configuration; this failure reproduces
on an untouched `HEAD` checkout and the test passes in isolation. This existing
parallel failure remains distinct from the passing continuation checks.
