# Serving and retained-JSON corruption investigation

Status on 2026-10-20: unresolved. Evaluation accounting and failure capture
have been repaired. A hardware memory fault is now confirmed and reproduced
after a CMOS reset at stock BIOS defaults; the failing component is not
isolated. Neither a toolchain pin nor an allocator switch is
supported as a fix. No promotion comparison should use incomplete arms. See
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
on different tasks. The isolated JSON failure is now explained by the hardware
memory fault; the exact-key malformed JSON error has not been separately
reproduced. Both retrieval arms scored 25 of 28 labels, so their equal observed
counts do not establish equivalence or an exact-key gain.

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

The three cores share an impossible chunk size: bit 43 set at page byte offset
`0xa0d`. A hardware memory test now reproduces that signature.

### Hardware memory fault confirmed

On 2026-10-20, `stressapptest -s 60 -M 30000 -m 12 -i 4 -C 4 -W` found four
hardware miscompares in 12 seconds. The test allocates 30 GiB and checks
walking-bit patterns with multiple reader and writer threads.

| Physical word | Failed bit | Page offset | Read | Expected |
| --- | --- | --- | --- | --- |
| `0x9b2853a08` | 43 | `0xa0d` | `0x55555d5555555555` | `0x5555555555555555` |
| `0xeba0a4158` | 31 | `0x15b` | `0xffffffff80000000` | `0xffffffff00000000` |

The first physical page is the exact JSON signature: bit 43 in the 8-byte word
at page offset `0xa08`, with the bad byte at `0xa0d`. The three JSON cores had a
glibc chunk size of `0x0000080000000061` or `0x0000080000000021` where `0x61` or
`0x21` was expected, and one free path held `0x80000000020` where `0x20` was
expected. `stressapptest` produced the same bit at the same offset on physical
page `0x9b2853a08`.

Other failures used page offsets `0x54a` and `0x158` with bits 16 and 31. At
least two physical pages and several bits are affected. The failing virtual
addresses were reproduced on CPU 0, 2, 4, 6, 8, 16, 18 and 22, so this is not one
core's arithmetic error.

The baseline was conservative: CPU boost was disabled and actual single-core
and all-core frequency was 4.2 GHz. Memory runs at 2133 MT/s
JEDEC, not the 3600 rating of the four G.Skill modules. ECC is absent. Tctl was
about 41 C at the failure and package power was about 63 W. No MCE or EDAC
record exists because the memory is non-ECC. A residual BIOS PBO, Curve
Optimizer or SoC undervolt setting was not visible from the OS before the
reset. Stock defaults were then loaded; the fault persisted.

The reboot also restored stock CPU defaults: boost is enabled and active with a
4.95 GHz limit. The memory remains at 2133 MT/s and 1.2 V. This is not an
overclock.

This confirms a hardware memory fault. It does not yet identify a DIMM, slot,
CPU IMC or board trace.

### Post-CMOS retest

A 10-minute `stressapptest -s 600 -M 4096 -W` run passed with 0 incidents. Four
GiB is only a small fraction of the 62 GiB installed, so this pass does not
cover enough memory to rule out the fault and is not evidence of repair.

A 30 GiB run with the original thread and pattern options failed after 31
seconds. It found two miscompares on the same physical word `0x9b2853a08`, at
page offset `0xa0d`, bit 43:

| Read | Expected |
| --- | --- |
| `0x0001080000010000` | `0x0001000000010000` |
| `0x4000080040000000` | `0x4000000040000000` |

A repeat run resolved the failing virtual address through `/proc/<pid>/pagemap`
to the same physical word `0x9b2853a08`. The bad byte was `0x9b2853a0d`, again
matching the JSON chunk-size corruption. The fault therefore persists at stock
BIOS defaults.

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

Hardware-fault logs are under `.agents/artifacts/scale-next/host-diag/`:
`stressapptest.log`, `stressapptest-2.log`, `stressapptest-root.log`,
`physical-addresses.txt`, `capture-phys.py`, `pagemap-capture.sh` and
`summary.txt`. The post-CMOS run is under `host-diag-post-cmos/`:
`stressapptest-post-cmos.log`. The full write-up is
`.agents/artifacts/NATIVE_CORRUPTION_HARDWARE_ANALYSIS.md`.

CMOS defaults did not repair the fault. Next work is component isolation. Power
off and test one DIMM at a time in the same slot with MemTest86+ for at least
four passes. Replace any failing module. Reseat the CPU, DIMMs and cooler. Do
not flash the BIOS while the memory fault is present; a bit flip during a flash
can brick the board. Update the BIOS after the memory is stable. If every module
passes alone, test slots and combinations, then suspect the CPU IMC or the
board.

Do not use this host for new evaluation until the fault is repaired and a clean
`stressapptest` run plus a full MemTest86+ pass are recorded. Keep all R4 tasks
as regression cases, require a full error-free run before any relevance
comparison, and freeze independently authored new tasks before a promotion test.
Do not hide the failures with retries, remove failed labels, or assume successful
short reruns are a repair. Do not run a repair scrub on the corrupted Btrfs
filesystem until the hardware is fixed.
