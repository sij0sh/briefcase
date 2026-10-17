# Stage 4: usable sync and verified source delivery

Date: 2026-10-17 UTC. Scope follows the user's Stage 3 rerun: fix perpetual
sync failure from partial static calls, verify the actual Pi budget and omitted
source fetches, and make remaining retrieval misses diagnosable. No separate
Stage 4 specification or per-candidate rerun data was supplied.

## Baseline supplied by the user

The Stage 3 tree (`e2a19d0`) builds and installs without edits. Original recall
is 32/36 and held-out recall 16/19 at a 32-file cap. Harness recall matches brief
recall, with 31–32 files, zero omissions and roughly 6.5–6.6 KB manifests.
Freshness is 199 B. Initial sync still exits one because static calls are partial.
These are user measurements of Stage 3, not measurements of this change.

## Sync readiness and coverage

Two checks previously made partial calls a permanent sync failure:
`project::check_one` marked identical inputs dirty until resolution was complete,
and `sync::run` changed any partial result into `ready:false`. Unsupported
methods, closures, generated source and external targets can keep coverage
partial indefinitely, regardless of retries.

Stage 4 treats a successfully published partial call analysis as usable local
context. It stores a small operational receipt in the existing execution log,
scoped to the source view and exact source/profile/implementation input key.
Read-only checks and unchanged syncs reuse it, allowing scheduler work to be
acknowledged instead of repeatedly performing the same analysis. The key now
includes the analyzer launcher's byte digest, including PATH resolution, so a
provider change with an unchanged version string also invalidates reuse.

| Call phase | Operationally ready | Next step |
| --- | --- | --- |
| Complete, current Rust engine and full producer scope | Yes | Reuse |
| Successfully published partial | Yes | Reuse; `sync --retry-analysis` requests another attempt |
| Not requested | Yes | No call extraction requested |
| Default analyzer unavailable, syntax-only profile | Yes | Coverage gap remains explicit |
| Requested attempt not run | No | Run it |
| Analyzer failure or budget exhausted before usable publication | No | Retry; preserve the failure |
| Invalid explicit analyzer/profile configuration | No | Correct configuration |

Source or profile changes trigger normal reconciliation. An explicit retry can
publish improved additive evidence; a failed retry remains dirty even if an
older partial receipt exists. Receipts are append-only, and the newest attempt
wins. Multiple profile outcomes and other local domain failures cannot be
overwritten by a successful profile. The existing execution table needs no
schema migration. The first Stage 4 sync may refresh inputs because the source
key includes implementation and profile details.

`ready:true` and `--require-ready` describe local source/projection alignment.
Sync/check/status annotate that scope. A partial result retains
`static_calls.state:partial`, its exact coverage/counts/limitations,
`resolution_complete:false`, and `native_resolved_subset`. No run, assertion,
unresolved reference, pass coverage or task completeness is rewritten as complete.
Cached complete runs must belong to the current Rust execution implementation
and `retained_free_function_scope`; an old engine or subset guarantee is
insufficient. `--retry-analysis` applies to partial Rust call extraction and
conflicts with `--no-calls`.

```sh
cargo build --locked --release
./scripts/verify-context
target/release/briefcase --json sync --timeout 600 --require-ready
target/release/briefcase --json sync --check
target/release/briefcase --json sync --timeout 600 --require-ready
target/release/briefcase --json sync --retry-analysis --timeout 600 --require-ready
```

Keep database and profile options identical when comparing the unchanged run.
The second run should reuse a partial attempt; the last command requests a new
attempt without asserting that it will resolve unsupported constructs.

## Delivery verification and remaining misses

`task-evaluate.py --verify-sources` now executes the case-pinned source calls,
first exercising any advertised nonzero offset, then reading full files from
zero to EOF. It verifies every selected file, including unlabelled selected
files and rows omitted from a small manifest. Each page must preserve its case,
reference, path, artifact, digest, byte count and contiguous UTF-8 byte cursor.
Reconstructed bytes must match their retained SHA-256. Advertised excerpts must
agree with the full stream. Empty files are supported; corrupt bytes, stalled
cursors, wrong entries and budget notices are errors. Bodies are not embedded
in evaluation results. Source fetch timing is reported separately from context
delivery timing.

`--budget-sweep 3000,4000,6000,8000,12000` measures total harness bytes including
the exact Pi framing. Every manifest must use the same case and selected paths,
and larger budgets must preserve the anchor and supplemental path prefix.
Reports include delivered/omitted selected paths and delivery recall separately
from labelled retrieval recall. The file cap remains scenario-controlled.

Diagnostics now preserve signal provenance such as lexical rank and graph hub
penalties, selected candidates, and the first five unselected candidates in each
role. This allows the remaining `command.rs`, history-test, `verify-pi`,
`integrations.rs`, `native_host.rs` and `sync/project.rs` misses to be classified
using actual discovery/selection scores and role competitors. Their new candidate
dumps were not provided; no exact cause or restored recall is claimed. Facet
weights, thresholds, role cycles and retrieval traversal are unchanged.

```sh
mkdir -p /tmp/briefcase-stage4
python3 -B scripts/task-evaluate.py --binary target/release/briefcase --root . \
  --db /tmp/briefcase-stage4/baseline.db \
  --scenarios evaluation/task-scenarios.json --harness pi --manifest-bytes 8000 \
  --verify-sources --budget-sweep 3000,4000,6000,8000,12000 --diagnostics \
  --ablations all --output /tmp/briefcase-stage4/original.json
```

Initialize and enrich that database with the same source/profile/Git/session
recipe before measuring. Repeat with the authored held-out scenario file.
The existing `task-controls.py` runs both sets and held-out ablations in the
retained condition, then checks a clean source-only clone. It now enables source
verification and budget sweeps in both full conditions, and keeps the clean sync
call coverage beside operational readiness. Optional `--control-ablations`
still adds clean-condition facet ablations. Source verification and extra budget
queries run only on full scenarios, not every ablation. Warm timings reflect
these extra reads and must not be compared as cold-start measurements.

## Validation

Executed locally: 39 Python script tests, 8 Python tool/schema tests and 16 pure
adapter tests: **63 passed**. Protocol fixtures test the Python control driver
with real local Git cloning; they do not emulate or validate Rust retrieval.
Six new Python regressions cover source transport, corruption, nonzero hints,
budget ordering and candidate provenance.

Authored and explicitly wired: three new Rust unit tests and four `stage4`
integration tests. Native protocol fixtures cover partial readiness without
semantic promotion, unchanged reuse, explicit retry, failed-retry recovery,
source/provider invalidation and multiple profiles. The text fixture checks Pi
budget monotonicity and fetches all selected UTF-8 sources. `verify-context`
runs library/context and numbered Stage 2–4 targets; its Python wiring guard
passes.

Source review also corrected the Stage 3 syntax-sync test's CLI setup: its
temporary directory has no Git root, so it must pass `--project-root` explicitly.
The Stage 4 protocol fixtures use the same explicit-root convention. This fixes
fixture setup without changing repository discovery or production assertions.

Rust compilation and these regressions are **unverified here**: Cargo, rustc,
rustfmt and Clippy are absent. The attempted Rust gate exits 127. An actual
control invocation records `blocked` because the release executable is absent.
The package checker still fails on missing original
`evaluation/fixtures/index.json` and `evaluation/sources` inputs. Existing
unclassified refactor-touchpoint failures need their runtime assertion output;
their expectations are unchanged. See [VALIDATION.json](../VALIDATION.json).

Run the Rust gate, release build, original/held-out evaluations and clean control
on the evaluation host before claiming runtime readiness, recall, call-resolution,
latency or memory gains. No new corpus measurements are reported here.
