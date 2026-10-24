# Briefcase refactor: implementation and validation

## Result

Implemented the Briefcase portion of the architectural-context pivot. The
default path supplies evidence for an agent's design decision, supports focused
expansion, and avoids routine acceptance comparisons. Architectural rationale
remains a separate responsibility.

The refactor is applied in the complete checkout, with its root build manifests
and toolchains. The required native and adapter lanes pass.

One delivery regression appeared only when the native suites ran: budget
trimming spent a row on advisory findings and dropped a path that the delivery
floor requires. Trimming now drops findings before the gap warning and before
the delivered paths. The gap warning and the omitted-row counts are unchanged.

## Implemented behavior

| Area | Change |
| --- | --- |
| Default turn completion | Pi and native Claude hooks reuse the original immutable Case for a live worktree diagnostic. They no longer automatically assemble a second review Case or require a dimension comparison. |
| Compatibility | `BRIEFCASE_REVIEW_CONTEXT=1` restores automatic full review. Pi retains the `briefcase-completeness` entry type with explicit `not_requested`, `compared`, or `unavailable` comparison status. Native default diagnostics use `case_worktree_check`; full review retains `case_comparison`. |
| Relevant readiness | Pi reads use zero automatic writer wait. Explicit CLI waiting and `sync --require-ready` retain their semantics. Compact and expanded native fallback metadata are both recognized. |
| Workspace identity | Pi completion diagnostics use the original prompt workspace and Case, even if the UI changes its current directory. |
| Gap support | Reordered gap previews carry the original `/not_covered/<index>` reference and a path when available. |
| Bounded delivery | Trimming drops advisory findings first, preserves a gap warning, and records omitted findings. Pinned omission notices carry scoped completeness, support and truthful delivery counts. |
| Impact/rationale boundary | An absent decision registry no longer creates an unconditional implementation-evidence gap. Existing empty registry fields remain wire-compatible and explicitly do not prove absence of applicable reasons or policies. |
| Agent guidance | Pi/MCP guidance permits fresh-context reuse and direct inspection for clear local work. It directs investigation toward questions that could change design or verification and exposes existing unselected-candidate inventory. |
| Outcome evaluation | Added necessary-migration and intentional-separation corpus families, actual overlap/lifecycle checks, and a valid alternative-design check. Harness failures/timeouts cannot become verified successes; non-discriminating outcome oracles are rejected. |

## Relationship to the implementation plan

- **B1, compact delivery:** implemented support-preserving previews, warning
  retention, and explicit finding omissions in existing projection/budget owners.
- **B2, freshness:** removed adapter-imposed waits and fixed compact fallback
  interpretation. Existing native text bootstrap, layer states, explicit sync,
  and integrity failures remain in place; no new refresh subsystem was needed.
- **B3, alternative foundations:** exposed the existing inventory/expansion path
  in default guidance and added contrasting outcome fixtures. Retrieval already
  has candidate inventories and explicit expansion; no evidence justified a
  second ranker or larger global budgets in this task.
- **B4, impact:** implemented the rationale boundary while preserving v1 fields,
  actual source deltas, suggested-check status, and implementation unknowns.
- **B5, adapters:** removed unconditional full-review investigation and redundant
  metadata reads. Existing operation discovery remains available on demand;
  there was no adapter-owned schema-discovery loop requiring another cache.
- **B6, durability:** preserved ledger, Case assembly/schema, pins, completeness
  logic, sync, and explicit CLI lifecycle code. No migration of retained records
  is required. The added native regressions run in the library and context
  targets.
- **B7, outcomes:** extended the existing corpus/evaluator and ran its
  deterministic fixture tests. A coding-agent benchmark still needs an external
  harness; no end-to-end improvement is claimed.

## Verification performed

| Check | Result |
| --- | --- |
| `scripts/verify-context` | Pass. Runs the library, context, impact, stage suites, both Python suites and the adapter tests. |
| `scripts/verify-lint` | Pass. Owned schemas, Python suites, artifact report, Clippy, adapter syntax and gofmt. |
| `cargo test --locked --lib` | 182 passed. |
| `cargo test --locked --test context` | 62 passed. |
| `cargo test --locked --test impact` | 17 passed. |
| `cargo test --locked --test p0p1 --test p2 --test stage2 --test stage3 --test stage4` | 7, 11, 3, 5 and 4 passed. |
| `node --test tests/briefcase-task-adapter.test.mjs` | 20 passed. |
| `python3 -B -m unittest discover -s tools -p 'test_*.py'` | 17 passed. |
| `python3 -B -m unittest discover -s scripts -p 'test_*.py'` | 135 passed. |
| `python3 -B tools/check_package.py --schemas-only` | All 11 owned schemas passed. This is not full historical package conformance. |

Observed limits:

- One `scripts/verify-context` run failed
  `tests/briefcase_stage3.rs::invalid_profile_preflight_reports_configuration_without_creating_a_database`
  with `sync: locked`. The same flake is recorded for a clean commit in
  `.agents/artifacts/source/briefcase-source.md`. Repeated runs passed.
- `tests/briefcase_consolidation.rs` is not a `Cargo.toml` test target and no
  other target includes it. Its assertions compile but do not run here.
- Clippy reports warnings only. They sit on unchanged lines and the lane treats
  them as non-blocking.
- Structured validation details are in
  [docs/refactor-validation.json](docs/refactor-validation.json).

## Remaining validation

Unrun in this checkout:

- Installed Pi and Claude Code adapter integration outside the unit harness.
- A release package built from this tree. The archives under `dist/` predate the
  refactor.
- Coding-agent outcome measurement, including token cost and task success.

Static source review and passing Python, JavaScript and Rust checks do not
substitute for those results.

## Use and rollback

The [usage and compatibility guide](docs/evidence-first-context.md) documents
the new defaults. Existing explicit source capture, policy inspection, and
project verification commands continue to use their existing contracts.

For the previous automatic review behavior, set `BRIEFCASE_REVIEW_CONTEXT=1`.
To disable automatic prompt/completion context, use `BRIEFCASE_CONTEXT=0`.
Automatic monitor maintenance remains independently controlled by
`BRIEFCASE_SYNC`.

Existing retained records do not need rewriting or repinning. A code rollback
can restore the original source while leaving those records intact.
