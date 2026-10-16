# Stage 2: retrieval quality and inspectable selection

Date: 2026-10-16.
Status: source implemented; available Python/schema/adapter checks pass; Rust
build/tests and corpus measurements still require a Rust-enabled host.

The user's Stage 1 measurements establish delivery parity: 26/36 on the original
scenarios and 17/19 on the held-out scenarios, matching the brief. Those are
user-reported Stage 1 results, not measurements of Stage 2.

## Implemented changes

| Area | Change |
| --- | --- |
| Facet validation | Validate arrays recursively, including item enums, lengths and uniqueness. All facet ablations can reach execution instead of being rejected as invalid argument types. |
| Role balance | Separate reading selection from evidence scores/ranks. Coding duties cycle through implementation/tests/supporting roles; test duty gives tests priority. Exact anchors remain first; unused role capacity falls through. |
| Rank provenance | Candidate `rank`/`score` remain evidence rank/score. Selected candidates have optional `delivery_rank`; slice `rank` is reading order and optional `candidate_rank` preserves evidence rank. `candidate_ref` still addresses the original candidate. |
| Evaluation contamination | Exclude evaluation trees and evaluation report/answer-key documents before normal source FTS indexing. Block their automatic hints and adjacency bridges across facets, not merely lexical ranking. Exact anchors and explicit `search include_evaluation:true` preserve inspection. |
| Disposable state | Exclude `.exitcode` state trees from recall, fixing the reported disposable-tree predicate regression. |
| Cargo targets | Discover explicit and automatic bin/test/example/bench targets, honor auto-discovery switches, and use the correct source directory for custom crate roots. Process configured roots before unconfigured fallback files. |
| Module inclusion | Resolve ordinary literal external `#[path]` modules, including those inside inline blocks. Retain file-to-file `CONTAINS` with source witnesses. Reject escaping/missing/conditional paths and bound recursive cycles. |
| Test ownership | Propagate Cargo test ownership through nested includes. Derive static `TESTS` links for included test modules, intermediate modules and the runner, with association, include and Cargo-manifest witnesses. This does not establish execution. |
| Shared source | Preserve distinct module contexts when multiple Cargo targets include one file. Native locations that cannot disambiguate those contexts remain unknown. Parse trees are reused across target contexts. |
| Parser boundaries | Localize error/missing nodes and report file/line/byte boundaries. Keep valid sibling declarations/calls available. Evaluation fixture manifests and invalid fixture input no longer poison normal Rust syntax scope. |
| Degraded reads | Invalid Cargo manifests make optional module hints incomplete; they do not block retained-text task context. Details remain behind a pinned explanation. |
| Native work | Replace both fixed 4,096-item ceilings and upfront verification of unrelated functions with on-demand, cached endpoint verification and per-file round-robin call processing. A native call-target resolution verifies that retained free-function endpoint. |
| Time/coverage | Reserve 10% of the overall timeout, capped at five seconds, for publication. Keep deadline checks and partial/retryable semantics. Native failures do not establish absence; extraction failures are unresolved. |
| Metrics | Keep the closed knowledge Coverage schema unchanged. Store native-request/endpoint/parser metrics in `native_analysis` diagnostics. The evaluator reports both required-path reading rank and evidence rank. |

The generic reading cycles are declared in `task/duty.rs`:

- Debug, refactor, extend, review, impact: implementation, implementation, test,
  implementation, test, supporting role.
- Test: test, test, test, implementation, supporting role.
- Other duties retain evidence order.

These are transparent initial selection rules. They were not tuned against the
scenario labels, and no measured recall improvement is claimed here.

## Protocol and retained evidence

The Stage 1 agent manifest remains version 2.0, with the same compact packet,
separate anchor, shared fetch instructions and final byte-budget owner. Its
`files` now follow the selected reading order. Budget growth still removes/adds
only the tail of that fixed order for a retained case.

New case/brief rows add optional `candidate_rank`. New selected candidates add
optional `delivery_rank`. Old candidates omit that field when decoded and
serialized, preserving old retained-case identities. A new query can select a
different case because selection policy, derived module links and eligibility
have changed. No stored case is rewritten.

Evaluation artifacts remain captured. Default source recall omits them from its
rebuildable FTS projection; explicit `include_evaluation` uses a separate
projection. Generic literal reads and exact task targets remain available. Git
history is still historical evidence; the source-exclusion policy is not a
claim that old commit messages/patches contain no evaluation material.

Full Rust module/parser diagnostics are retained under the context index's
`/coverage/rust_syntax`. Case coverage contains an exact explanation call rather
than duplicating that inventory. Source/native publication diagnostics include
the same compact counts and bounded examples.

## Validation

Executed successfully:

- 20 Python script/evaluator tests.
- 8 package/schema unit tests.
- 16 pure Pi adapter tests, retaining Stage 1 framing and continuation behavior.

Total: **44 tests passed**. These checks do not execute Rust implementation.

Authored, not executed: 20 new Rust unit regressions and three new context
integration regressions. They cover facet arrays, role balance and stable
selection prefixes, old-candidate roundtrip, evaluation hint/graph exclusions,
Cargo roots and shared includes, cycle handling, localized parser errors,
graceful invalid-manifest hints, more than 4,096 functions/calls, cached/fair
native work, ambiguous native locations, default FTS candidate starvation,
runner linkage and actual pinned source reads. The prior selected-candidate
paging test no longer assumes a selected prefix of global evidence rank.

Blocked on this host:

- Cargo/rustc/rustfmt/Clippy are unavailable. Rust compilation and runtime tests
  remain unverified; the user-reported Stage 1 build does not validate Stage 2.
- The supplied archive still lacks the original `evaluation/fixtures` and
  `evaluation/sources` conformance inputs.
- No binary-backed sync, native resolution run, original/held-out measurement
  or ablation run could execute here.

The grammar dependency remains locked to the supplied version. The changes
improve scope/ownership, localization and preservation of valid code; they do
not claim that every remaining parser error or unsupported Rust construct has
been eliminated. Real native resolution and whole-repository memory/latency
require measurement. Methods, macros, conditional source and ambiguous target
contexts can still keep sync partial and `ready:false`.

## Required acceptance on a Rust-enabled host

```sh
cargo build --locked
cargo test --locked --lib
cargo test --locked --test context
cargo fmt --all -- --check
cargo clippy --locked --lib --bins --tests
node --test tests/briefcase-pi.test.mjs
briefcase --json sync --language rust --timeout 600
python3 scripts/task-evaluate.py --binary target/debug/briefcase --root . \
  --scenarios evaluation/task-scenarios.json --ablations all --harness pi \
  --output task-results.json
python3 scripts/task-evaluate.py --binary target/debug/briefcase --root . \
  --scenarios evaluation/heldout-scenarios.json --ablations all --harness pi \
  --output heldout-results.json
```

Freeze captures, inputs and budgets when comparing stages. Keep answers and
new result reports under the excluded evaluation policy. Compare brief recall,
harness recall, reading/evidence ranks, missing-test cases, native request
counts, parser diagnostics and false-complete outcomes separately. Held-out
measurements remain an evaluation gate, not a source of weight tuning.

Cargo/module rules follow the primary
[Cargo targets reference](https://doc.rust-lang.org/cargo/reference/cargo-targets.html)
and [Rust module reference](https://doc.rust-lang.org/reference/items/modules.html).
