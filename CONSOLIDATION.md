# Approved consolidation implementation

The simplification engine and human map/presentation machinery are deleted.
Data, deployment and runtime analyzers remain evidence producers; the context
index consumes their outputs and exposes relationships through brief/neighbors.
The implementation is supplied for review. Rust validation remains outstanding.

## Removed products

- Simplification candidate scoring, review/compare reports and commands.
- Technical/human map models, layout, rendering, visual exports, navigation and
  presentation resources, including session maps and feature projections.
- Separate data/deployment/runtime query families and product routing/planners.
- Their schemas, command dispatchers, product-specific tests and obsolete
  evaluators/gates. Original missing evaluation fixtures were not fabricated.

Optional application/feature/workflow declarations still provide explicit
labels and bindings. They do not create a feature product or inferred ownership.

## Consolidated implementation

| Concern | Owner and resulting behavior |
| --- | --- |
| Duties | `task/duty.rs`: typed vocabulary, routing cues, required roles and coverage/depth templates |
| Operations | `service/catalog.rs`: typed kinds and generated argument schemas; exhaustive dispatch shared by CLI, MCP and Pi |
| Task answers | `task/brief.rs`: every ask route composes the same packet, including explain duties |
| Ranking | `search.rs`: one BM25 engine; aliases use it, and unresolved brief candidates use its ranking |
| Evidence projection | `evidence_index.rs`: retained span verification for ledger projections, with no query ranking or fusion pipeline |
| Adjacency | `task/queries.rs` and `task/index.rs`: shared incoming/outgoing relationships; legacy Rust query wrappers reuse adjacency |
| Reports | `reports.rs`: pinned paging, cursor checks, source explanations and delegation to existing assertion provenance traversal |
| Producers | `task/producers.rs`: explicit source/build/deployment/runtime joins with original assertion IDs, pins and gaps |
| Freshness | `freshness.rs`: capture/live HEAD, ahead/behind counts, committed-path deltas and separate dirty overlap |
| Adoption | `adoption.rs`, hooks and Pi extension: bounded prompt context and advisory post-task coverage reports |

Evidence, Derived, Declaration and Claim authority stay distinct. The context
index excludes Claims and inference-dependent derived facts from structural
ownership. It preserves scalar relationship values and original assertion IDs.
Exact retained path, configuration and test references are labelled derivations.
File-association propagation and relationship/publication bounds are explicit.

## Producer boundaries

- A desired deployment must pin the selected source snapshot. Build-manifest
  file paths and digests must match retained source before adding source links.
- Runtime selection is bounded, environment/interval-aware and sampled. Missing
  observations do not mean no activity. Name-only or ambiguous resource mappings
  remain gaps, and affected retractions withhold cross-scope identity joins.
- Source/profile facts compose only for matching source snapshots. Mixed
  projects use explicit profiles; automatic profile guessing was not added.
- Source text and literal reads remain usable without graph projection readiness.
  A fresh core read captures only text and never runs compiler providers.
- External inspection reads only explicitly selected absolute files and returns
  digests/identity observations, with no file contents or execution.
- Live Git status failures are unknown/unavailable, including in post-task checks.

Briefs scope localized producer boundaries to the selected files/entities and
count out-of-slice and unlocalized boundaries. Duty templates select meaningful
gaps instead of adding unconditional role/test/plan/install warnings to every
answer. Completion reports remain advisory.

## Compatibility

| Old spelling | Canonical behavior |
| --- | --- |
| `ask` | `brief` for every question |
| `recall`, `retrieve` | `search` |
| `dependencies` | Filtered outgoing `neighbors` |
| `callgraph` | Outgoing `CALLS_STATIC` neighbors |
| `task_page`, `continue` | `page` |
| `task_explain` | `explain` |

Compatibility names are unadvertised. Removed map, feature-product and
simplification operations have no aliases. Old serialized product contracts
are intentionally unsupported. Callers that inspected old retrieval fusion,
profile timings, graph expansions or evidence-text fields must migrate to
search items/annotations and generic report references. Direct Rust retrieval
keeps strict ledger projection validation before invoking canonical search.
Direct Rust adjacency wrappers keep query report shape; historical inspection
uses pinned reports/explanations.

## Verification and follow-up

The new `context` Rust suite contains 32 self-contained regressions: 15 task
cases, 9 consolidation cases and 8 producer cases. They cover pinned delivery,
byte witnesses, retractions, source-profile boundaries, generic producer joins,
name-only runtime identities, external equality and advisory lifecycle hooks.
Existing query/retrieval/integration assertions were migrated to shared reads.
These Rust tests were authored but could not be executed on this host.

Checks executed here are listed in [VALIDATION.json](VALIDATION.json). Source
lexical/reference checks are not a Rust parser, type checker or compiler.
No latency, recall-quality, release or original corpus results are claimed.
The supplied archive omits `evaluation/fixtures/index.json` and the broader
`evaluation/` corpus, so original conformance/package gates remain blocked.
Missing compile-time unit inputs were replaced by small self-contained unit
fixtures; runtime original-corpus tests still require the complete checkout.

Run on a Rust-capable host before release:

```sh
cargo fmt --all
cargo check --locked --all-targets
cargo test --locked --lib
cargo test --locked --test context
cargo clippy --locked --lib --bins --tests
node --test tests/briefcase-task-adapter.test.mjs
python3 -B -m unittest discover -s scripts -p 'test_*.py'
python3 -B -m unittest discover -s tools -p 'test_*.py'
```

With the original evaluation corpus and native providers restored, also run the
core, toolchain_smoke and conformance suites and the full Pi installer/binary
suite. `scripts/dogfood.py` now exercises current generic reads independently
of the retired product gates; it is a smoke workload, not a substitute gold set.
