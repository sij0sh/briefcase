# Priority 0: establish the loss boundary

This evaluation keeps the measured R1/R2/R4 engine baseline: import
corroboration, 48 selected files, and the corrected roles for code under
`hooks/` and `bin/`. Ranking, capture, producer limits, index limits, and
wire schemas are unchanged by this implementation.

## Measured results, 2026-10-20

The frozen `rec-1019` binary (`sha256:4579ecd919e1a9b51bac9f6228ce5cbb3a282d52f3a8c85454777caeac12dfd0`)
and both retained databases were available locally. All 84 investigations
(14 scenarios × three policies × two arms) finished without protocol errors.
Every investigation matched its baseline case, source view, and index identity.
The [machine-readable results](loss-boundary-results.json) retain per-path
attribution, producer/index counters, discovery/frontier counts, per-scenario
reading results, and passage scores. Raw responses and resumable checkpoints
remain in `.agents/artifacts/loss-boundary-{joined,text}-run/`.

All 56 authored required paths were captured in both arms. The latest funnel is:

| First observed exclusion | Joined | Text-only |
| --- | ---: | ---: |
| Never became a candidate | 1 | 2 |
| Below minimum score 24 | 5 | 5 |
| Excluded by selection cap/policy | 6 | 7 |
| Selected but omitted from the framed manifest | 2 | 1 |
| Delivered by the framed manifest | 42 | 41 |

The joined index **did saturate**: 159,582 index relationships before
limiting, 100,000 retained, and 59,582 removed (43,460 `DECLARES`, 15,922
`CONTAINS`, 200 `IN_CYCLE`). The text-only index retained 27,416 and pruned none.
The TypeScript workspace reported `bounded: false`, no import bound, and 67
source files omitted as `outside_partitions`; its file, definition, and import
caps did not saturate. Calls remain unresolved in workspace mode. These facts
establish scope boundaries, not that pruning caused a particular path miss.

Reading order recovered additional paths under the same limits, but isolated
excerpts provided substantially fewer claim witnesses:

| Arm / policy | Required paths inspected / 56 | Beyond manifest | Claims with all witnesses / 44 | Primary passage witnesses / 79 |
| --- | ---: | ---: | ---: | ---: |
| Joined / manifest-first | 37 | 0 | 28 | 51 |
| Joined / inventory-first | 25 | 0 | 4 | 14 |
| Joined / excerpt-first | 48 | 6 | 8 | 27 |
| Text-only / manifest-first | 39 | 0 | 27 | 50 |
| Text-only / inventory-first | 48 | 7 | 7 | 27 |
| Text-only / excerpt-first | 48 | 7 | 7 | 27 |

These counts also hold for the strict byte prefix, excluding the last response
when it crossed 2 MiB. Whole-file digest verification is counted independently:
manifest-first verified 37/39 required files, while excerpt-first verified 1/2.
The comparison uses the historical investigation's 8,000-byte **unframed**
manifest response setting; the first-exclusion table uses the original
evaluation's framed Pi manifest. Each investigation measures recovery against
its own actual manifest, avoiding an assumed shared file cutoff.

The six joined tail recoveries include finalization, PR compatibility, an ACP
adapter, webhook routing/forwarding, and attachment claims. Text-only also
reaches the permission contract and shared keybindings. Nevertheless, the
experiment does **not** justify adopting excerpt-first as the default: reading
one localized chunk is frequently insufficient to cover a condition, exception,
or test witness. Enumerating all verbose candidates first can itself starve
source inspection, especially with joined signals.

Priority 0 therefore establishes two concrete boundaries: actual relationship
pruning before task retrieval, and passage starvation within the available
evidence. Preserve the baseline default. Next work should make retained/overflow
evidence cheaply queryable and support task-relevant passage escalation; larger
file or byte caps alone are not an evidenced solution. No answer-quality or
required-relationship recall improvement was measured.

Validation: all 100 Python tests pass, including scheduling, UTF-8/digest
resumption, pinned excerpt identity, loss attribution, and disjoint passage
scoring. Historical package conformance remains blocked by the pre-existing
missing `evaluation/fixtures/index.json` archive.

## Reproduce the comparison

`scripts/task-loss-boundary.py` takes an existing `task-evaluate.py` report,
its exact scenario bytes, the frozen executable, and its retained database.
It checks executable/scenario digests and case/view/index identities. It reads
loss diagnostics offline, then runs blind investigations on disposable database
copies. Scorer labels never reach the reading policy.

```sh
python3 scripts/task-loss-boundary.py \
  --baseline /path/to/frozen/results/t3-joined.json \
  --scenarios /path/to/frozen/t3code-scenarios.json \
  --db /path/to/frozen/db-sem/briefcase.db \
  --binary /path/to/frozen/briefcase \
  --root /path/to/original/evaluation/worktree \
  --work .agents/artifacts/loss-boundary-joined \
  --workers 4 --output .agents/artifacts/loss-boundary-joined.json
```

Repeat with the text-only database and its baseline report. `--work` must be a
new directory; checkpoints, immutable evidence events, and raw RPC records stay
there. Temporary database copies use that filesystem to permit reflinks and
are removed when the run finishes. The original database must remain frozen.

The comparison inherits the baseline's aggregate call/byte/time limits and
manifest byte setting. The T3 run uses 300 calls, 2 MiB, 180 seconds, and an
8,000-byte manifest. It preserves the previous driver's response-byte accounting
and checks limits before the next RPC. A final response can cross the byte
threshold; each result records `byte_limit_overshoot`.

The three policies use the same protocol:

| Policy | Reading order |
| --- | --- |
| `manifest-first` | Existing baseline: complete manifest files before inventory reads |
| `inventory-first` | Enumerate candidate/gap/neighbor inventories, then localized excerpts, then full-file verification |
| `excerpt-first` | Alternate one inventory action with up to four localized excerpts; defer full-file verification until these queues drain |

The interleaving ratio is an explicit experiment, not a tuned optimum.
`exhaustive` remains available with its previous ordering. The default remains
`manifest-first` so existing callers can reproduce their baseline. All modes
are also available through `task-investigate.py --mode` and
`task-evaluate.py --investigation-modes`.

New policies honor manifest fetch offsets and candidate source spans. A
localized read records its pinned identity and inspected range in `excerpts`;
it does not assert a client-verified whole-file digest. Deferred verification
starts at byte zero and keeps the existing contiguous UTF-8 and SHA-256 checks.
Scheduling progress survives CLI resumption.

## Inspect the loss report

The offline report records each required path's capture state, lexical rank
and admission, candidate score/rank, selection, and manifest delivery. It also
records lexical overflow and term omissions, initial join discovery, graph
starter/frontier counts, and actual index and producer saturation counters.
Missing producer counters remain unknown. A global omission counter does not
identify a particular missing task relationship.

Inspection results distinguish candidate inventory delivery, source inspection,
whole-file digest verification, and remaining queued work. They do not establish
that an investigator understood the source or produced a correct answer.

Score actual passage witnesses after the blind runs:

```sh
python3 scripts/task_loss_witnesses.py \
  --results .agents/artifacts/loss-boundary-joined.json \
            .agents/artifacts/loss-boundary-text.json \
  --claims evaluation/t3-outcome-claims.json \
  --output .agents/artifacts/loss-boundary-witnesses.json
```

This scorer verifies retained event hashes and matches patterns only within
contiguous inspected ranges. It reports micro and macro claim supportability
separately. `strict_byte_prefix` excludes the final response if it crossed the
byte limit, so recovery does not receive credit for excess bytes. Witness
availability remains distinct from correct conclusions or executed tests.

## Lexical overflow route

Explain the case's `/coverage/retrieval/lexical_discovery`, then follow its
`continue_with`: `page` against the separate lexical report's `/items`, starting
at offset 1000 for overflow. The case's `/candidates` is the fused inventory,
not the lexical tail. Reading that tail does not promote it into ranking or
recompute the assembled case. See [P2_IMPLEMENTATION.md](../P2_IMPLEMENTATION.md).
