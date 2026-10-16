# Stage 3: regression repair and controlled evaluation

Date: 2026-10-16.
Scope: the four recommendations in the user's Stage 2 evaluation. No additional
Stage 3 specification was available. Source changes and the control workflow are
implemented; Rust compilation, runtime regressions and new corpus results remain
unverified on this host.

## User-reported Stage 2 baseline

| Measure | Stage 2 result |
| --- | --- |
| Release build | Succeeds with no source edits |
| Library tests | 126 pass, zero fail |
| Original brief / harness recall | 31/36 / 31/36 |
| Held-out brief / harness recall | 16/19 / 16/19 |
| False-complete | Zero |
| Rust native sync | 183 seconds; partial static calls; `ready:false` |
| Native calls | About 3,109 of 25,073 attempted |

These are the user's measurements of Stage 2, not Stage 3 results. The held-out
set is five previously labelled scenarios. Original-set ablations are directional
evidence; no weights were tuned to those labels or to the held-out misses.

## 1. Explicit test discovery

The reviewed Stage 2 source already included `briefcase_stage2.rs` through the
registered `context` target. Its 20 new unit tests are inline library tests, so
they are covered by `cargo test --lib`; the reported 126 is consistent with the
previous 106 plus those 20. The supplied test-list/individual assertions were not
available to prove which commands ran on the user's host.

Stage 3 removes ambiguity: Cargo explicitly registers `stage2` and `stage3`.
The Stage 2 module is removed from `context` to avoid running it twice.
`scripts/verify-context` runs library, context and both numbered targets, and
`verify-mvp` includes that gate. A Python guard checks the target/gate wiring.

```sh
cargo test --locked --test stage2 -- --list
cargo test --locked --test stage3 -- --list
./scripts/verify-context
```

## 2. Integration failure classification

| Failure | Source diagnosis | Change / remaining check |
| --- | --- | --- |
| `hooks_use_event_cwd...` and `alignment_checks...` | Confirmed shared defect: Stage 2 builds source recall with a policy-aware cache key, while alignment looked for the old key. An unchanged source therefore appeared dirty. These tests use self-contained repositories, not the missing source fixtures. | Centralized the policy key in `search::index_configuration`; builder and checker use it. Added an isolated syntax-sync regression that switches search policies, checks read-only alignment and confirms the watcher enqueues no revision for unchanged inputs. Actual failing assertion locations still require a Rust run. |
| `typescript_sync_requires_explicit...` | A known pre-Stage-2 failure, not one of the missing-fixture tests. Confirmed one failing path by source inspection: unreadable/malformed `.briefcase/sync.json` can fail in `configured_profiles` before ordinary configuration handling, yielding `failed` instead of `needs_configuration`. | Both profile apply/check classify those errors consistently. Added a preflight regression for a directory and malformed JSON; neither may create a ledger. Other assertions in the long TypeScript test remain unverified. |
| `every_duty_and_target...` | Known 8 KB ordinary-brief metadata floor, reported before Stage 2. The brief could be budgeted before freshness, then fail again after it was attached; generic required-metadata handling returned an error. | Gateway renders ordinary briefs without preliminary byte reduction. Oversized briefs summarize advisory freshness first; a metadata-only floor returns case pins and an offset-zero restart. Full retained case completeness is unchanged. |
| `refactor_packet_surfaces...` | Known earlier failure; this fixture is self-contained. No assertion output was supplied, and source review does not establish a specific faulty touchpoint. | Kept the assertions and expected roles/touchpoints. Failure output now includes the full packet; this remains unclassified pending the targeted runtime result. |
| Missing `evaluation/sources` / `evaluation/fixtures` | Input fixture absence in the supplied archive. | No generated replacements, skipped assertions or fabricated conformance passes. Restore the original fixtures for those lanes. |

Focused commands preserve the first useful assertion:

```sh
cargo test --locked --test core hooks_use_event_cwd -- --nocapture
cargo test --locked --test core alignment_checks -- --nocapture
cargo test --locked --test core typescript_sync_requires_explicit -- --nocapture
cargo test --locked --test context refactor_packet_surfaces -- --nocapture
cargo test --locked --test context every_duty_and_target -- --nocapture
```

## 3. Retrieval regressions and ablations

The supplied summary says H3 loses `src/bin/briefcase/command.rs`, H4 loses
`src/architecture/native_host.rs`, and the duty variant task still misses
`trace.rs`/`command.rs`. The new per-case candidate dumps were not supplied, and
this host cannot execute Briefcase, so their exact candidate ranks, scores and
loss stage have not been measured. No restored-file claim is made.

Source inspection did identify a general crowding mechanism. At depth two,
`implementation A <- TESTS - shared suite - TESTS -> implementation B` could
introduce B and many other implementations solely through the shared suite.
`TESTS` is now direct-only. An explicitly targeted test still reaches the
implementations it directly tests, and Cargo-derived runner/module links remain
available. Explicit dependency edges keep their configured depth.

Disabled facets also no longer contribute hub-degree penalties to enabled
facets. The previous ablations removed traversal signals but still counted those
edges in degree normalization. Facet weights, minimum score and duty reading
cycles are unchanged. Synthetic source/graph regressions cover both behaviors.

New selected and unselected candidates include optional `role`. With
`--diagnostics`, the evaluator pages the exact retained candidate inventory and
reports each labelled path as not a candidate, below threshold, not selected,
missing from brief delivery, missing from harness delivery, or selected. It keeps
evidence rank, delivery rank, score, role and signal/link references. Ablation
reports include required files gained/lost, all selected-file changes and role
count changes. This is the evidence needed to distinguish selection crowding
from a discovery miss on H3/H4.

A missing labelled file that the brief never retrieved is no longer treated as a
manifest continuation error. Actual brief-to-harness delivery omissions still
require a continuation. These are distinct measurements.

## 4. Frozen-input controls

The evaluator accepts explicit `--db` and `--view`. It pins the source view
before querying and uses that same explicit view for both brief and manifest, so
their request identities match. It rejects source/corpus mutation using status
watermarks, retractions, current-view checks and scenario hashes. Timing now
starts after the status/pin preflight, so cold-read figures are not directly
comparable with older evaluator output.

`scripts/task-controls.py` performs the following on a Rust-enabled host:

1. Read the baseline source/index pins and complete retained source inventory.
2. Require live source files to match every retained path/digest; reject changed
   bytes, symlinks and unsafe paths.
3. Construct a fresh local Git seed from exactly those retained files and clone
   it with one synthetic snapshot commit, without inheriting project history.
   Captured files remain present even when `.gitignore` matches them.
4. Sync only the project into a new database. Do not import Git/session domains.
5. Require identical captured path/digest/size manifests and an identical single
   Rust analysis profile. Summary status exposes `source_sync` for that check.
   Mismatched profiles are a blocker, not a measured comparison.
6. Run original and held-out sets with all ten ablations in the retained
   condition. Run both full sets in the clean condition; `--control-ablations`
   also runs all ten clean-condition ablations when desired.
7. Save per-condition JSON, stderr, clean sync diagnostics, source/binary hashes,
   source pins, profile and per-scenario comparisons. Refuse to overwrite a
   nonempty results directory and require results outside the source tree.

This is an identical-source, fresh-evidence control, not a claim to reproduce an
upstream remote clone. Retained evaluation documents stay identical in both
captures and remain excluded from default recall by Stage 2's policy. Native
results may still differ between runs; their coverage and `ready:false` remain
visible. A usable source publication with partial calls is permitted, while
missing source/profile data or evaluation errors block comparison.

Example using a new output directory and the same enrichment recipe as the
baseline under comparison:

```sh
cargo build --locked --release
target/release/briefcase --db /tmp/briefcase-stage3/baseline.db --json \
  sync --project --language rust --timeout 600
target/release/briefcase --db /tmp/briefcase-stage3/baseline.db --json sync --git
target/release/briefcase --db /tmp/briefcase-stage3/baseline.db --json sync --sessions all
python3 -B scripts/task-controls.py --binary target/release/briefcase --root . \
  --db /tmp/briefcase-stage3/baseline.db --output /tmp/briefcase-stage3/control-001
```

A source sync can exit one with partial static calls. Inspect its published view
and diagnostics before continuing. The control runner checks usability and never
changes that state to ready/complete. For explicit analyzer or other profile
options, repeat them as `--sync-arg=--analyzer --sync-arg=/trusted/analyzer`.

## Validation and limits

Executed: Python evaluator/control/schema tests and pure adapter tests, as
recorded in `../VALIDATION.json`. The control-driver integration uses a protocol
fixture plus real local Git cloning. It validates orchestration, matching inputs,
held-out ablation execution, error handling and honest partial-state reporting;
it does not execute Rust retrieval.

Authored: four new Rust unit regressions and four new Stage 3 integration
regressions. The three Stage 2 integration regressions now have their own explicit
target. No Rust regressions were executed here: Cargo, rustc, rustfmt and Clippy
are unavailable. An actual control invocation recorded `blocked` because the
release executable is absent. The original conformance fixtures are still absent.

No Stage 3 recall, resolution rate, parser-error count, latency or memory gains
are claimed. `refactor_packet...` remains unclassified without runtime output.
The grammar dependency and native partial-coverage rules are unchanged; Stage 3
does not claim to make sync universally complete.
