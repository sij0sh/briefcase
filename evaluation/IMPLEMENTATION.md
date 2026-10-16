# Evaluation recommendations implementation

Status: source changes implemented; Rust compilation and runtime evaluation are unverified.
Date: 2026-10-16.

Subsequent Stage 1 harness-delivery changes and current validation are recorded
in [STAGE1.md](STAGE1.md). This document records the preceding implementation.

The original `EVALUATION_FINDINGS.md` is preserved as the historical baseline.
No new recall, latency or rust-analyzer resolution figures are claimed.

## Changes

| Recommendation | Implementation |
| --- | --- |
| Compact graph reads | `neighbors` returns counts and a pinned coverage-detail call. Index paging lists a compact section inventory. Raw unresolved occurrences and limitations live in content-addressed producer reports. |
| Actionable gaps | Cases aggregate task-local producer occurrences by available producer/run identity, with predicate/reason counts, three examples and a detail continuation. Occurrences without run attribution explicitly use `unattributed`. Outside-task counts and non-actionable global health remain distinct. |
| Rust analysis | One deadline covers version/startup, one readiness window, native lookups and checked publication phases. Null results are single misses; ContentModified retries are capped. Partial runs remain retryable, carry an explicit subset scope, and cannot satisfy complete call readiness. Identical normalized partial outcomes deduplicate. Git/session domains receive a reserved portion of a shared sync budget. |
| Canonical endpoints | Assertion support files no longer widen traversal endpoints. Existing symbol owners are preserved when structural ownership propagates. Test links use localized owners; exact quoted relative includes retain the test runner association. Cargo `src/bin/**` Rust targets are implementation files. |
| Ranked selection | Exact and ambiguous located anchors, additive lexical retrieval, declared graph facets, eligible candidate-only domain Claims, Git and successful recorded session activity feed one candidate set before final selection. Scores use independent facet contributions, duty/role relevance, distance, endpoint fanout and hub degree. Default selection is 32 files. |
| Original graph vision | Structure, dependency, tests, domain, execution, data and runtime remain policies over one index. History and sessions add candidates with observational provenance. No independent map products or renderers were restored. |
| Harness delivery | `agent_context` projects the same retained case as `brief`. It delivers ranked files, reasons, relevant source spans, exact retained-source fetches, coverage, compact gaps and evidence continuations. Pi and Claude use it automatically, with configurable framing-inclusive budgets. Codex/Muse can call the same operation explicitly; integration status states that delivery is not automatic per prompt there. |
| Evaluation | The eight labelled scenarios were recovered into `task-scenarios.json`. The evaluator reports required-file rank, Recall@K, labelled precision, source/response bytes, actionable gap counts and actual adapter-framing-aware manifest delivery. `--ablations` disables one facet at a time. |

Session discovery reads cached normalized request/activity summaries rather than
hydrating every candidate recording. It permits successful reads and confirmed
edits, requires matching repository and root, and excludes failed reads, edit
attempts and transcript-only path mentions. Retractions invalidate summary keys.
Full recordings remain available for the postselection evidence joins.

Structured metadata pages now fit the largest budgeted contiguous prefix with
logarithmic size measurements. The next cursor addresses the first undelivered
row. This also addresses the previously reported slow occurrences-budget test;
the Rust regression remains pending execution, rather than being skipped.

## Validation

Executed successfully:

- 18 Python script/evaluator tests.
- 8 Python package/schema tests.
- 15 pure Pi adapter tests, including prompt/complete operations, pinned omission
  recovery, configurable budgets and framing parity.
- Owned-schema vocabulary inspection, Python syntax, JavaScript syntax,
  documentation links, Rust lexical delimiters/static includes and whitespace
  checks. These static Rust checks do not parse or compile Rust.

Authored but not executed:

- 7 new Rust unit regressions for analyzer readiness/deadlines, partial call-cache
  semantics, signal fusion, provenance endpoints and metadata-page delivery.
- 7 new Rust integration regressions for additive lexical selection, ranked file
  bounds, hub damping, exact test includes, positive co-change, positive/negative
  session discovery, and large producer inventories.
- Existing context/deployment/data/runtime/case tests.

Blocked checks:

- `cargo test --locked --lib --test context` exits 127 because Cargo is absent.
  Rust builds, formatting, Clippy, binary-backed Pi tests, real analyzer runs and
  task-corpus measurements could not run on this host.
- `python3 tools/check_package.py` cannot load
  `evaluation/fixtures/index.json`; the original package/conformance fixture tree
  was absent from the supplied archive. The recovered task scenarios do not
  replace that fixture tree.

`VALIDATION.json` is the machine-readable execution record. No test results from
an earlier implementation are counted as results for this change.

## Required acceptance on a Rust-enabled host

```sh
cargo build --locked
cargo test --locked --lib
cargo test --locked --test context
cargo fmt --all -- --check
cargo clippy --locked --lib --bins --tests
node --test tests/briefcase-pi.test.mjs
target/debug/briefcase --json sync --language rust --timeout 600
target/debug/briefcase --json sync --git
python3 scripts/task-evaluate.py --binary target/debug/briefcase --root . \
  --scenarios evaluation/task-scenarios.json --harness pi \
  --ablations all --output task-results.json
```

Restore the original fixtures before running the full package/conformance lane.
Review any existing failures independently; the attached baseline validation
already noted a disposable-tree search regression.

## Limits and compatibility

The ranking weights are transparent initial heuristics. They have not been tuned
or validated on a held-out corpus. A selected retrieval hint cannot establish
implementation membership, satisfy a required role's established-association
count, or make a case complete. Unselected candidates, bounded lexical/session
scans and disabled facets remain visible gaps.

Rust still covers retained free functions, not all method/dynamic/generated call
paths. Installed sysroot discovery and all-target metadata are enabled; build
scripts, proc macros, checks and dependency fetching remain disabled. The effect
on real resolution rates requires measurement. Deadline checks are cooperative
around capture/serialization/atomic publication, not hard real-time preemption.
The implementation follows the official rust-analyzer
[server-status extension](https://rust-analyzer.github.io/book/contributing/lsp-extensions.html)
and [configuration reference](https://rust-analyzer.github.io/book/configuration).

Old retained cases omit the new optional fields and keep their load identity.
Strict external validators should use the updated case/brief schemas for new
ranking fields. `agent_context` has its own v1 projection schema. Existing stored
reports are not automatically pruned; storage improvements apply to newly
constructed indexes/cases.
