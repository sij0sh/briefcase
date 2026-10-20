# P2 implementation

This implements §8 P2 of `.agents/artifacts/BRIEFCASE_COMPREHENSIVE_CONTEXT_RECOMMENDATIONS.md`. It follows the §8.1 constraints:

- one evidence store and authority model;
- existing duty policy and typed gaps;
- one small reference driver plus a report record;
- lossless access before ranking;
- no new rank weights, role quotas, services, vector store, ML or SDK.

## P2a: resumable discovery

- **File-level lexical discovery.** `discover` (`src/architecture/task/retrieval.rs`) asks FTS for distinct matching files through `Ledger::recall_files`, not for chunks. Many duplicate chunks from a few distractor files therefore cannot fill the candidate bound. Up to `LEXICAL_FILE_LIMIT` (1000) files become candidates. `/coverage/retrieval/lexical_discovery` records the matched/admitted counts, the separate lexical report's `report_id`, and an executable `continue_with`. Follow that continuation with `page`, using the lexical report ID and section `/items` (overflow starts at offset 1000). `/candidates` contains the fused case candidates; it does not contain the unadmitted lexical tail. Reading the lexical report does not promote its tail into ranking or recompute case assembly. [Scale Priority 1](SCALE_PRIORITY1.md) adds compact `inventory` reads and explicit `expand_case` promotion into an immutable child case.
- **Task terms.** `term_plan` orders terms by kind: explicit identifiers first, then their parts (so `parseConfig` also matches `parse_config`), then ordinary words. A long task can no longer push a late identifier out of the term budget. Omitted terms are reported, and the omission triggers a `candidate_selection` gap.
- **Graph expansion.** `expand` records its coverage. When expansion stops early, it emits up to `GRAPH_CONTINUATIONS` (16) explicit continuations instead of dropping the frontier.
- **Session joins.** `joins.rs` applies `SESSION_LIMIT` after repository filtering, so sessions from other repositories cannot use up the bound.
- **Not changed:** upstream capture and index limits. Exclusions, oversize files and unanalysed sources still come back as typed gaps. P2 makes them visible and resumable; it does not raise them.

## P2b: lazy source access and measured work

- **Lazy snapshot contents.** `Snapshot.contents` (`evidence.rs`, `Contents`) loads file bytes on demand. Sizes come from `Ledger::payload_sizes` without reading payloads. Every load is still digest-checked, and `Contents::work` reports files loaded versus files in the snapshot.
- **Occurrences.** `queries.rs` uses KMP matching. The first page builds and caches an `occurrence-counts` inventory. Later pages read only the files inside their window, and each response carries `work` stats. In the 41-file test the first page loads 45 files (40 scanned plus 5 for the window), later pages load at most 6, and the paged rows match one exhaustive page.
- **Integrity is unchanged.** Corrupting a retained payload makes a lazy read fail with a digest error. Files outside that read still work.

## P2c: monorepo scope and federation

- **Monorepo scope.** `retrieval::project_scope` assigns each file to its nearest manifest directory. The manifests recognised are Cargo.toml, package.json, pyproject.toml, setup.py, go.mod, pom.xml, build.gradle(.kts) and *.csproj. When candidates span more than one project, the case reports `retrieval.project_scope`:
  - candidate projects ordered by best rank;
  - which projects had nothing selected;
  - up to 16 `continue_with` calls to `agent_context` anchored at each unselected project root.

  A single-project repository reports `single_project` and nothing else.
- **Federation.** `scripts/task_federation.py` and the CLI `scripts/task-federate.py` coordinate two or more repositories. Each repository keeps its own root, database and capture. The coordinator works as follows:
  - **Pinning:** it pins each repository through `agent_context`, `status` and `worktree`, then records a version vector of view, case, index, git revision, tree digest and alignment.
  - **Edges:** it reads root manifests through digest-verified `explain` pages. Edges come only from declared dependencies and from explicit `contracts` in the workspace file. Name similarity never creates an edge.
  - **Consumer evidence:** it pages consumer `occurrences` with qualified refs (`repo@view:path`) and records `continue_with` when the occurrence pages are not exhausted.
  - **Boundaries:** an unavailable repository, or a live tree that differs from the capture, is recorded as a boundary, and coverage is reported as `partial`.
  - **Recheck:** it rechecks every pin at the end. A changed view invalidates the whole run.
  - **Compatibility:** this is reported as `declared_only`. A vector of snapshots is not a coordinated release.

  Workspace file:

  ```json
  {"repositories": [{"id": "api", "root": "../api"}, {"id": "web", "root": "../web", "db": "web.db"}],
   "contracts": [{"provider": "api", "path": "schema/api.json", "terms": ["getUser"], "consumers": ["web"]}]}
  ```

  Run `briefcase sync` in each repository first; the coordinator only reads existing pins. Then run:

  ```sh
  python3 -B scripts/task-federate.py --workspace workspace.json --task 'change getUser' \
    --binary target/release/briefcase --output federation-run
  ```

  The command exits with code 2 when it is blocked.

## P2d: independent task corpus

- **Corpus generator.** `scripts/task_corpus.py` deterministically generates seven fixture families: hidden_caller, config_default, contract_rename, late_identifier, distractor_scale (1100 notes), deep_chain and duplicate_chunks. Each fixture has:
  - a task with no path hints;
  - an answer key: required evidence paths, an executable check and a reference patch;
  - a held-out split assigned per whole repository by a stable hash.
- **Evaluator.** `scripts/task-corpus-evaluate.py` processes each fixture in turn:
  1. It validates every oracle: the check must fail on the fixture and pass with the reference patch.
  2. It runs the reference `Investigation` driver blind, giving it only the task text.
  3. It scores a funnel for each required path: candidate, selected, digest-verified source.
  4. It runs the external harness only when `--harness-command` is given. The harness edits a copy, and the fixture check then decides the outcome. Without it, outcomes are `not_run` and are never inferred from retrieval.

  Errors count as failures.

### Measured run (seed 0, 4 variants, 28 fixtures, 400 driver steps)

| group | fixtures | errors | oracles valid | required paths | candidate | selected | verified source | mean calls |
|---|---|---|---|---|---|---|---|---|
| all | 28 | 0 | 28 | 48 | 48 | 44 | 48 | 133.4 |
| dev | 25 | 0 | 25 | 42 | 42 | 39 | 42 | 147.2 |
| held_out | 3 | 0 | 3 | 6 | 6 | 5 | 6 | 18.3 |

How to read these numbers:

- **Selection misses.** All four selection misses are `config_default/*/settings.py`, ranked 10–13. The driver still fetched and digest-verified each one through candidate paging.
- **Step cap.** distractor_scale and duplicate_chunks reached the 400-step cap with 19 and 12 actions still pending. The required file was verified before the cap, and the pending work is kept for resumption. These two families account for the high mean call count.
- **Sample size.** The hash split held out only 3 of 28 repositories here. Treat the held-out row as a small sample, and use more variants or seeds for a stronger split.
- **What the numbers measure.** They measure evidence access, not task success. The outcomes column is `not_run` for all 28 fixtures because no external harness was run.

Reproduce with:

```sh
cargo build --release
python3 -B scripts/task-corpus-evaluate.py --binary target/release/briefcase --output /new/dir --variants 4
```

## Tests

- **New Rust tests.** `tests/briefcase_p2.rs` has 6 real-ledger tests: duplicate distractor chunks, paging past the 1000-candidate bound, late identifiers, occurrence work per page, lazy-read corruption, and the monorepo follow-up. `term_plan` also has a new unit test.
- **New Python tests.**
  - `scripts/test_task_federation.py`: 7 protocol tests plus a two-repository smoke test against the real binary. The smoke test checks that the revision equals git HEAD, that there is an api→web edge, and that an unavailable repository becomes a boundary.
  - `scripts/test_task_corpus.py`: 5 tests.
  - All 65 Python tests pass: `python3 -B -m unittest discover -s scripts -p 'test_*.py'`.
- **Rust suites.**
  - lib 134 passed; p2 6; p0p1 7; stage2 3; stage4 4.
  - **core:** 9 failures, all of which also fail at 99271b9 (13 fail there).
  - **stage3:** `invalid_profile_preflight_reports_configuration_without_creating_a_database` also fails at clean 99271b9 with `"sync":"locked"` in full-suite runs, and passes alone.
  - **context:** `refactor_packet_surfaces...` fails on a preview-truncation assertion. This was not rechecked at 99271b9.
