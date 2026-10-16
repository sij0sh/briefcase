# Briefcase self-evaluation: findings

Date: 2026-10-16 · Commit evaluated: `80a4b0f` · Host: Fedora 44, cargo 1.98.0, rust-analyzer 1.98.1

The stated objective (README / TASK_CONTEXT): *"trace retained evidence from a
codebase to build a case around a task, delivered as a brief"*, with explicit
source/coverage/provenance, typed gaps, and honest completeness. The task
evaluator measures labelled file recall, explicit gaps, false-complete outcomes and
delivery.

This run installed Briefcase, indexed its own repository, ran the project's
evaluator (`scripts/task-evaluate.py`) on 8 hand-labelled scenarios, and checked
each core read (`brief`, `search`, `occurrences`, `neighbors`, `page`, `explain`)
by hand.

## TL;DR

| Objective | Verdict |
| --- | --- |
| Builds from source | ✅ `cargo install --path . --locked` in ~1 min. 6 dead-code/unused-import warnings |
| Initial DB build | ⚠️ `sync` **overran its 600 s budget (719 s) and failed** with `"sync":"timeout"`. A re-run then reported `up-to-date` |
| Honest completeness (no false "complete") | ✅ 0/8 false-complete. Every case is `partial`, with typed dimensions |
| Required-file recall | ⚠️ 27/36 (75%) of labelled paths. 2/8 scenarios pass |
| Precision / focus of the brief | ❌ Slices hold 6–110 files. Four of the eight return over half of the repo's 199 files |
| Gap inventory as a usable checklist | ❌ 1.6k–31k `not_covered` rows per case, >99% `producer_boundary` noise |
| `neighbors` | ❌ **Returns zero items on this repo**, even at max budget (≈19 MB of metadata per response) |
| `occurrences` | ✅ Exact. Matches `git grep` |
| `explain` / provenance | ✅ Every slice row's `via_link` resolves to an inspectable assertion or witness |
| `search` | ➖ Works. Ranking is noisy (the right file came 5th for the analyzer query) |
| History / co-change joins | ✅ Commits and co-change are joined to cases |
| Session joins | ➖ 12 sessions imported. Nothing in this repo let me test a positive join |
| Speed of reads | ✅ First brief in 1.3–3.3 s. ❌ Full paging takes up to 42 s (mostly gap rows) |
| Storage | ⚠️ 2.4 MB of tracked source becomes a **328 MB** DB, about 67 MB of it retained reports after ~30 queries |

Bottom line: the evidence and provenance model works. Every row is explainable,
completeness is never overclaimed, and literal enumeration is exact. The "case around
a task" is weak in practice: it is either too broad (hub-file blowup) or misses the
direct test and implementation files, and its gap list is too large to act on. A
`git grep` with obvious terms beat it on both recall and size (see the caveat
under Recall).

---

## 1. Install and initial build

```sh
cargo install --path . --locked                      # 1m02s, OK
briefcase --json sync --language rust --timeout 600  # failed: timeout after 718.8 s
briefcase --json sync --git                          # 0.17 s, 3 commits
briefcase --json sync --sessions all                 # 6.3 s, 8 Claude Code + 4 Pi sessions
briefcase --json sync --language rust --timeout 300  # 8.7 s, "up-to-date", ready: true
```

I did not use `briefcase init`, because `init --dry-run` showed it would write
hooks into `.claude/settings.json`, `.codex/hooks.json` and `.pi/extensions/`.
`sync` builds the same DB without changing agent configuration.

### Findings

1. **The sync budget is not enforced.** With `--timeout 600`, it ran 719 s and
   then reported `status: failed, sync: timeout`. Git history was never synced,
   because that step queues behind static call analysis.
2. **Rust static call analysis is mostly unresolved.** The `CALLS_STATIC` run
   attempted 23,470 call sites, `completed` 4,096, and left 22,747 unresolved
   (about 3% resolved). rust-analyzer sat idle (1 s of CPU in 10 min) while
   Briefcase waited. Likely cause: `analyzer.rs` `definition()` retries each null
   result for up to 5 s, and with `noDeps`, `sysroot:null` and proc-macros off,
   rust-analyzer returns null for most sites. Each one burns the retry window.
3. **A partial run is cached as final.** `ensure_rust_calls` (`sync/project.rs`)
   returns `"retained"` once *any* `CALLS_STATIC` run exists. The 3%-resolved
   run is never retried, and later syncs report `ready: true`.
4. **The first sync leaves the project domain "dirty".** `status` showed
   `ready:false` with `project: dirty` until a second sync was run.
5. **DB size.** 328 MB for a 2.4 MB repo. Largest tables: `retrieval_units` 69 MB,
   `reports` 67 MB, `records` 43 MB. Each retained case carries its full
   gap inventory, so `reports` grows quickly with use.

## 2. Task-case recall (project evaluator)

I ran `scripts/task-evaluate.py --scenarios scenarios.json` with 8 scenarios.
Each one names the files a developer would need. I derived the labels from
`git grep` and by reading the code (e.g. tests are included via `#[path]` from
`tests/context.rs`, `core.rs` and `conformance.rs`).

| Scenario | Target / duty | Recall | Slice size | Missing |
| --- | --- | --- | --- | --- |
| duty-variant-refactor | `task/duty.rs` / extend | 6/7 | 45 | `src/bin/briefcase/command.rs` |
| task-case-schema | `task-case.schema.json` / extend | 3/6 | 17 | `task/case.rs`, `tests/briefcase_cases.rs`, `contracts/v1/README.md` |
| analyzer-timeout | `analyzer.rs` / debug | 5/5 ✅ | **110** | — |
| git-history-limits | `git_history.rs` / refactor | 4/4 ✅ | **104** | — |
| pi-extension | `briefcase-pi.ts` / refactor | 3/4 | 12 | `scripts/verify-pi` |
| case-tests | `task/case.rs` / **test** | 1/3 | 6 (0 tests!) | `tests/briefcase_cases.rs`, `tests/context.rs` |
| new-harness (no target) | — / extend | 4/5 | **106** | `profiles/sessions/codex-v1.json` |
| occurrences (no target) | — / debug | 1/2 | **108** | `task/queries.rs` (was an anchor candidate!) |

Totals: **27/36 recall (75%)**, 2 pass, 6 miss, 0 errors, **0 false-complete**,
median first brief 2.1 s, full delivery 1.6–42 s.

**Naive baseline:** `git grep -lE '<obvious term>'` recalled **33/36** with
result sets of 10–44 files. Caveat: I chose the grep terms and the labels with the
answers in mind, so this baseline is optimistic. Even so, the brief should beat it
and does not.

### Why recall/precision suffer

- **Hub-file closure.** Links carry multi-file endpoints such as `from_files: [X,
  src/architecture/mod.rs]`. Depth-2 undirected closure reaches `mod.rs` or
  `src/lib.rs` and then pulls in nearly everything. `explain` shows
  `tests/briefcase-pi.test.mjs` in the rust-analyzer case through
  `TESTS → src/lib.rs`, and `contracts/v1/README.md` through a path reference
  to `identity.rs`. Slices are listed alphabetically with only a depth, with no
  ranking, so a reader cannot pick out the 5 relevant files among 110.
- **Test linkage stops at the module level.** Static `TESTS` links point at
  `src/architecture/task/mod.rs`, not `case.rs`. The `test` duty uses depth 1,
  so asking "which tests exercise task/case.rs" returns **zero test files**.
- **Ambiguous anchors are discarded.** For the untargeted occurrences task, the
  anchor candidates included `architecture::task::queries::occurrences`, which is
  correct, but `anchors.paths` was empty and the slice did not contain
  `queries.rs`. The slice came from a broad lexical/closure fallback instead.
- **Role classification.** `src/bin/briefcase.rs` and `src/bin/briefcase/*.rs`
  are placed in the `script` role, not `impl`.
- Co-change does surface `case.rs` for the schema task (33 co-changed files from
  the single big commit), but the evaluator scores only the slice.

## 3. Gaps / completeness

The completeness model behaves as documented. Dimensions are typed, `outcome`
is always `partial` here, `verification` stays `unknown`, and `history`
correctly flips `unknown → partial` after the Git sync.

The `not_covered` inventory cannot be used as "the remaining evidence checklist"
the instruction text promises:

| Case | not_covered rows | of which `producer_boundary` |
| --- | --- | --- |
| case-tests | 1,618 | 1,606 |
| task-case-schema | 7,178 | 7,160 |
| analyzer-timeout | 30,446 | 30,433 |
| occurrences | 30,996 | 30,982 |

Fewer than 20 rows per case are meaningful (`language_profile` ×8, one per
missing artifact role, `history`, `sessions`, `verification`). The rest are
one row per unresolved call site from finding 2. Gap counts also changed between
two identical queries (7,454 → 12,758) while the sync was still writing, which
is consistent with "retained evidence" but surprising to a user.

## 4. Other reads

- **`neighbors` returns no results.** Every call returned `returned: 0`,
  `outcome: partial`, `omissions.unrendered_response_bytes ≈ 19.3 MB`, even with
  `max_bytes: 1048576` and `total: 21–45`. Paging the pinned report gives the same.
  Cause: `src/architecture/task/queries.rs:92` sets `result["coverage"] =
  ix.coverage.clone()`. That puts the whole index coverage, including all
  producer boundaries, in every response, so metadata alone exceeds the
  1 MiB cap. The README's headline examples (`neighbors … --predicate TESTS`)
  do not work on a Rust repo of this size.
- **`occurrences "DutyPolicy"`**: 5 matches in `duty.rs`, `mod.rs` and
  `CONSOLIDATION.md`, identical to `git grep`. `coverage.state:
  complete_for_retained_text`. ✅
- **`search "rust-analyzer deadline timeout"`**: the top hits were `sync/project.rs`,
  `native_host.rs`, a commit, `clang.rs`, then `analyzer.rs`. Acceptable but noisy.
  The response omitted 95 of 100 items by default within the 24 KB budget.
- **`explain`**: resolved every `via_link` I tried to its predicate, endpoints,
  basis and assertion provenance. ✅
- **History**: commits with messages and changed paths are attached to cases.
  Co-change is labelled as co-change, not dependency. ✅
- **Sessions**: 8 Claude Code and 4 Pi transcripts were imported (2,750
  other-repo transcripts were correctly skipped). No case joined a session.
  That is plausibly correct: the one past session's only edit was a gitignored
  `.agents/artifacts/*.md` file. A positive join remains unverified.

## 5. Recommendations (highest leverage first)

1. **Fix `neighbors` payload size**: return a coverage summary (counts), not
   `ix.coverage` verbatim, in `task/queries.rs`.
2. **Collapse `producer_boundary` gaps** into one aggregated row per
   producer/run with counts and a pageable detail section. That makes
   `not_covered` usable as a checklist and shrinks retained cases and the DB.
3. **Fix rust-analyzer analysis**: enforce the sync deadline end-to-end, and
   skip the 5 s retry once the workspace is known loaded (or wait for
   rust-analyzer's quiescent status). Investigate why so few definitions resolve
   (sysroot/noDeps). Do not treat a mostly-unresolved run as final.
4. **Damp hub nodes in closure**: don't expand through `mod.rs`/`lib.rs` re-export
   edges at full weight, and rank slice rows (depth, predicate strength,
   lexical score) instead of sorting alphabetically.
5. **Let the `test` duty follow `TESTS` up to the containing module**, or
   attribute test links to the files the tests actually call.
6. **Keep ambiguous anchor candidates** that carry file locations as low-confidence
   seeds instead of dropping them.
7. Classify `src/bin/**` as `impl`.

## Reproduce

Scratch artifacts from this run (scenarios, evaluator output, per-case JSON) are
in the Claude session scratchpad. They are not committed. To reproduce:

```sh
cargo install --path . --locked
briefcase --json sync --language rust --timeout 1200
briefcase --json sync --git && briefcase --json sync --sessions all
python3 scripts/task-evaluate.py --binary ~/.cargo/bin/briefcase --root . \
  --scenarios scenarios.json --output results.json --timeout 300
```

Scenario file (labels used above):

```json
{"scenarios": [
 {"id": "duty-variant-refactor", "task": "add a new Duty variant called migrate and wire it through parsing, schema and routing", "target": "src/architecture/task/duty.rs", "duty": "extend",
  "required_paths": ["src/architecture/task/duty.rs","src/architecture/task/request.rs","src/architecture/task/mod.rs","src/architecture/task/trace.rs","src/architecture/service/catalog.rs","src/bin/briefcase/command.rs","tests/briefcase_task.rs"]},
 {"id": "task-case-schema", "task": "add a required field to the task case JSON schema", "target": "contracts/v1/task-case.schema.json", "duty": "extend",
  "required_paths": ["contracts/v1/task-case.schema.json","src/architecture/contracts/schema.rs","src/architecture/task/case.rs","tools/test_task_case_schema.py","tests/briefcase_cases.rs","contracts/v1/README.md"]},
 {"id": "analyzer-timeout", "task": "rust-analyzer static call analysis ignores the sync timeout and hangs; fix the per-request retry deadline", "target": "src/architecture/analyzer.rs", "duty": "debug",
  "required_paths": ["src/architecture/analyzer.rs","src/architecture/execution.rs","src/architecture/rust.rs","src/architecture/sync/project.rs","tests/briefcase_execution.rs"]},
 {"id": "git-history-limits", "task": "raise the git history max_commits limit and make it configurable", "target": "src/architecture/git_history.rs", "duty": "refactor",
  "required_paths": ["src/architecture/git_history.rs","src/architecture/sync/git.rs","src/architecture/search.rs","tests/briefcase_history.rs"]},
 {"id": "pi-extension", "task": "change how the Pi extension records post-task coverage case IDs", "target": "extensions/briefcase-pi.ts", "duty": "refactor",
  "required_paths": ["extensions/briefcase-pi.ts","src/architecture/pi.rs","tests/briefcase-pi.test.mjs","scripts/verify-pi"]},
 {"id": "case-tests", "task": "which tests exercise task case retention and loading", "target": "src/architecture/task/case.rs", "duty": "test",
  "required_paths": ["src/architecture/task/case.rs","tests/briefcase_cases.rs","tests/context.rs"]},
 {"id": "new-harness-untargeted", "task": "add support for importing sessions from a new agent harness similar to codex", "duty": "extend",
  "required_paths": ["src/architecture/session/harness/mod.rs","src/architecture/session/harness/codex.rs","profiles/sessions/codex-v1.json","src/architecture/sync/sessions.rs","src/architecture/integrations.rs"]},
 {"id": "occurrences-untargeted", "task": "occurrences response budget pagination skips undelivered matches", "duty": "debug",
  "required_paths": ["src/architecture/task/queries.rs","tests/briefcase_task.rs"]}
]}
```
