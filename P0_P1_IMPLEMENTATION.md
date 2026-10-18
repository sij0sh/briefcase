# P0/P1 implementation

This implementation is based on the 212-file source export supplied on 2026-10-17. It is an editable reconstruction, with a patch against that export. There was no linked live checkout. The historical benchmark archive and database were not supplied.

## Changes

### P0: reproducible measurements and regression gates

- `scripts/validate-p0-p1.py` records source manifests, toolchain versions, raw subprocess outputs, commands, timings, and explicit passed/failed/blocked checks. A successful stable-source build produces a receipt binding source and executable digests.
- `scripts/task-evaluate.py` accepts `--trace-dir`, `--build-receipt`, `--implementation-root`, `--source-budget`, and `--verify-candidates`. Source verification uses the requested actual response budget. Candidate reachability is measured separately from selection, manifest delivery, harness inspection, and patch correctness.
- Control runs retain raw RPC evidence. Existing historical metrics keep their meaning.
- Oversized Pi responses now provide executable restart actions for legacy continuation tokens and source reads. Source reads rewind to the first undelivered byte.
- The missing `scripts/verify-context` gate was added. `tools/check_package.py` reports missing historical fixtures as blocked; `--schemas-only` checks the available contracts without implying fixture conformance.
- New native and real-binary acceptance fixtures are self-contained and explicitly distinct from the missing historical corpus.

### P1: source access, reference workflow, and current-source recheck

- Every new candidate has `source_ref`. `explain` resolves `/candidates/N` through the case's pinned index, including candidates outside the selected slice.
- Source responses expose digest/byte identity, UTF-8-safe continuation, configurable `source_bytes`, source/index pins, and retraction state. Byte-budget reduction updates source cursors rather than skipping undelivered bytes.
- New `worktree` reads compare all capturable current source bytes against a pinned view/case. Added, modified, deleted, and unavailable paths remain in a paged retained report. Counts include changes outside the original slice. No source is published by this operation.
- `scripts/task-investigate.py` provides the reference CLI workflow. It pages candidates and gaps, fetches full source, verifies digests, follows explicit adjacency, accepts additional pinned queries, and saves resumable progress plus immutable revisions.
- Requirements derive applicable roles from the existing case policy. The workflow distinguishes fetched bytes, delivered evidence, harness assessments, context readiness, and pending test verification. Evidence collection alone never marks a task ready.
- Freshness checks run before and after retrieval. Changed bytes, changed source pins, pending capture, and retractions invalidate progress. `--rebind` starts new pins after an explicit source refresh and retains the prior revision.
- Pi and Claude post-task reports include a retained worktree comparison against the initial case. Hooks remain advisory and do not start another agent turn.

## Use the reference CLI

Build Briefcase, then start a task from this project:

```sh
cargo build --locked --release
python3 -B scripts/task-investigate.py \
  --binary target/release/briefcase --root /path/to/repository \
  --task 'Rename the storage entry and identify affected consumers' \
  --target src/store.rs --duty refactor \
  --state-dir /path/to/repository/.briefcase/investigations/storage-rename \
  --steps 20
```

Use the same `--root`, `--binary`, `--db` when specified, and `--state-dir` to resume. Task arguments can be omitted on subsequent invocations. `--steps` limits one invocation; pending work survives. `--initial-files 1` can demonstrate recovery beyond a one-file initial selection; it does not limit the follow-up source inventory.

The output includes a state-file location, readiness, pending action count, and either the latest response or a reference to it. Earlier responses can be delivered with `--read-event <event-id>`. Full event and raw RPC logs live under the investigation directory. The record distinguishes responses collected locally from responses delivered through the CLI output.

Additional task-directed queries are explicit JSON:

```json
{"operation":"occurrences","arguments":{"text":"old_entry_name"}}
```

Pass that file with `--query queries.json`. A list of requests is also accepted. Only pinned `occurrences`, `neighbors`, and `search` operations are accepted; this route does not execute arbitrary commands. Neighbor/occurrence results can introduce source outside the initial candidate list.

After inspecting evidence, the harness can submit an assessment:

```json
{
  "requirements": {
    "implementation": {
      "status": "supported",
      "reason": "The inspected definition and error path establish the intended behavior",
      "evidence": ["sha256:<delivered-event-id>"]
    }
  }
}
```

Pass it with `--assessment assessment.json --steps 0`. Valid statuses are `supported`, `open`, `blocked`, and `not_applicable`. Every assessment requires a reason; supported claims require evidence already delivered by this investigation. The source-scope requirement is controlled by the current-byte check. Assessments are labelled as harness judgments and do not turn static evidence into test execution.

After edits, run the same command to recheck. If source differs, inspect the returned worktree report and refresh source using the project's normal sync configuration. For text-only capture, the existing explicit command is:

```sh
briefcase --db /path/to/evidence.db --project-root /path/to/repository \
  --json capture-text /path/to/repository
```

Then use `--rebind`. Previous source ranges and judgments are not carried into the new pins. `capture-text` does not run language analysis; use normal project sync when semantic profiles are needed.

The native recheck is independently usable:

```sh
briefcase --project-root /path/to/repository --json call worktree \
  --args '{"case_id":"task-case:<id>","limit":20,"max_bytes":8000}'
```

Follow `continue_with` or `page` to inspect all changed paths. The comparison covers capturable source text; exclusions, race diagnostics, inaccessible inputs, and external repositories remain explicit boundaries. It does not infer the cause of changes.

## Validate and reproduce

```sh
python3 -B scripts/validate-p0-p1.py --output /path/outside/project/p0-p1-validation
```

When Cargo is available, this builds release/debug executables, runs the Rust context/stage/P0-P1 tests, runs the installed Pi tests, and exercises real candidate recovery, Unicode source budgeting, out-of-slice changes, and fresh-pin rebinding. Python, schema, and pure adapter tests also run. Missing executables or historical assets produce explicit blockers.

For a restored historical corpus and freshly synced baseline, add `--baseline-db /path/to/baseline.db`. Historical controls retain their original single-Rust-profile restriction; generalized multi-profile controls belong to P2.

To run a separately labelled corpus with complete evidence:

```sh
python3 -B scripts/task-evaluate.py --binary target/release/briefcase \
  --root /path/to/repository --db /path/to/evidence.db \
  --scenarios /path/to/scenarios.json --diagnostics \
  --verify-sources --verify-candidates --source-budget 8000 \
  --build-receipt /path/to/validation/build-receipt.json \
  --trace-dir /path/to/new-rpc-directory --output /path/to/results.json
```

Candidate verification deliberately fetches all candidates and measures protocol reachability. It is not a blind harness-discovery score. No historical 36-path or 19-path result is claimed by this change.

## Remaining validation limits

Cargo, rustc, the historical scenario/fixture archive, the retained benchmark database, and a prebuilt binary are unavailable in the supplied environment. Native tests and a real-binary smoke runner were added, but their execution requires those tools. The previously reported Rust assertion must be reproduced on a Rust host before declaring it fixed; its expectation was retained.

P2 remains outside this implementation: resumable upstream FTS/capture/graph limits, lazy whole-snapshot loading, multi-repository federation, and an independent outcome corpus. The reference workflow can expose those gaps and preserve pending work; it cannot recover source absent from the retained corpus. No SDK or ML dependency was introduced.
