# Stage 1: compact harness context delivery

Date: 2026-10-16.
Status: implementation complete; available checks pass; Rust execution pending.

The coding-harness packet is now a routing projection over the retained case.
Retrieval, ranking, selected case membership and case/brief identity contracts
are unchanged. No new corpus recall, latency or actual Rust delivery figures
are claimed.

## Changes

| Area | Behavior |
| --- | --- |
| Target | An explicit target resolving to one selected file appears in `anchor`, outside the supplemental `files` list. Directory, ambiguous and inferred anchors keep their paths in the supplemental ranking. |
| File rows | Ranked path, role, one primary reason string, pinned case-local `ref`, optional source byte offset and at most one compact span. Scores, secondary reasons and provenance remain in the case. |
| Source reads | Shared `fetch_source` operation/arguments plus each file's `ref` and `source_offset` replace repeated nested fetch objects. Offset 0 is the default. The anchor uses the same template. |
| Coverage | Preserve the case's completeness states. Include a three-gap preview and truthful delivery totals; full coverage/retrieval/readiness diagnostics remain accessible through pinned explanations. |
| Freshness | Summarize all pins in about 200–300 normal-case bytes: projection state, view count, source age/capture state, pending flag, Git alignment state, dirty-check state/overlap and fallback flag. Secondary stale/invalidated pins remain visible; unknown values remain unknown. |
| Budget owner | Agent projection is unbudgeted. The service attaches freshness/fallback, then calls one final response budgeter with the transport's actual serializer. The 512-byte reservation and projection-level trimming pass are removed. |
| MCP | Pass the complete JSON-RPC serializer into the service, including both text and structured result representations and the trailing LF. Remove MCP's later re-budgeting pass. |
| Trim order | Compact freshness first; remove gap prose, optional coverage-area inventory and span detail before dropping the lowest-ranked supplemental paths. Source offsets survive span removal. |
| File counts | Agent delivery ignores the deprecated generic `limit`; no 10/12/20-file ceiling remains. `max_files` still bounds retained selection. The service default for this operation is 8,000 bytes. |
| Adapters | Pi/Claude prompt and completion calls send task/budget without `limit:12`. Framing-inclusive configurable budgets remain unchanged. |
| Continuations | Complete section handles remain pinned to the same case. The candidate continuation starts at offset 0, because supplemental counts are not candidate offsets after anchor separation or selection. |
| Evaluator | Accept both legacy per-file fetches and v2 shared fetches; validate pinned source calls; count anchor paths in delivered recall and report supplemental count/freshness/non-file metadata bytes separately. |

`src/architecture/task/agent.rs` owns projection and reduction policy.
`freshness.rs`, `response.rs`, `service.rs` and `mcp.rs` own final advisory
freshness and transport accounting. Adapter callers, the catalog, owned schema,
evaluator and documentation use the new protocol together.

## Protocol migration

`agent-context.schema.json` now identifies disposable projection version **2.0**.
Its location under `contracts/v1` is retained; retained case and brief contracts
remain v1. This is an explicit projection-format change, not an additive v1
change. External structured consumers must replace:

- `file.fetch.arguments` with `manifest.fetch_source.arguments` plus that file's
  `ref` and optional `source_offset`;
- structured `why` arrays with the primary reason string;
- target assumptions about `files[0]` with `anchor` inspection;
- inline relationships/history/sessions and verbose coverage with the section
  inventory and pinned `page`/`explain` operations;
- `freshness.fallback.used` with the Boolean `freshness.fallback` for v2 agent
  packets. Ordinary operations keep the full freshness envelope.

`coverage.delivery.files` counts supplemental selected files, excluding the
anchor. A case's `max_files` bound still includes its selected anchor; delivery
adds no further file-count quota. Presentation budgets can omit any supplemental
row, including the last one if a row is individually too large. The case and
restart continuation remain available. Extremely small budgets still return a
pinned omission notice or a budget error if pinned metadata cannot fit.

## Executed validation

| Command | Result |
| --- | --- |
| `python3 -B -m unittest discover -s scripts -p 'test_*.py'` | 19 passed |
| `python3 -B -m unittest discover -s tools -p 'test_*.py'` | 8 passed |
| `node --test tests/briefcase-task-adapter.test.mjs` | 16 passed |
| JavaScript syntax and changed Python syntax | Passed |
| Owned JSON parsing/schema vocabulary and whitespace checks | Passed |

Total: **43 executed tests passed**. These checks validate schema/consumer
changes and adapter behavior; they do not execute the Rust delivery code.

## Rust regressions authored, not executed

Five new unit tests cover:

1. No projection file-count ceiling, one primary reason and compact source
   locations without losing the original source offset.
2. Directory, ambiguous, unresolved and inferred anchor behavior.
3. Six large freshness views, 8/12/16/32 KB budget monotonicity, at least eight
   supplemental paths at 8 KB, exact UTF-8 accounting and pinned inventory.
4. Pi/Claude framing plus the packet fitting 8,000 bytes.
5. Secondary-view staleness/invalidation, final delivered dirty overlap, root
   mismatch and preservation of unknown diagnostics.

Two new context integration tests cover byte-driven delivery independent of
`limit`, large-budget recovery of all 31 supplemental files in a 32-file case,
actual source reads/section continuations and final MCP framing on both
surfaces. The existing bounded-selection integration test now verifies the
separate anchor and shared source-fetch template.

## Blocked checks and remaining acceptance

- `cargo test --locked --lib --test context` exits 127: Cargo is absent. Rustc,
  rustfmt and Clippy are also unavailable. Compilation, type checking, Rust
  tests and production 8 KB delivery remain unverified.
- `python3 -B tools/check_package.py` exits 1 because the supplied archive lacks
  `evaluation/fixtures/index.json` and the original conformance fixture tree.
- No Briefcase executable is available for binary-backed Pi tests, source sync
  or original/held-out corpus measurements.

Run on a Rust-enabled host:

```sh
cargo build --locked
cargo test --locked --lib
cargo test --locked --test context
cargo fmt --all -- --check
cargo clippy --locked --lib --bins --tests
node --test tests/briefcase-pi.test.mjs
python3 scripts/task-evaluate.py --binary target/debug/briefcase --root . \
  --scenarios evaluation/task-scenarios.json --harness pi --output task-results.json
python3 scripts/task-evaluate.py --binary target/debug/briefcase --root . \
  --scenarios evaluation/heldout-scenarios.json --harness claude-code \
  --output heldout-results.json
```

Use the same capture/index inputs when comparing before/after delivery. Inspect
brief recall separately from harness recall and compare the new byte/count
metrics. Facet ablations still need the separate Stage 2 array-validator fix.
Restore the original fixture tree before the complete package/conformance lane.
