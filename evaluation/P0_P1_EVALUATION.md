# P0/P1 integration check and evaluation rerun

Date: 2026-10-18. Base commit: `c3e67be` (P0/P1 as delivered). The fixes in this
document are uncommitted working-tree changes on top of that commit.

Inputs: `IMPLEMENTATION_STATUS.md` (P0/P1 delivery notes) and
`.agents/artifacts/BRIEFCASE_COMPREHENSIVE_CONTEXT_RECOMMENDATIONS.md` (§9 paired
experiment, P0/P1/P2 items).

## Summary

- **Integration:** all P0/P1 files are wired in: the `worktree` operation, candidate
  `source_ref`, `task-investigate.py`, `validate-p0-p1.py`, the eval flags and the
  `verify-context` gate. As delivered, though, the reference workflow could not run
  on the real repository at an 8 KB budget. Five protocol bugs blocked it, and the
  same bugs made `task-evaluate.py --verify-sources --source-budget 8000` error on
  6 of 8 task scenarios. All five are fixed here, with regression tests.
- **P0 (reproducible, verified eval):** works after the fixes. On both sets, every
  delivered required path was digest-verified, with 0 errors, 0 false-complete
  outcomes and no "implementation changed" invalidation. The historical-baseline
  check is still blocked, because the corpus (`evaluation/fixtures`,
  `evaluation/sources`) is not in the repository.
- **P1 (follow-up recovery):** works as intended in one sense. A blind driver that
  follows candidates and the graph from the first packet recovered **31/36**
  required paths on the task set and **19/19** on held-out. The first packet alone
  carries 24/36 and 12/19. Retained selection at cap 32 gets 30/36 and 15/19. The
  workflow recovered every Stage 4 held-out "known miss" (`command.rs`,
  `native_host.rs`, `sync/project.rs`) and `integrations.rs` on the task set.
- **The recovery is expensive and has low precision.** It took about 300 calls,
  1.6–2.1 MB and 130–265 s per scenario, and digest-verified 54–74 files to find
  2–7 required ones. Recall per file read is roughly what raising the selection cap
  gives: Stage 4 at cap 64 reached 34/36 and 19/19. P1 makes recovery *possible
  and verifiable*. It does not yet make it *cheap*. The next lever is the size of
  page envelopes and freshness metadata, not ranking.

## 1. Integration check and fixes

| # | Problem found | Effect | Fix |
|---|---|---|---|
| 0 | `tests/briefcase_p0_p1.rs` did not compile (type inference on `reconstructed`). `p0-p1-smoke.py` raised `StopIteration` because it read only the first `/candidates` page. | `verify-context` gate and smoke both failed | Annotated the type. The smoke now follows `continue_with`. |
| 1 | Budget reduction of `status` dropped `current_view` / `pending_or_interrupted` | The workflow saw a "pin changed" and invalidated a valid investigation | `response::notice` keeps the pin keys (`current_view`, `pending_or_interrupted`, `retracted_count`, `ledger_watermark`). `check_current` raises "increase --max-bytes and resume" instead of invalidating when they are absent. |
| 2 | A single `/candidates` row (about 6 KB of signals and spans) was larger than the page budget | Candidate paging stalled at offset 0 | `fit_metadata_page` binary-searches the largest fitting prefix. If even one row doesn't fit, it strips `signals`/`spans` and records `/items/ranking_detail_bytes` as omitted. |
| 3 | The freshness diagnostic took about 5.3 KB of an 8 KB page | `/not_covered` pages returned zero rows and stalled | When a page or source read is over budget, the full freshness block becomes `freshness::agent_summary`, marked `presentation: "summary_by_budget"`. Pins stay at the top level. |
| 4 | The same freshness overhead hit source reads: about 4.9 KB of metadata and 1.85 KB of text per 8 KB page (≈23% efficient) | `task-evaluate --verify-sources` errored with "Non-contiguous or stalled source pagination" | Same compaction as #3. The workflow's `source_bytes` went from budget/3 to budget/2. |
| 5 | The workflow scheduled continuations breadth-first | `analyzer-timeout` ran 600 steps, 603 calls and 4.4 MB without finishing any required file | Continuations go to the front of the queue (`enqueue(front=True)`), so a started file finishes before the next one starts and rank order decides which files are complete. |

New tests in `tests/briefcase_p0_p1.rs`:

- `oversized_status_keeps_the_pins_that_freshness_checks_compare`
- `candidate_pages_progress_when_one_full_row_exceeds_the_budget`
- `small_pages_and_source_reads_compact_freshness_before_rows_or_bytes`

There is also a new Python test,
`test_budget_truncated_status_blocks_without_invalidating`.

`Cargo.lock` also changed. That diff only re-sorts the `briefcase` package entry
to match `Cargo.toml`.

### Gate status (`scripts/validate-p0-p1.py`, fixed tree)

8 checks passed, 1 failed and 1 was blocked.

**Passed:**

- release build
- Python (52 tests)
- tools/schemas
- owned schemas
- Node adapter (17)
- debug build
- installed Pi (16)
- real-protocol smoke, which covers follow-ups, a source budget of 2600 B,
  out-of-slice recheck and fresh-pin rebind

**Failed: `rust-context`.** The only failure is
`briefcase_task::refactor_packet_surfaces_tests_plans_scripts_configuration_and_install_touchpoints`.

- It is **pre-existing**: it fails the same way at `616994f` (Stage 4), before P0/P1.
- **Cause:** the brief preview shows 10 `not_covered` rows, but the case has 12, so
  `executed_test_coverage` gets cut off. A `brief.rs` unit test expects that
  truncation.
- **Recommendation:** order verification gaps first in `/not_covered`, or exempt
  them from the preview limit.
- Because cargo stops at the first failing test binary, the gate run did not
  execute stage2/3/4/p0p1. Run separately, all of them pass: lib 133, stage2 3,
  stage3 4, stage4 4, p0p1 7.
- The stage3 test `invalid_profile_preflight…` is flaky under load.

**Blocked: historical baseline.** `evaluation/fixtures/index.json` and
`evaluation/sources` are untracked. For the same reason, the `core` and
`toolchain_smoke` suites can't run here.

## 2. Evaluation method

- **Corpus:** a scratch copy of the retained DB, synced with the fixed binary.
  The sync took 246 s, and `git status` was unchanged before and after. The user
  DB was not touched.
- **Arm A:** `task-evaluate.py --diagnostics --verify-sources --source-budget 8000
  --harness pi`. This is retained selection at cap 32, with every selected file
  fetched through `explain` pages and digest-verified.
- **Arm B:** a blind driver around `task_investigation.Investigation`.
  - The driver sees only `task` / `target` / `duty`.
  - Each call has budget 8000 and `max_bytes` 7488.
  - Each scenario is capped at 300 steps.
  - Required paths are used only afterwards, to score the step, call, byte and
    second at which each path was first digest-verified.
  - No scenario exhausted its queue in 300 steps.
- `--verify-candidates` was not used. With it, the first arm-A attempt hit a
  1500 s timeout without writing results. That cost is itself a finding.

## 3. Results

### Required-path recall

| | Task set (36 paths, 8 scenarios) | Held-out (19 paths, 5 scenarios) |
|---|---|---|
| Stage 4 retained, cap 32 / 8 KB (`NEXT_IMPROVEMENTS.md`) | 32/36 | 16/19 |
| Stage 4 retained, cap 64 / 8 KB, selected | 34/36 | 19/19 |
| **Arm A, pre-fix binary (`c3e67be`)** | 6 of 8 scenarios **errored**: stalled source pagination | not run |
| **Arm A, fixed binary**: selected = delivered = digest-verified | **30/36** | **15/19** |
| Arm B, required paths in the first 8 KB packet | 24/36 | 12/19 |
| **Arm B, recovered and digest-verified within 300 steps** | **31/36** | **19/19** |
| Union of A and B | 32/36 | 19/19 |

Arm A's delivery recall was 1.0 in every scenario. Every selected required path
was in the Pi manifest (about 6.7–7.0 KB) and its source bytes verified.

**Why arm A is below Stage 4 at cap 32 (30 vs 32, 15 vs 16): corpus drift, not a
ranking regression.** The corpus now contains the P0/P1 files themselves:
`tests/briefcase_p0_p1.rs`, `scripts/test_task_investigation.py`,
`scripts/p0-p1-smoke.py`, `scripts/task_investigation.py`,
`scripts/validate-p0-p1.py` and `P0_P1_IMPLEMENTATION.md`. These files match the
task vocabulary heavily, and each arm-A slice in the task set took 1–5 of them.

The new misses beyond the Stage 4 known-miss list were exactly the files that sat
just below the cut:

| Scenario | Newly missed path | Evidence rank | P0/P1 files in the slice |
|---|---|---|---|
| `duty-variant-refactor` | `task/trace.rs` | 39 | 3 |
| `case-tests` | `tests/context.rs` | 69 | 5 |
| `H2-claude-code-discovery` | `sync/sessions.rs` | 26 | 4 |

Stage 4's known misses are still missed: `command.rs`, `briefcase_history.rs`,
`verify-pi`, `integrations.rs`, `native_host.rs` and `sync/project.rs`. A
P2-worthy follow-up is to stop evaluation and acceptance scaffolding from
competing with product code for implementation slots.

### What arm B recovered that arm A did not

- **Task set:**
  - `task/trace.rs` (duty-variant-refactor)
  - `src/architecture/integrations.rs` (new-harness-untargeted; a Stage 4 known miss)
- **Held-out:**
  - `sync/sessions.rs` (H2)
  - `src/bin/briefcase/command.rs` (H3; a Stage 4 known miss)
  - `native_host.rs` (H4; a Stage 4 known miss)
  - `sync/project.rs` (H5; a Stage 4 known miss)
- **Never recovered by either arm:** `command.rs` (duty-variant), `briefcase_history.rs`,
  `scripts/verify-pi` and `tests/context.rs`.
- **Missed by arm B only:** `tests/briefcase_execution.rs` (analyzer-timeout). Arm A
  has it, but arm B's 300-step budget ran out on other files first.

### Cost

| | Arm A (cap 32, verify all) | Arm B (300 steps) |
|---|---|---|
| First packet | 4.4–5.0 s (median 4.6 s task, 4.8 s held-out) | same first call |
| Pi manifest bytes | 6.7–7.0 KB | — |
| Source verification | 141–235 pages, 62–167 s, 32 files (≈360 KB of source) | 303 calls, 1.62–2.06 MB of responses, 133–265 s, 54–74 files verified |
| Cost to match arm A's per-scenario recall | — | median ≈ 112 calls / 600 KB / 95 s |
| Cost to each scenario's last recovered path | — | 54–246 calls, 0.3–1.35 MB, 30–216 s |

Recovery order from arm B's first-verified data:

- Anchor files and first-packet files verify within about 10–40 calls.
- Paths outside the manifest arrive late, after 100–250 calls. Examples:
  `integrations.rs` at 246 calls / 1.35 MB, `native_host.rs` at 198 / 1.07 MB,
  `sync/project.rs` at 160 / 1.08 MB.
- Responses average about 5.4 KB per call, but a source call carries at most
  3.7 KB of text (`source_bytes = budget/2`). The rest is envelope: pins, the
  freshness summary, `continue_with`, and ranking metadata on candidate pages.
  The exact split per call was not measured.

## 4. Are the recommendations helping as intended?

| Recommendation | Intended effect | Observed |
|---|---|---|
| P0 reproducible eval (`--verify-sources`, `--trace-dir`, build receipt, source pin check) | Results tied to verified bytes and a pinned build | **Yes, after the fixes.** Before them, the verified-source mode could not complete at 8 KB. That bug went unnoticed because the delivered numbers came from protocols that never paged real sources at this budget. No false-complete outcomes. The historical baseline is still blocked (missing corpus). |
| P0 `--verify-candidates` | Check every candidate's source | Works, but is too slow to use routinely: more than 1500 s per set. |
| P1 candidate `source_ref` | Fetch an unselected candidate without re-ranking | **Yes.** Arm B reached unselected candidates (rank 26–97) and verified their digests. |
| P1 `worktree` recheck | Detect out-of-slice drift | **Yes.** The smoke detects modify, add and delete outside the slice (3/3) and invalidates the investigation. A fresh pin rebinds cleanly. |
| P1 reference investigation workflow | Recover required paths beyond the first packet, blind | **Yes for recall:** +7 task and +7 held-out paths over the first packet, and +1 / +4 over retained cap 32. **Not yet efficient:** reading about 60 files per scenario gives roughly the recall of a cap-64 selection, at about 1.6 MB and 4 min per scenario. |

**Caveat.** Arm B measures *protocol reachability*: whether a blind agent can reach
and verify the right bytes using only Briefcase continuations. It does not measure
discovery by a reasoning agent, which would prune the queue. The driver follows
every follow-up in workflow order and never decides that a file is irrelevant.

## 5. Recommended next steps (P2)

1. **Shrink page envelopes.**
   - Source pages: send the pins and a freshness digest once per investigation,
     not per page.
   - Have `continue_with` reference the previous arguments rather than repeating them.
   - Goal: more than 6 KB of source text per 8 KB call, which would roughly halve
     arm B's bytes and calls.
2. **Rank the follow-up queue.** Prefer candidates with graph or occurrence evidence
   over lexical-only ones, and skip files whose digest is already known.
   Out-of-manifest hits currently arrive after 100–250 calls.
3. **Keep evaluation and acceptance scaffolding out of implementation slots.**
   This covers `tests/briefcase_p0_p1.rs`, `scripts/test_*`, `*_IMPLEMENTATION.md`
   and similar files. Their arrival cost 2/36 and 1/19 at cap 32.
4. **Fix the `not_covered` preview truncation** that keeps the `verify-context`
   gate red (see §1). Add `--no-fail-fast` to the gate so one failure doesn't hide
   the later suites.
5. **Make `--verify-candidates` practical.** Verify only the candidates the workflow
   actually follows, or reuse the digests from sync.
6. **Restore the historical corpus** (`evaluation/fixtures`, `evaluation/sources`)
   so the blocked baseline and the `core` and `toolchain_smoke` suites can run.

## Reproduction

```sh
cargo build --locked --release            # or CARGO_TARGET_DIR=<scratch> for an isolated binary
# Arm A (per set: task, heldout)
python3 -B scripts/task-evaluate.py --binary target/release/briefcase --root . \
  --db <scratch>/retained.db --diagnostics --verify-sources --source-budget 8000 \
  --harness pi --scenarios evaluation/task-scenarios.json --output <out>.json
# Gate
python3 -B scripts/validate-p0-p1.py --output <new-dir>
```

The arm-B driver used `scripts/task_investigation.Investigation` with a
`call(op, args)` wrapper. The wrapper ran `briefcase --json call <op> --args …`
at `max_bytes = 7488` and recorded calls, bytes and seconds. Required paths were
read from the scenario files only for scoring.
