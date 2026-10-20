# Serving and retained-JSON corruption investigation

Status on 2026-10-20: unresolved. Evaluation accounting and failure capture
have been repaired; the native process/data corruption has not. Neither a
toolchain pin nor an allocator switch is supported as a fix. No promotion
comparison should use incomplete arms. See
[round-4 results](priority2-r4-results.json) for frozen experiment hashes.

## Evaluator repairs

The evaluator now serializes request/reply validation, continuously drains
stderr, retains bounded stderr diagnostics and exit codes for EOF failures,
and detects nonzero process death between requests and during shutdown. It
does not silently restart a dead process and credit the next task as a clean
comparison. No automatic retry replaces a failed observation.

Worker databases and frozen reading-policy comparisons use SQLite's backup
API, including committed WAL state in one consistent snapshot. Source DBs
remain read-only during backup; queries retain new reports in disposable
copies. Separately copying `.db` and `-wal` was not a consistent snapshot.

Error rows retain every authored required-path label and explicitly mark it
unscored. `arm_coverage` reports authored, scored and error denominators, plus
observed selection/delivery counts. An incomplete arm has null recall and
`comparison_eligible:false`; observed successes over the full authored count
are separately labelled lower bounds. Serial timing errors preserve previously
captured selections and also fail comparison eligibility. Paired deltas are
suppressed if either arm has errors. Input changes and shutdown failures fail
the report-wide comparison gate. Preflight failures stop evaluation before a
comparison starts. Exit status remains nonzero on errors or missed tasks.

The between-request/shutdown checks and delta suppression were added after the
frozen R4 run. They are regression-tested, not retroactively substituted into
that run. Its two retrieval errors and one timing error remain recorded.

## Evidence for the remaining blocker

R4 reproduced `free(): invalid next size (normal)` in baseline serving and
`double free or corruption (out)` in a separate process-per-call latency pass.
The exact-key arm also had a malformed retained-JSON error. These errors occurred
on different tasks; the shared root cause of every error is not established.
Both retrieval arms scored 25 of 28 labels, so their equal observed counts do
not establish equivalence or an exact-key gain.

The investigation reduced a corruption failure to a separate executable that
only parses a 227,229,487-byte retained JSON bundle, deeply clones
`serde_json::Value` three times, compares the clones and drops them. The isolated
project uses the same locked serde/serde_json dependencies and runs no Briefcase,
SQLite, producer, graph or retrieval code.

| Diagnostic | Observation |
| --- | --- |
| Rust 1.98, system allocator | Aborted after seven completed iterations; another 40-iteration run passed |
| Rust 1.97, system allocator | Initial 40 iterations passed; repeat aborted after nine completed iterations |
| Rust 1.98, jemalloc preload in isolated process | Equality failed after 34 completed iterations; first differing digest character was ASCII `6` versus `7` (XOR 1) |
| Bounded first-difference version, jemalloc | 80 iterations passed; intermittent failure remains |
| SQLite quick check and FTS integrity | Passed on the disposable reproduction database |
| C memory checks | 1 GiB/32 passes and 8 GiB/6 passes passed |
| C allocation/copy/free stress | 120 million allocations passed |
| C targeted page-offset check | 8 GiB, 16 passes at offset `0xa08`, passed |
| Plain Rust BTreeMap clone/drop stress | 100,000 entries, 64 rounds passed |
| ASAN JSON stress, application/dependency instrumentation | 40 iterations passed without a report |

Three independent native cores have an implausible malloc size with bit 43
set at the same virtual page byte offset `0xa0d`:

| Core PID | Failing context | Chunk size address |
| --- | --- | --- |
| 1523544 | Round-1 serde_json drop | `0x55d3632d1a08` |
| 1616968 | Round-2 retained Link drop | `0x55d1de444a08` |
| 1810400 | R4 manifest process allocator free | `0x561548c16a08` |

The repeated bit/page pattern and JSON-only reproduction make host instability
a serious hypothesis. They do not prove faulty RAM, CPU settings, or a specific
software defect. Online memory checks passed; no actual MCE/EDAC failure was
found. XMP/EXPO, PBO/Curve Optimizer, overclock and undervolt settings remain
unknown. No firmware or system tuning settings were changed.

Native ASAN replay completed 13 of 16 requests before being deliberately
stopped for diagnostic cost, with no sanitizer finding. One optimized targeted
ASAN request passed with SQLite's duplicate-global ODR check disabled; its
initial ODR startup diagnostic was not evidence for the observed corruption.
A separate fully instrumented standard-library ASAN log records 37 completed
iterations without a report; a final successful exit was not captured, so it
is not counted as a completed run. Successful sanitizer runs do not resolve an
intermittent failure.

## Reproduction and retained evidence

The checked-in bounded diagnostic is
[`examples/retained-json-stress.rs`](../examples/retained-json-stress.rs):

```sh
cargo run --locked --release --example retained-json-stress -- /path/to/view.json 40
```

It emits only the first differing path/byte and pointer/page offsets instead
of printing the full bundle on assertion failure. It exits before dropping a
graph known to differ. An isolated minimal Cargo project and its lockfile are
retained under `.agents/artifacts/scale-next/json-stress/`. Use that project to
exclude Briefcase's linked native libraries entirely.

Local diagnostic artifacts are under `.agents/artifacts/scale-next/`:
`retained-view.json`, `json-stress.log`, `json-stress.err`,
`json-stress197-repeat.err`, `json-jemalloc.err`, `json-difference.*`,
`json-asan.*`, `json-full-asan.*`, `sqlite-integrity.log`,
`round1-corrupt-allocation.txt`, `corrupt-allocation.txt`, and
`current-free-assembly.txt`. The old equality assertion produced a very large
stderr file; use the bounded diagnostic for subsequent reproduction.

Next discriminating work is to run the isolated project and identical bundle
on a second host, and inspect the current host's memory/CPU configuration and
offline memory diagnostics. Reproduce at stock settings if tuning is enabled.
That requires access to another machine or a maintenance window; it has not
been performed here. Keep all R4 tasks as regression cases, require a full
error-free run before any relevance comparison, and freeze independently
authored new tasks before a promotion test. Do not hide the failures with
retries, remove failed labels, or assume successful short reruns are a repair.
