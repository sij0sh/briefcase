# Task case implementation

Status: current source implementation, 2026-10-16. Rust compilation and runtime
regressions require a host with Cargo. See [VALIDATION.json](VALIDATION.json).

Briefcase traces retained evidence from a codebase to build a case around a
task, delivered as a brief. The evidence substrate stays upstream of the task
pipeline; task reads do not inspect live source files.

| Module | Responsibility |
| --- | --- |
| `task/request.rs` | Pure validation of task, duty, target, depth and producer scope; separate presentation limits |
| `task/evidence.rs` | Retained snapshots, artifacts, links and witnesses; digest verification |
| `task/trace.rs` | Exact/candidate seeding, lexical fallback, bounded expansion, reach provenance and typed boundaries |
| `task/joins.rs` | History and session traversal; boundary facts without duty input |
| `task/case.rs` | Pure assembly from traced facts and captured ledger state; retention, loading, sections and comparison |
| `task/completeness.rs` | Typed gap areas, exhaustive dimension mapping, duty policy and derived outcome |
| `task/brief.rs` | V1 projection, instruction, previews, delivery counts, continuation and byte-budget trimming |

`DutyPolicy` separates traversal predicates/depth from case obligations. Shared
expansion preserves the brief's undirected accumulating closure and neighbors'
directed frontier semantics. Differential unit tests retain the original loops
as independent traversal oracles. The brief output intentionally changes, so
the pre-completeness byte-equality oracle is not part of the final test suite.
That intermediate guard could not be executed on this host.

Cases are schema-validated before retention and use `task-case:<hash>`. The
identity covers the complete case without its ID, including request scope,
evidence pins and captured ledger state. Presentation limits are excluded.
Loading checks the schema and content identity. Briefs use the case ID for
`id`, `report_id` and `case_id`, and are not retained separately. Section
pointers, including `/slice/impl` and `/not_covered`, are preserved. Legacy
`kind: brief` reports remain pageable and explainable. Retraction epochs
continue to label historical reports without rewriting them.

Completeness dimensions are `anchors`, `slice`, `neighborhood`, `history`,
`sessions`, `gap_inventory` and `verification`. States are `established`,
`partial`, `unknown` and `not_required`. Unresolved anchors produce an unknown
outcome. All required dimensions must be established for a complete outcome.
Verification obligations remain unknown under the current evidence model.
`outcome_scope` is always `retained_evidence`. C1–C6 describe the case;
C7 and `coverage.delivery` describe only the brief. Gaps are sorted by
dimension name, then area, preserving fact order within equal keys.

Prompt hooks keep initial case IDs. Completion checks compare full retained
cases, including slice additions/removals, opened/closed gaps, dimension changes
and advisory dirty paths outside the initial case. Pi records case IDs and
dimension changes, reading retained metadata if a budget notice omitted it.
Checks remain advisory and never start another agent turn.

The path-token guard now removes trailing sentence punctuation before exact
retained lookup. Foreign-session routing also checks recorded roots: imports
share the ledger repository ID even when their recording root differs. Foreign
recordings may join only through retained absolute paths or literal executable
invocations; their relative edits cannot become local edits or commit ownership.

The index implementation key includes `evidence.rs`. Vocabulary uses
`knowledge::support_closure`, evaluator scenarios, and runtime spans or OTLP
traces. Existing OTLP `trace_id` and `span_id` fields are unchanged. No new
agent-callable trace/case operation or v2 contract was added.

Run these checks on a Rust-enabled host:

```sh
cargo build --locked
cargo test --locked --lib
cargo test --locked --test context -- --skip response_budget_pagination_does_not_skip_undelivered_matches
cargo fmt --all -- --check
cargo clippy --locked --lib --bins --tests
node --test tests/briefcase-task-adapter.test.mjs
node --test tests/briefcase-pi.test.mjs
```

The occurrences budget test remains unchanged and should be investigated
separately, as specified by the plan. Full package/conformance checks and real
recall measurements also require the evaluation files omitted from the supplied
archive. No Rust pass/fail baseline or recall numbers were inferred here.
