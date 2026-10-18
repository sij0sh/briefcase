# Next improvements: bang-for-buck analysis

Date: 2026-10-17 · Baseline: Stage 4 tree (`616994f`) · Fresh DB in scratch, not committed.

Sources: `task-controls.py --control-ablations` run (retained and clean-clone, original
and held-out), per-path diagnostics, a manifest captured from `brief --agent-context`,
and a cap/budget sweep with `task-evaluate.py` on the retained DB. Raw outputs are in
the session scratchpad and are not committed.

## 1. Where we are

| Condition | Original (36 paths) | Held-out (19 paths) | Notes |
| --- | --- | --- | --- |
| Prior rerun, retained | 27/36 | 17/19 | Manifest delivered 1 file |
| Stage 4, retained, cap 32, 8 KB | **32/36** | 16/19 | Delivery matches brief recall |
| Stage 4, clean clone, cap 32 | 27/36 | 17/19 | Sessions and history help duty tasks |
| Cap 48, 8 KB | 32/36 | **18/19** | No required path trimmed |
| Cap 64, 8 KB | 34/36 brief, **32/36 delivered** | 19/19 brief, **18/19 delivered** | Extra files trimmed by the budget |
| Cap 64, 12 KB | **34/36 delivered** | not run | All 64 selected files delivered |

Delivery is no longer the problem it was. The remaining gap is in two places: how many
files fit the 8 KB agent budget, and retrieval that never surfaces some files.

## 2. What is binding

### 2.1 The byte budget binds before the file cap

- A manifest at cap 32 uses about 6.6 KB. About 2.6 KB of that is fixed metadata
  (`instruction`, `coverage`, `continue_with`, `sections`, `pinned_views`, `fetch_*`),
  and the file list is 4.4 KB.
- Each file entry is about 141 bytes: `path`, `rank`, `ref`, `role`, `source_offset`
  and `why`. `ref` and `source_offset` are the bulk of the avoidable bytes.
- At cap 48 and cap 64 on an 8 KB budget, only 40–42 files are delivered. The rest are
  listed in `omitted_selected_paths`.
- At cap 64 on original, the two newly recalled required paths are trimmed:
  `tests/briefcase_history.rs` and `scripts/verify-pi`. So delivered recall stays at
  32/36. Raising the cap alone does not help.
- At 12 KB, cap 64 delivers all 64 selected files and reaches 34/36. The budget is the
  constraint, and the retrieval is capable of more.

### 2.2 Every missed required path is a candidate

Every miss on both sets was present in the candidate pool with signals. None was
absent for lack of evidence.

| Scenario | Missed path | Evidence rank | Facets | Recovered by |
| --- | --- | --- | --- | --- |
| duty-variant | `src/bin/briefcase/command.rs` | 93 | history, lexical | — |
| git-history | `tests/briefcase_history.rs` | 87 | execution, lexical | cap 64 (then trimmed) |
| pi-extension | `scripts/verify-pi` | 67 | dependency, lexical | cap 64 (then trimmed) |
| new-harness | `src/architecture/integrations.rs` | 59 | dependency, history, lexical | — |
| H3 watch-poll | `src/bin/briefcase/command.rs` | 23 | dependency, history, lexical | cap 48 |
| H4 disposable | `src/architecture/native_host.rs` | 32 | execution, lexical | cap 64 (then trimmed) |
| H5 python-roots | `src/architecture/sync/project.rs` | 27 | dependency, execution, history, lexical | cap 48 |

Those misses are ranking misses, not discovery misses. The path pool is fine; the
cut-off and the ordering are what drop them.

### 2.3 Retained context can displace useful clean-clone files

On H4, `native_host.rs` is missed in the retained run but found in the clean clone
(2/2 clean versus 1/2 retained). The retained evidence fills the slots that the clean
run gave to it. This is a small effect, but it means more retained evidence is not
automatically better, and delivery order matters.

### 2.4 Facet ablation: lexical and sessions carry the weight

| Facet removed | Original: required paths lost / gained | Held-out: lost / gained |
| --- | --- | --- |
| lexical | 4 / 1 | 5 / 0 |
| sessions | 5 / 0 | 0 / 0 |
| dependency | 3 / 0 | 1 / 1 |
| history | 2 / 0 | 0 / 0 |
| execution | 1 / 1 | 2 / 1 |
| structure | 1 / 0 | 0 / 0 |
| tests | 1 / 1 | 0 / 2 |

`lexical` is the strongest signal and its removal costs the most. `sessions` only
matters on the original set, and mainly for the duty task. `domain`, `data` and
`runtime` make no difference on either set.

## 3. Ranked candidates

Ranked by expected gain per unit of effort. Gains are measured on the existing sets
unless marked as an estimate.

### #1. Compact the manifest so more files fit in 8 KB (highest)

- **What:** shrink each file entry from about 141 bytes to about 90–100: drop
  `source_offset`, fold `ref` into the fetch template, and shorten `why` to a single
  primary reason. Cut the fixed metadata from about 2.6 KB to under 1 KB by moving
  `instruction`, the full `coverage` object and `pinned_views` behind a continuation.
- **Why it wins:** it is the only change the sweep shows is needed. The cap-64 and
  12 KB runs already prove the retrieval can deliver 34/36 if the bytes are there.
- **Estimate:** at about 100 bytes per file plus about 1 KB metadata, 64 files fit in
  roughly 7.4 KB. Original delivered recall should rise from 32/36 toward 34/36 with
  no change to retrieval. This is an estimate until it is implemented and measured.
- **Effort:** small to medium. The change is in the `agent_context` assembly and the
  byte budgeter. The Pi adapter and any consumer of `ref` must be updated together.
- **Risk:** the agent loses inline reasons and refs. Keep `ref` available through the
  fetch template so every omitted file still has a pinned continuation.

### #2. Raise the agent-manifest file cap from 32 to 48

- **What:** default the `agent_context` cap to 48 and let the byte budget decide how
  many are delivered.
- **Evidence:** held-out delivered recall goes from 16/19 to 18/19 with no required
  path trimmed. Original stays at 32/36. Nothing is lost, and the cost is one constant.
- **Caveat:** cap 64 on its own is worse than 48 in delivery (it trims two required
  paths on original and one on held-out). So do this only after #1, or with a cap that
  matches the budget. Without #1, cap 48 is the safe ceiling.
- **Effort:** trivial. Re-run the cap sweep to confirm.

### #3. Delivery diversity by role for `debug`, `refactor`, `extend` and `test`

- **What:** guarantee a small quota of `test`, `script` and `bin` files in the delivered
  set rather than letting `impl` and `doc` fill it.
- **Evidence:** the misses that remain on original are a bin file (`command.rs`), a
  test file (`briefcase_history.rs`) and a script (`verify-pi`). Each is a role that
  loses ranking to `impl` (17 selected in the duty set). Roles in the competitor counts
  are heavily `impl`-weighted.
- **Effort:** medium. It belongs in the delivery layer, not in evidence scoring, so
  the global ranking and its provenance stay intact.
- **Caveat:** this is a hypothesis. It has not been measured as a change. Test it with
  the cap sweep and check that no required `impl` file is displaced.

### #4. Improve lexical matching

- **What:** identifier-aware tokenization for lexical retrieval (split snake_case,
  kebab-case and CamelCase; match path segments as well as text).
- **Evidence:** removing `lexical` is the largest single loss (−4 original, −5
  held-out). The signal is strong, and better matching could lift the candidates whose
  only match is lexical.
- **Effort:** medium. Changing lexical scoring is risky because it shifts every
  ranking. Measure with the full controls run, not just recall.

### #5. Test linkage through `#[path]` modules

- **What:** the Stage 4 sync already reports 166 module inclusions and 7 test targets.
  Propagate `TESTS` edges from each test target's referenced implementation files so
  that duty `test` finds the test file for a source file.
- **Evidence:** `briefcase_history.rs` and `briefcase_cases.rs` are the tests that the
  test-duty and history tasks need. Only the module-level linkage is in place.
- **Effort:** medium to large. Touches the Rust analyzer and the test-attribution rules.

### Lower priority, not recommended now

- **Static call resolution (about 12% of sites, 18,301 unresolved, 3,837 unsupported).**
  The execution facet adds little to recall on either set (+1 / −1 and −2 on held-out),
  so improving it is expensive for a small recall gain now.
- **Parse errors (30 diagnostics in retained Rust files).** Real, but they affect call
  edges, which matter less than the budget and ranking issues above.
- **Raising the byte budget.** Explicitly ruled out in the Stage analysis. It would hide
  the problem, not fix it.

## 4. Hygiene to fix alongside

These do not change recall but they keep the measurements trustworthy.

- `briefcase_task::refactor_packet_surfaces_tests_plans_scripts_configuration_and_install_touchpoints`
  fails on a `not_covered` assertion. This is the only real Rust assertion failure left.
  The other failures are missing `evaluation/sources/` fixtures.
- `tests/briefcase-pi.test.mjs`: "oversized structured output fails closed with a valid
  JSON notice" still fails. The output has no continuation hint.
- Latency: first-packet median rose from about 2.1 s to 3.8 s on original and 5.2 s on
  held-out, and up to 6.3 s at cap 48. Re-measure after #1, since smaller entries
  should also reduce serialization time.

## 5. Plan

1. **Implement #1** (compact manifest), then **#2** (cap 48). Measure with
   `task-evaluate.py --budget-sweep 8000,12000` on the retained DB.
2. **Acceptance for #1 and #2:** at the 8 KB Pi budget, delivered required paths reach
   at least **34/36 original** and **19/19 held-out**, with every omitted selected file
   covered by a valid continuation.
3. **Then #3**, measured by the same sweep. Reject it if it displaces a required `impl`
   file.
4. **Then #4 and #5**, each re-running the full controls with ablations.
5. Keep the held-out set separate. Choosing the cap on the same set we report on
   overfits. Author fresh held-out scenarios before claiming #2 generalizes.

## 6. Caveats

- All numbers come from one run on one repository, against labels written by hand for
  this repo. The held-out set was written by the same author as the original set and
  was used to choose the cap. Treat #2 as a hypothesis until it is confirmed on new
  scenarios.
- Timings are warm and were taken alongside extra reads. Do not compare them to cold
  starts.
- The `delivery_recall` field is a delivered-to-selected ratio, not a recall figure.
  The figures in this document use the authored required paths and
  `omitted_selected_paths`.
