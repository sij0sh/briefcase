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
| `task/trace.rs` | Candidate orchestration before final selection, ranked reach provenance and typed boundaries |
| `task/joins.rs` | Preselection history/session discovery and postselection evidence joins |
| `task/retrieval.rs` | Declarative facets, independent signal fusion, degree-aware traversal, localization and bounded ranking |
| `task/agent.rs` | Budgeted harness projection, precise source fetches and retained evidence continuations |
| `task/case.rs` | Pure assembly from traced facts and captured ledger state; retention, loading, sections and comparison |
| `task/completeness.rs` | Typed gap areas, exhaustive dimension mapping, duty policy and derived outcome |
| `task/brief.rs` | V1 projection, instruction, previews, delivery counts, continuation and byte-budget trimming |

`DutyPolicy` separates traversal predicates/depth from case obligations. Task
selection uses scored, directed frontiers with degree/fanout costs and structural
localization. `neighbors` retains exhaustive directed frontier semantics. Legacy
closure/frontier oracles remain in unit tests for the enumerator; the task engine
no longer uses uniform accumulating closure.

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
traces. Existing OTLP `trace_id` and `span_id` fields are unchanged. The v1 case/brief contracts accept additive ranking/candidate fields.
`agent_context` is one new projection operation over the existing case, not
a new graph product. Its owned contract is `agent-context.schema.json`.
Original cases that omit the new optional fields keep their load identity.

Stage 1 replaces the disposable agent projection with v2: one target anchor,
supplemental ranked paths, one reason/location per path, shared pinned source
fetch instructions and compact evidence states. The gateway budgets after
freshness/fallback attachment; MCP supplies its final frame serializer to that
same pass. Generic presentation limits no longer cap agent files. Complete
tables and diagnostics remain in the case, including the candidate inventory
whose continuation starts at zero. See [evaluation/STAGE1.md](evaluation/STAGE1.md).

Stage 2 keeps candidate evidence rank/score intact and selects a separate duty
reading order. Slice `candidate_rank` and candidate `delivery_rank` make both
orders inspectable; legacy candidates omit the new optional field on roundtrip.
Evaluation artifacts are excluded before normal lexical indexing and graph
expansion, with explicit inspection still available. Cargo test ownership follows
real module declarations into included files and their runner. Native calls use
cached required endpoints and a shared deadline, with no fixed 4,096-item caps.
See [evaluation/STAGE2.md](evaluation/STAGE2.md) for scope and validation.

Run these checks on a Rust-enabled host:

```sh
cargo build --locked
cargo test --locked --lib
cargo test --locked --test context
cargo fmt --all -- --check
cargo clippy --locked --lib --bins --tests
node --test tests/briefcase-task-adapter.test.mjs
node --test tests/briefcase-pi.test.mjs
```

The structured-page budget path now fits a contiguous prefix with logarithmic
measurements; the occurrences regression should run without a skip. New fixtures
cover candidate fusion/ablations, hub damping, test includes, positive Git/session
discovery, foreign/failed session activity, retained source fetches and large
producer inventories. Real Rust execution is still unverified on this host.

The eight recovered task scenarios exercise regression recall, rank, labelled
precision, source/response bytes, actionable gaps and actual harness manifest
delivery. Per-facet ablations are available through the evaluator's `--ablations`
option. The full original conformance corpus is absent; weights need a larger,
held-out task corpus before making generalized retrieval-quality claims.
