# Briefcase self-evaluation: re-run after the recommendations commit

Date: 2026-10-16 · Commit evaluated: `8be9523` plus two one-line compile fixes (below) ·
Host: Fedora 44, cargo/rustc 1.98, rust-analyzer 1.98.1, node 24.19 ·
Baseline: [`EVALUATION_FINDINGS.md`](EVALUATION_FINDINGS.md) (commit `80a4b0f`)

This run checks the same objective as the baseline: *trace retained evidence from a
codebase to build a case around a task, delivered as a brief*, with explicit
provenance, typed gaps and honest completeness. It also checks the new
`agent_context` harness manifest, which is what an agent actually receives.

Method: build and install from source, run the test suites, build a **fresh** DB
(`--db` in a scratch directory, so the existing `.briefcase/briefcase.db` was not touched),
run `scripts/task-evaluate.py` on the 8 original scenarios and on 5 new **held-out**
scenarios (`evaluation/heldout-scenarios.json`), then repeat both on a **clean
clone** with the evaluation answer key removed. Each core read was then checked by hand.

## TL;DR

| Objective | Baseline `80a4b0f` | Now `8be9523` |
| --- | --- | --- |
| Builds from source | ✅ | ❌ **Did not compile** (lib: E0505; lib tests: E0283). Builds after two one-line fixes |
| Rust tests | not run | ⚠️ 243 pass / 35 fail. About 21 distinct failures: 11 are missing fixtures, **10 are real defects** |
| Initial DB build | ❌ 719 s, timed out | ✅ **140 s**, Git and sessions included. ❌ Still `sync: failed`, `ready: false`, exit 1, **on every run** |
| Rust call resolution | ❌ ~3% | ❌ **3.3%** (800 of 24,479 sites). Unchanged |
| Honest completeness | ✅ 0 false-complete | ✅ 0/13 false-complete |
| Brief file recall (top 32) | 27/36 (75%) | 27/36 (75%) original; **17/19 (89%) held-out** |
| Brief focus / ranking | ❌ 6–110 files, alphabetical | ✅ Ranked, capped at 32. Median rank of a found required file: 4–5 |
| Gap inventory usable | ❌ 1.6k–31k rows | ✅ **13–24 actionable rows** per case |
| `neighbors` | ❌ 0 items, 19 MB metadata | ✅ Works, 3–12 KB |
| `test` duty finds tests | ❌ 0 tests | ❌ Still misses `tests/briefcase_cases.rs` and `tests/context.rs` |
| **Agent manifest (`agent_context`)** | n/a | ❌ At the Pi default budget it delivers **1 file, usually the target itself**. Recall **6/36** |
| Facet ablations (`exclude_facets`) | n/a | ❌ **Every call rejected**. The argument validator has no `array` type |
| `occurrences` | ✅ exact | ✅ exact (3/3 literals match `git grep`) |
| `explain` provenance | ✅ | ✅ |
| Latency | 1.3–3.3 s first, up to 42 s full | ✅ ~2.1 s first, ≤ 4.7 s full |
| Storage | 328 MB | ⚠️ 290 MB after sync, 326 MB after ~30 reads. Unchanged |

Bottom line: the recommendations commit fixed the problems that made the baseline
hard to use: hub blowup, unranked slices, the gap flood, `neighbors` and sync time. The
**brief** is now a reasonable ranked starting point. Recall did **not** improve on
the original labels. Static call resolution is still about 3%, test linkage is still at
module level, and sync can never reach `ready`. The new **agent manifest** loses most
of the brief's value: at its default budget it hands the agent the file the agent
already named. The commit was also merged without compiling.

---

## 1. Install and build

```sh
cargo build --locked --release   # FAILED: E0505 in src/architecture/task/index.rs:179
cargo test  --locked --release   # FAILED: E0283 in src/architecture/task/completeness.rs:323
```

`evaluation/IMPLEMENTATION.md` says the Rust changes were authored on a host without
Cargo and that Rust compilation was "unverified". That was the case. Two
minimal fixes were applied to the working tree (not committed):

| File | Fix |
| --- | --- |
| `src/architecture/task/index.rs:179` | `BTreeSet::from([target])` → `BTreeSet::from([target.clone()])` (`target` is still borrowed by `Link::new`) |
| `src/architecture/task/completeness.rs:323` | `"…".into()` → `"…".to_string()` (ambiguous `TryFrom`) |

After the fixes it builds in ~47 s cold, with 7 dead-code/unused-import warnings
(`declared_symbols_of`, `module_category`, `module_parents`, `retrieval_units_batch`, …).
`cargo install --path . --locked` then works.

## 2. Test suites

| Suite | Pass | Fail | Ignored |
| --- | --- | --- | --- |
| `--lib` | 98 | 3 | 0 |
| `--test context` | 39 | 6 | 0 |
| `--test core` | 45 | 10 | 36 |
| `--test conformance` | 53 | 13 | 111 |
| `--test toolchain_smoke` | 8 | 3 | 61 |
| Python (`scripts/`, `tools/`) | 15 | 0 | — (run from each script's directory) |
| Node (`tests/*.test.mjs`) | 30 | 1 | — (needs `BRIEFCASE_TEST_BINARY`, defaults to `target/debug`) |

`core`, `conformance` and `toolchain_smoke` share modules, so failures repeat across
them. The distinct failures:

**Environmental. The fixture tree `evaluation/sources/` is not in the repo (11):**
`briefcase_contracts` ×3, `briefcase_evolution`, `briefcase_session` ×3,
`briefcase_substrate`, `briefcase_typescript`, `briefcase_workflow` ×2. IMPLEMENTATION.md
already documents this.

**Real defects (10):**

1. **`exclude_facets` is unusable (5 tests).** These are every new `briefcase_retrieval`
   regression plus `retrieval::tests::provenance_files…`. They fail with
   `ARGUMENTS: Invalid value for exclude_facets`. Cause:
   `src/architecture/service/arguments.rs:61-79` validates only `string`, `boolean`,
   `object` and `integer`, and `_ => false` rejects every `array`. As a result the new
   ablation feature, and `task-evaluate.py --ablations`, cannot run through CLI, MCP or Pi.
   The retrieval regressions written for this commit (hub damping, co-change,
   sessions, lexical additivity) have **never run**.
2. **Budget floors (2).** `an_oversized_agent_manifest_preserves_a_pinned_restart` needs
   20,335 bytes. `every_duty_and_target_uses_a_presentation_independent_case` needs 8,283.
   Both report `BUDGET: max_bytes is too small for required pinned metadata`. The new
   metadata, mostly `freshness`, has raised the minimum response size above what the
   tests and adapters assume. See §5.
3. `briefcase_task::refactor_packet_surfaces_tests_plans_scripts_configuration_and_install_touchpoints`
   fails an assertion on packet rows, probably because the 32-file cap dropped a touchpoint the test expects.
4. `search::tests::disposable_trees_are_never_recalled` is a known regression that was already in the baseline.
5. `briefcase_integration::typescript_sync_requires_explicit…` returns `failed` where `needs_configuration` is expected.
6. Node: `oversized structured output fails closed with a valid JSON notice`.

## 3. Initial DB build

```sh
BRIEFCASE_DB=<scratch>/briefcase.db briefcase --json sync --language rust --timeout 600
# 139.5 s wall, 0.9 GB peak RSS, exit 1
# sync: failed · ready: false · local_sync: incomplete
# git: refreshed (4 commits) · sessions: 9 Claude Code + 4 Pi imported
```

Improved:

- **5× faster, and within budget.** 140 s, against 719 s and a timeout. Git and sessions are
  synced in the same pass, and rust-analyzer reports `quiescent: true`.

Still broken:

- **Sync always fails on this repo.** Static calls end `state: partial`, so `sync`
  reports `failed` and exits 1. A second sync takes 20 s, **reuses** the identical
  partial run (`reused: true`, from the new "identical partial outcomes deduplicate"
  logic) and fails again. The DB can never reach `ready` here, so any script or hook
  using `--require-ready` or the exit code treats a usable DB as broken.
- **Call resolution has not improved.** It attempted 24,479 sites, resolved about 800 (3.3%),
  left 23,211 unresolved and 468 unsupported. `completed: 4096` equals the
  stated "native-function limit is 4096", so the run is capped by that limit. The deadline
  fixes made sync fast, but did not make it resolve more calls.
- **Parse coverage.** `limitations` lists **19 Rust files with parse errors**, including
  `task/index.rs`, `task/joins.rs`, `session/mod.rs`, `native_host.rs` and `bin/briefcase/run.rs`.
  `rustc` compiles them, so this looks like grammar drift in `tree-sitter-rust 0.23`. It also lists
  **29 test files "outside supported Cargo module roots"**. Those `tests/briefcase_*.rs`
  files are pulled in through `#[path]` from `tests/context.rs`, `core.rs` and `conformance.rs`. This explains
  why test links still attach to module files (§4).
- `project` and `sessions:claude-code` stay `dirty` after sync. Claude Code is
  plausibly dirty because this session's transcript keeps changing.

## 4. Task-case recall

Same evaluator, default `max_files: 32`, `--harness pi`.

### Original 8 scenarios (`evaluation/task-scenarios.json`)

| Scenario | Recall | Required-file ranks | Missing | Gaps |
| --- | --- | --- | --- | --- |
| duty-variant-refactor | 5/7 | duty 1, mod 10, tests 17, catalog 18, request 19 | `trace.rs`, `bin/briefcase/command.rs` | 24 |
| task-case-schema | **6/6** | schema 1, case 4, py-test 5, schema.rs 9, cases-test 18, README 23 | — | 24 |
| analyzer-timeout | **5/5** | 1, 2, 3, 7, 21 | — | 17 |
| git-history-limits | 3/4 | 1, 8, 31 | `tests/briefcase_history.rs` | 23 |
| pi-extension | 2/4 | ts 1, test.mjs 22 | `pi.rs`, `scripts/verify-pi` | 24 |
| case-tests (duty `test`) | 1/3 | case 1 | `tests/briefcase_cases.rs`, `tests/context.rs` | 13 |
| new-harness (no target) | 3/5 | mod 2, codex 3, sync/sessions 5 | `codex-v1.json`, `integrations.rs` | 22 |
| occurrences (no target) | **2/2** | queries 5, briefcase_task 7 | — | 20 |

**27/36 (75%)**, the same total as the baseline. The missed files changed: the
schema task improved from 3/6 to 6/6. Pi-extension dropped from 3/4 to 2/4 and new-harness from 4/5 to 3/5,
because the 32-file cap now cuts files that the old unbounded closure happened to include.
Recall@5 = 14/36 and Recall@10 = 19/36.

### Held-out scenarios (new, `evaluation/heldout-scenarios.json`)

I wrote and labelled these myself from `git grep` and reading the code. The
implementation was never tuned on them.

| Scenario | Recall | Ranks | Missing |
| --- | --- | --- | --- |
| H1 MCP gateway tools (refactor) | 3/4 | 1, 2, 6 | `service/catalog.rs` |
| H2 Claude Code discovery (debug) | **5/5** | 1, 2, 4, 7, 30 | — |
| H3 watch poll interval (extend) | **4/4** | 1, 3, 4, 6 | — |
| H4 disposable trees (no target) | **2/2** | 2, 4 | — |
| H5 python source roots (no target) | 3/4 | 3, 5, 16 | `sync/project.rs` |

**17/19 (89%)**, Recall@5 = 12/19. Caveat: the wording of my tasks may share more
vocabulary with the code than the original scenarios do.

### Leakage control

This repo contains its own answer key. `evaluation/task-scenarios.json` and
`EVALUATION_FINDINGS.md` list every labelled path. They appear inside the slices, for example
`EVALUATION_FINDINGS.md` at **rank 2** for the duty task, through `retained_text`,
`REFERENCES_ARTIFACT` and a past session's `file_edit_confirmed`. I cloned the repo,
deleted both, committed, synced a fresh DB (no sessions match the clone's root),
and re-ran everything:

| Set | Repo as-is | Clean clone |
| --- | --- | --- |
| Original 8 | 27/36, R@5 14 | 27/36, R@5 11 |
| Held-out 5 | 17/19, R@5 12 | 17/19, R@5 11 |

Leakage moves rank by a few positions but does not explain recall. The control is
partial: Git history still contains the commit that added those files.

### Why recall is capped

- **Test linkage is still module-level.** `neighbors src/architecture/task/case.rs --direction in`
  returns no `TESTS` edges. `task/mod.rs` receives all 6. With 29 test files
  "outside supported module roots" (§3), the `test` duty cannot name the tests for a file.
- **Static calls are 3% resolved**, so call-based neighbours like `trace.rs` ← `duty.rs` are missing.
- **Docs crowd out code.** The duty-task slice, for example, holds 5 doc and 4 config rows, including `README.md`, `CONSOLIDATION.md`
  and contract schemas, which are pulled in by text mentions. With a fixed 32-file cap, every doc row
  takes a slot an implementation file could use.

## 5. Agent manifest (`agent_context`)

This is the new path that Pi and Claude Code receive automatically on each prompt.

| Set | Brief recall (32 files) | Manifest recall at Pi default (~8 KB) | Files delivered |
| --- | --- | --- | --- |
| Original 8 | 27/36 | **6/36** | 1 in every scenario |
| Held-out 5 | 17/19 | **5/19** | 1–2 |
| Clean clone, original | 27/36 | 9/36 | 2–4 |

In all 6 targeted original scenarios, the one delivered file was the **target the user
already supplied**. Inspecting the 8,000-byte duty-task manifest:

- `freshness` takes **3,589 bytes (45%)**, mostly a `views` list with 6 pinned
  views on a single-generation DB. `coverage` takes 1,739 bytes and `files` 953 bytes.
- Delivery scales poorly. 12 KB gives 6 files and 16 KB gives 10. 24, 32 and 64 KB still give **10 files**, a hard
  cap below the 32-file slice.
- Manifest order follows slice rank, so the agent's second file is
  `EVALUATION_FINDINGS.md`, then `service.rs`, `README.md`, `contracts.rs`, `adoption.rs`…
  The required `task/mod.rs` is #10, and `request.rs` and `catalog.rs` are not delivered.

This explains the budget-floor test failures in §2.

## 6. Other reads

- **`neighbors`** works now. Output is compact (3–12 KB) and pinned. For example, `analyzer.rs` has 17
  outgoing relationships by predicate. An unknown path returns `resolution: unresolved`
  with exit 0, which is explicit enough. ✅
  (Shell gotcha while testing: in zsh, an unquoted `$args` is not word-split, so a
  multi-argument string reaches Briefcase as one path. That briefly looked like a regression.)
- **`occurrences`** for `DutyPolicy`, `retrieval_facets` and `is_disposable` gave 6, 2 and 7 matches,
  identical to `git grep`, with `complete_for_retained_text`. ✅
- **`search`**: about 1.1 s per query. The right file is in the top 4 (`analyzer.rs` #4 for "rust-analyzer
  deadline timeout", `arguments.rs` #1 for "argument validation array schema", `agent.rs` #2).
  ⚠️ The same commit fills 3–6 of the first 9 slots, so per-commit hits need deduplicating.
- **`explain`** on a slice ref returns the retained source. On a `via_link`, it returns the full
  assertion, derivation, pass name (`module-import-dependency-v1`) and support chain. ✅ Endpoints
  are shown as entity IDs, so readers need another hop to see file paths.
- **`page /slice/test`**: 5 rows, deterministic, no continuation needed. ✅
- **Gaps**: 13–24 `not_covered` rows per case, all marked actionable.
  Producer-boundary occurrences (3.7k–12.4k) moved to a pageable detail. This is the biggest
  usability improvement over the baseline. ✅
- **Storage**: 290 MB after the first sync and 326 MB after ~30 reads, for a ~2.4 MB repo. Largest tables are
  `retrieval_units` 71 MB, `reports` 55 MB, `records` 46 MB and `overlays` 33 MB. Not improved.

## 7. Recommendations (highest leverage first)

1. **Compile before merging.** Add `cargo build && cargo test --lib` to CI or `scripts/verify-core`.
   Both errors here were trivial, but they blocked every user.
2. **Add `array` to `service/arguments.rs::validate`**, with `items.enum` and `uniqueItems`.
   This unblocks `exclude_facets`, ablations, and 5 unexecuted regressions. Re-run them.
3. **Make the manifest deliver files, not metadata.** Summarise `freshness` (state, age,
   head, and dirty overlap count) in about 200 bytes. Put full views behind `fetch_evidence`. Drop the target
   from the delivered list, or count it as free. Remove the hard 10-file cap. Goal: about 8 ranked
   paths with one-line reasons in 8 KB.
4. **Let a partial static-call run count as ready**, with an explicit "partial" scope, or
   stop exiting 1. Today the DB is permanently `failed` on its own repo.
5. **Raise or remove the 4,096 native-function cap**, then measure resolution again.
   Resolution was 3% before the deadline fixes and is still 3%.
6. **Follow `#[path]` includes** so `tests/briefcase_*.rs` are module-attributed. Then
   attribute `TESTS` edges to the files the tests call. This is the main blocker for the `test` duty.
7. **Down-weight doc and config text-mention rows** in a fixed-size slice, or give docs their own
   small quota so they don't push out implementation files.
8. Look into the 19 tree-sitter parse errors. A grammar upgrade may recover structure
   for files central to the task pipeline.
9. Keep evaluation answer keys out of indexed paths (for example, ignore `evaluation/` in
   capture), or the labels will keep leaking into rankings.

## Reproduce

```sh
cargo build --locked --release && cargo install --path . --locked
export BRIEFCASE_DB=/tmp/bc-eval/briefcase.db
briefcase --json sync --language rust --timeout 600          # ~140 s, exits 1 (partial calls)
python3 scripts/task-evaluate.py --binary ~/.cargo/bin/briefcase --root . \
  --scenarios evaluation/task-scenarios.json --output orig.json --timeout 300
python3 scripts/task-evaluate.py --binary ~/.cargo/bin/briefcase --root . \
  --scenarios evaluation/heldout-scenarios.json --output heldout.json --timeout 300
BRIEFCASE_TEST_BINARY=$PWD/target/release/briefcase node --test tests/*.test.mjs
```

Raw outputs (sync JSON, evaluator JSON, test logs) were kept in the session scratchpad
and are not committed.
