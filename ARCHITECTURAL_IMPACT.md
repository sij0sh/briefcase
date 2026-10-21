# Architectural Change Impact Contract

`impact` composes Briefcase's existing source inventory, task Case, evidence
index, behavior/convention findings, policy reports and retained correspondence.
It identifies what a proposed or captured change may affect and what should be
investigated or verified externally.

It is available through the shared operation catalog, CLI `call impact`, MCP
`briefcase_query` with `operation: "impact"`, and compact MCP `briefcase` with
the same operation. Discover its current request schema with:

```sh
briefcase --json operations
briefcase --json call describe --args '{"operation":"impact"}'
```

## Before a change

Supply a task and an existing captured source view, or a retained Case that
supplies both. Labels are resolved once and the resulting IDs are retained.
The operation does not refresh source capture or launch semantic producers.

```sh
briefcase --json call impact --args \
  '{"mode":"pre","task":"change token encoding safely","view":"before","targets":["src/auth.rs"],"depth":2}'

briefcase --json call impact --args \
  '{"mode":"pre","case_id":"<task-case ID>"}'
```

Explicit `target` or `targets` accept identities supported by the existing
resolver, including exact file paths and directory prefixes ending in `/`.
Resolved captured files become planned changes. Ambiguous or unlocalized
targets retain their candidate identities and an unknown, without choosing a
direct change. If a task-only Case has no exact anchor, at most eight ranked
candidates seed a labelled hypothetical investigation. They never become
confirmed change targets.

An explicitly supplied `view` must match an input Case's source pin. When no
Case was supplied in pre mode, the existing Case builder creates one at that
pin; its ID is included for subsequent source/evidence investigation.

## After a change

Capture and analyze the candidate through the normal producer workflow, then
supply two distinct immutable source snapshots of the same repository:

```sh
briefcase --json call impact --args \
  '{"mode":"post","base":"before","view":"after","case_id":"<original task-case ID>"}'
```

`base` and `view` are required. `case_id` is optional and must match the base
view. The task description is optional in post mode. Targets cannot filter a
post-change assessment: it compares every captured path, including paths
outside the original selected Case slice.

The inventory distinguishes modified, added, deleted and unavailable paths.
Missing paths in an incomplete inventory cannot establish an addition or a
deletion. Supplied architecture declaration artifacts participate in the delta.
A declaration change explicitly requires external review.

`worktree` remains a separate live-byte diagnostic. It does not publish a source
snapshot and cannot substitute for the candidate view. A text-only capture is
accepted, but its missing semantic analysis stays unknown.

## Retained sections

| Section | Meaning |
| --- | --- |
| `changes` | Planned paths or the full captured file delta, with source digests and pinned references |
| `resolutions` | Exact, ambiguous or unresolved pre-change target resolution |
| `affected_components` | Direct changes, possible consumers, dependency context, candidate targets and declared semantic bindings |
| `relationship_changes` | Relationships observed only on one side; missing opposite-side evidence does not prove absence |
| `correspondence` | Existing retained exact or uncertain historical correspondence; only exact content moves remap comparison keys |
| `constraints` | Applicable existing policy diagnostics from each side, preserving report-only mode and authority |
| `verification` | Suggested associated tests, report-only constraint reviews and endpoint/domain interface reviews; every entry is `not_executed` |
| `findings` | Existing capability/condition and convention findings, with side, index and evidence references |
| `unknowns` | Capture omissions, unresolved targets, analysis gaps, bounds, withdrawn evidence and unconnected integrations |
| `governing_decisions` | Empty in v1; no authenticated pinned decision registry is connected |

Each `evidence_refs` entry contains `report_id` and `ref`, which can be copied
directly into `explain` arguments. File references fetch digest-verified retained
source. Link references use the original assertion/witness proof machinery.
Base and candidate references carry their own index IDs, so explaining old
evidence cannot accidentally select the candidate's relationship index.

```sh
briefcase --json call page --args \
  '{"report_id":"<impact-contract ID>","section":"/affected_components","limit":100}'
briefcase --json call explain --args \
  '{"report_id":"<pinned task-index ID>","ref":"<task-link ID>"}'
```

The full schema-valid report is saved before presentation. `limit` defaults to
20 rows per section; the shared gateway applies `max_bytes` to the final
transport envelope. Delivery counts and continuations refer to the full report.
Neither presentation argument enters its content-addressed identity. `page`
and `explain` continue serving old reports after retraction, with the existing
historical/retracted annotations. New assessments rebuild eligible evidence;
they do not copy withdrawn relationships from an old Case.

## Scope and limits

Impact is at captured-file and localized-entity granularity, not a semantic
symbol-level diff. Forward dependency context and reverse potential consumers
are explored separately over supported source predicates. The graph includes
retained overflow relationships, not only the hot adjacency. Every discovered
path retains its root source reference and ordered relationship witnesses.
`depth` defaults to 2 and is bounded to 1–3. `max_nodes` defaults to 4096 and is
bounded to 1–100000 per directional walk; direct changed paths are never dropped
from the source delta to satisfy that bound. A remaining frontier is unknown.

Static `TESTS` associations create check suggestions. They do not bridge
unrelated implementations, and reached test files/symbols are traversal leaves.
Data and route links keep their original static meaning. These paths do not
prove runtime propagation, behavioral effects, API compatibility or complete
consumer coverage. Runtime/deployment selection and cross-repository impact
are outside this initial operation.

Existing policy queries own constraint diagnostics. A report-only rule can
suggest a review but cannot create a mandatory test obligation. The schema
rejects executed-test results, automatic architectural approval, unconnected
ADR claims and a claim of overall completeness. `assessment` is deliberately
`partial` in v1: behavioral verification and decision-registry integration remain
external. No source, policy, declaration or runtime publication is written by
the operation; it only retains Cases/indexes/reports using existing machinery.

## Validation

The self-contained Rust target is registered explicitly because Cargo automatic
test discovery is disabled:

```sh
cargo test --locked --lib --test impact
./scripts/verify-context
python3 -B -m unittest discover -s tools -p 'test_*.py'
python3 -B tools/check_package.py --schemas-only
```

The fixtures cover direct/indirect consumers, complete witness trails, declared
semantics, policy violations, unchanged report identity across delivery limits,
Case pins, changes outside a selected slice, deleted consumers, partial text
captures, incomplete inventories, ambiguous targets, traversal bounds,
retractions, scope validation, CLI/MCP parity, retained correspondence and
recoverable small-budget summaries. Private traversal tests exercise overflow
and shared-suite isolation. Python checks validate the wire contract independently;
they do not establish Rust runtime correctness.

See [implementation validation](IMPACT_VALIDATION.json) for checks actually run
in the implementation environment and the remaining release checks.
