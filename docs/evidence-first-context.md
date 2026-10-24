# Evidence-first coding context

Briefcase supplies retained implementation and session evidence. The coding
agent uses that evidence, current requirements, and architectural reasons to
choose and implement a design. Evidence completeness does not grant permission
or decide whether a task succeeded.

## Ordinary work

Reuse a relevant fresh Case or supplied manifest. Direct source inspection can
suffice for a clear local change. Use `agent_context` for a compact retained
investigation, or `brief` for its full sections, when that investigation helps.

Read `not_covered` for questions that could change the design or verification.
Investigate those questions, then implement and test. A partial history,
unavailable runtime capture, or unsupported language layer limits the claims
you can make; it does not automatically block unrelated supported work.
An unavailable decisive fact remains unresolved.

Selection is a reading batch, not declared architectural ownership. Use the
existing `inventory` operation with `case_id` and `selected: false` to inspect
unselected alternatives. `expand_case` creates a new immutable revision when
more analytical scope is needed. Read source through the returned native
references; continue against their original pins.

Gap previews now include their retained `/not_covered/<index>` reference and a
path when the gap has one. Sorting the preview does not change that reference.
Use the manifest's `fetch_source` template with the gap's `ref` to inspect its
full detail. `coverage.delivery.findings` records returned and omitted findings,
as the existing counters do for files and gaps. Retained Case identities and
the manifest's `schema_version: 2.0` remain unchanged by these projection fields.

The manifest retains a gap warning under budget pressure. If a complete routing
packet cannot fit, the pinned omission notice preserves completeness, a gap
preview, and truthful delivery counts. Increase `max_bytes` or follow the
retained references. Omission is not absence.

## Automatic adapters

Pi and Claude Code obtain prompt context and perform a live `worktree`
diagnostic against the original Case at turn completion. The default no longer
builds a second ranked review Case or fetches completeness metadata just to
produce a turn-end comparison. A diagnostic does not refresh a capture or
certify the implementation.

Set `BRIEFCASE_REVIEW_CONTEXT=1` to retain the previous automatic full-review
behavior. Pi keeps its `briefcase-completeness` entry type; the default records
`dimension_comparison: not_requested`. The full review records compared or
unavailable dimension states. Native default hook reports use
`kind: case_worktree_check`; explicit full-review reports retain
`kind: case_comparison`. Existing reports remain readable.

`BRIEFCASE_CONTEXT=0` disables automatic prompt/completion context in both
adapters. `BRIEFCASE_CONTEXT_MAX_BYTES` retains its 4,096–60,000 byte range and
8,000 byte default, including framing text. `BRIEFCASE_SYNC=0` continues to
control automatic monitor maintenance independently.

Pi reads use `--wait-ready 0`: they do not wait for an unrelated writer before
asking the native operation for its supported result. This does not change
explicit CLI `--wait-ready` or `sync --require-ready` semantics. A pending
source capture can use a labelled published view through the existing native
fallback. Corrupt support, schema errors, and unrelated failures remain errors;
the adapter does not silently refresh or reinterpret them as current support.

Pi recognizes both compact native fallback metadata (`fallback: true`) and
expanded metadata (`fallback.used: true`). Its turn-end diagnostic stays bound
to the workspace and Case that supplied the prompt even if the UI changes its
current working directory.

## Architectural rationale and impact

`impact` composes retained changes, potential consumers, declared constraints,
unexecuted check suggestions, and implementation-evidence gaps. Architectural
rationale can be supplied separately by Archclt or existing project documents.
Briefcase does not require a rationale provider or an accepted architecture
model for ordinary evidence retrieval.

The v1 `governing_decisions: []` and
`governing_decisions_status: not_connected` fields remain for wire compatibility.
They describe the legacy integration's capability, not absence of applicable
reasons or policies. An unconnected registry no longer creates an unconditional
`design_decisions` unknown. Actual implementation and execution limitations
remain visible. Explicitly adopted project constraints keep their scope.

## Outcome evaluation

The existing generated task corpus now includes `prerequisite_migration` and
`intentional_separation`. The first requires repair of a shared lifecycle,
migration of its compatibility consumer, and genuinely overlapping jobs. The
second permits sharing pure logic while preserving immediate versus durable
state semantics. Checks exercise behavior and contracts rather than exact
reference-patch equality.

`task-corpus-evaluate.py` rejects an outcome oracle that cannot distinguish the
original and repaired implementation. A failed or timed-out external harness
is an error even if a later test could pass. No-harness runs remain `not_run`.
These checks improve the evaluation machinery; they are not a measurement of
coding-agent success or token savings.

## Compatibility and verification

The ledger, source pins, retractions, immutable Case schema, investigation
bounds, and producer profiles are unchanged. Existing explicit expansion and
layer-specific readiness already serve the target architecture. No second
graph, rationale store, correctness score, or approval workflow is introduced.

See the [impact contract decision](decisions/architectural-impact-contract.md)
and the [implementation and validation report](../BRIEFCASE_REFACTOR.md).
