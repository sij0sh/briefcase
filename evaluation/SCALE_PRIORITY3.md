# Scale Priority 3: inspected evidence and supported conclusions

Status: metric infrastructure implemented and exercised on round 4. Conclusion
recording and counterevidence obligations are now part of the evidence schema.
A development obligation pilot is authored but not run. No answer quality or
behavioral sufficiency result has been established. See
[round-4 results](priority2-r4-results.json) for counts, raw artifact paths and
SHA-256 digests, and [serving reliability](SERVING_RELIABILITY.md) for the
unresolved process/data corruption blocker.

## What is measured

`scripts/task-evidence-evaluate.py` reads scorer-only oracles and the immutable
events retained by `task-evaluate.py --investigation-dir`. Labels do not enter
retrieval or the investigation queue. The scorer verifies event hashes, source
identities, view/index pins and historical flags before scoring.

| Measure | Required evidence |
| --- | --- |
| Path inspection | Some pinned bytes from the authored path were inspected; this alone does not establish a passage hit |
| Passage coverage | Registered pattern in contiguous inspected bytes, expected source digest, and coverage of the authored byte range when supplied |
| Relationship coverage | Delivered relationship record with the registered predicate and endpoint paths; optional basis, class and authority must match |
| Counterevidence coverage | Authored limiting passage was inspected; it is a required part of the claim group, not an automatic refutation |
| Claim supportability | Every passage/relationship and counterevidence obligation in a registered all-of group, or one explicit alternative group, is available |
| Supported conclusions | Explicit submitted text matches a registered claim and cites inspected ranges/digests and delivered relationship IDs satisfying a complete group, including its counterevidence |

An exact-key association has its own `EXACT_KEY_ASSOCIATION` identity and static
authority. It cannot satisfy `CALLS_STATIC` or an execution obligation. Shared
paths do not imply a relationship. Static import/test associations do not prove
runtime execution or test coverage. The conclusion scorer checks authored
evidence obligations; it is not an unrestricted natural-language truth judge.

Each claim has a primary all-of evidence group. It may list alternate witness
groups under `alternatives`; one complete group is sufficient. A group may list
`counterevidence` passage IDs. A claim is supportable only when its positive
passages, relationships and counterevidence are inspected. A submitted
conclusion must also cite the counterevidence. Counterevidence marks a limit or
exception that the answer must acknowledge; it does not by itself refute the
claim. A claim may carry a `kind` such as `condition`, `consumer` or
`counterevidence` for reporting.

Reports include micro coverage (total hits / total authored obligations), macro
coverage (mean per-task coverage), and submitted/supported/unsupported conclusion
counts with precision and supported-claim coverage. They also report
counterevidence coverage, claim support by kind, actual calls, response bytes and
elapsed seconds, plus coverage-per-effort ratios. A ratio is null when any arm
has an error. No submitted conclusions means `not_submitted` and null
precision/coverage. It is not perfect accuracy.
Each arm must retain every authored task. Failed tasks retain their full
obligation counts; comparable coverage becomes null, with observed lower bounds
reported separately. These metric eligibility flags concern the scored evidence
arm; they do not override upstream run errors or qualify a diagnostic replay for
promotion.

## Recording conclusions

`scripts/task-investigate.py --conclusion conclusions.json` records explicit
conclusions in the investigation state. The file may contain one object, a list,
or `{"conclusions": [...]}`. Each conclusion has this shape:

```json
{"claim_id":"registered-claim-id","text":"Exact registered claim text.",
 "citations":[{"path":"src/example.ts","digest":"sha256:...",
   "start_byte":100,"end_byte":180}],
 "relationship_ids":["task-link:..."]}
```

The workflow rejects a duplicate claim ID in one investigation. Every citation
must name pinned bytes already inspected and the retained digest. The cited
range must fall inside an inspected range or excerpt. Every relationship ID must
appear in the investigation event log. These checks prevent a conclusion from
citing bytes that were never delivered. The scorer also requires a conclusion to
cite the counterevidence in its chosen claim group. The scorer still checks the
registered claim text and oracle obligations; a well-formed submission can
remain unsupported.

When `--conclusions` is omitted, `task-evidence-evaluate.py` reads conclusions
from each investigation checkpoint. An external `--conclusions` file overrides
the checkpoint for every task. This keeps recorded conclusions attached to the
evidence that produced them and still supports an external harness.

## Frozen round-4 pilot

[Tasks](t3-heldout-p2-r4-scenarios.json),
[oracles](t3-heldout-p2-r4-oracles.json) and
[freeze receipt](priority2-r4-freeze.json) were frozen before the exact-key
implementation and round-4 measurement. Ten modules were sampled without
replacement from 249 sorted implementation files with sibling tests, excluding
directories labelled by earlier development and held-out rounds. All first ten
samples were retained. Descriptions have no target anchors. The author knew the
proposed features; import collaborators may have appeared in prior work. This
is a prospective fresh split, not independently blinded authorship.

The pilot has 28 passage witnesses, 10 static import relationships and 10
limited implementation/verification availability obligations. Some markers
identify an imported symbol or test title. These are deliberately narrow
localization checks, not full behavioral explanations. No behavioral answers
were submitted. Future tasks need independently authored obligations covering
conditions, contracts, consumers and counterevidence, plus alternate valid
witness sets and actual conclusion submissions.

## Development obligation pilot

[Tasks](t3-p3-development-scenarios.json),
[oracles](t3-p3-development-oracles.json) and
[freeze receipt](t3-p3-development-freeze.json) add a small development pilot.
It has three tasks, 11 required paths and nine claims: three condition, three
consumer and three counterevidence claims. Fifteen counterevidence obligations
refer to 12 distinct limiting passages. Each claim also has positive passage or
relationship obligations. One claim has an alternate implementation-only witness
group; `alternatives` can carry a different complete group.

The pilot was authored after the round-4 results were known. Modules were chosen
for clear condition, consumer and counterevidence behavior, not sampled. It is
not held-out and cannot support a promotion claim. Source files are pinned by
SHA-256 in the oracle and freeze receipt. The tasks have no target anchors.

A scorer self-check confirmed that every primary group becomes supportable when
all of its passages are inspected and cited, and that omitting a counterevidence
citation makes the conclusion unsupported. No investigation was run against
this pilot, so it has no conclusion submissions, coverage or effort figures.
Collect conclusions at equal effort only after the serving corruption blocker
clears.

## Bounded structural exploration

`structural-explore` uses the existing resumable driver and retained
continuations. It reserves every fifth queued action for a bounded side queue:
filtered unselected graph candidates, depth-one adjacency from at most four
manifest roots, and at most sixteen additional source excerpts. It uses
`page`, `neighbors`, `query` and `explain`; it introduces no new engine operation
or default selection rule. Ordinary actions follow the excerpt-first schedule.

Both arms used 80 calls, 524,288 response bytes and 120 seconds per task,
8,000-byte manifests, compact inventories and inspection-only reading. Byte
accounting is compact UTF-8 JSON results plus newline, excluding the transport
envelope. The driver reserves the next request's response ceiling before
issuing it. Time is checked before each call; an in-flight call can exceed the
time limit and any overshoot is recorded. Source digest verification is a
separate cost; these runs inspected retained bytes without hashing each whole
file on the client.

The initial structural run exposed a scheduler defect: ordinary excerpt phases
did not advance, starving inspection. Its raw results remain archived and are
excluded from policy conclusions. A regression test now checks ordinary phase
progression, protected-action isolation and resume. The corrected replay uses
the exact same native binary and case/view/index identities as the successful
excerpt-first investigations. It reuses that control's events. Tasks were
already observed, so the replay is diagnostic.

| Corrected diagnostic measure | Excerpt-first | Structural exploration |
| --- | ---: | ---: |
| Authored tasks completed without investigation errors | 10/10 | 10/10 |
| Required paths with inspected bytes | 28/28 | 28/28 |
| Required paths inspected beyond initial manifest | 1 | 1 |
| Required passage witnesses | 12/28 | 12/28 |
| Macro passage coverage | 43.33% | 43.33% |
| Required relationship witnesses | 0/10 | 5/10 |
| Complete availability obligations | 0/10 | 0/10 |
| Submitted conclusions | 0 | 0 |
| Median calls | 76.5 | 77 |
| Median response bytes | 519,534 | 518,433.5 |
| Byte/time overshoot | 0/0 | 0/0 |

Protected exploration increased delivered relationships without changing the
registered passage hits. It showed no improvement in complete obligations.
Retain it as an opt-in investigation policy. Separate runs and concurrent
diagnostics prevent a causal latency comparison. The native retrieval run
still had errors, so these successful investigations do not repair its
incomplete denominators.

## Running the metrics

Retain investigation events during an evaluation:

```sh
python3 scripts/task-evaluate.py --binary /path/to/frozen/briefcase \
  --root /path/to/frozen/source --db /path/to/disposable/briefcase.db \
  --scenarios evaluation/t3-heldout-p2-r4-scenarios.json --serve \
  --selection-variants exact_keys --diagnostics --manifest-bytes 8000 \
  --investigation-modes excerpt-first,structural-explore \
  --investigation-max-calls 80 --investigation-max-bytes 524288 \
  --investigation-max-seconds 120 --investigation-compact-inventory \
  --investigation-inspection-only --investigation-dir /path/to/new/events \
  --trace-dir /path/to/new/rpc --output /path/to/results.json
python3 scripts/task-evidence-evaluate.py --results /path/to/results.json \
  --oracles evaluation/t3-heldout-p2-r4-oracles.json \
  --output /path/to/evidence.json
```

Use a new disposable SQLite backup and new event directories. A repeated run
on R4 is regression evidence. It is not a fresh held-out test. The raw primary
and corrected runs, frozen source trees, diagnostic replay driver and final
scoring artifact are named and hashed in `priority2-r4-results.json`.

For the development obligation pilot, use
`evaluation/t3-p3-development-scenarios.json` for the run and
`evaluation/t3-p3-development-oracles.json` for scoring. The pilot is not a
promotion gate. It is a schema and conclusion-collection exercise.

Optional `--conclusions conclusions.json` accepts this shape:

```json
{"scenarios":[{"id":"task-id","mode":"excerpt-first","conclusions":[{
  "claim_id":"registered-claim-id","text":"Exact registered claim text.",
  "citations":[{"path":"src/example.ts","digest":"sha256:...",
    "start_byte":100,"end_byte":180}],
  "relationship_ids":["task-link:..."]
}]}]}
```

A claim may be submitted at most once per task/mode. Citation ranges must be
inspected and contain the authored passage span. Availability alone never
creates a conclusion submission.
