# Briefcase

Briefcase traces retained evidence from a codebase to build a case around a
task, delivered as a brief. Source, coverage and provenance stay explicit.
Start a task with `brief`; follow relationships through
`neighbors`, locate passages with `search`, and enumerate literal matches with
`occurrences`. `page` and `explain` expand the retained results.

```sh
cargo build --locked
briefcase --project-root /path/to/repo --json brief \
  "rename the session importer safely" \
  --target src/session.rs --duty refactor
briefcase --project-root /path/to/repo --json neighbors src/session.rs \
  --direction in --predicate TESTS
briefcase --project-root /path/to/repo --json occurrences old_binary_name
```

On the first core context read, Briefcase retains a text inventory under its
writer lock. It does not launch compiler adapters or execute project code for
that read. Subsequent reads keep the existing source pin. Use `sync` to refresh
or `capture-text` for an explicit text-only capture. Capture omissions, producer
readiness, pending captures and the relationship to live Git state stay visible.

## Core reads

| Operation | Purpose |
| --- | --- |
| `brief` | A presentation of a retained task case: ranked files by role, relationships, history, sessions and typed gaps |
| `agent_context` | A compact routing manifest over the same case, with a separate target anchor, ranked supplemental files and shared pinned fetch instructions |
| `search` | One lexical ranking over retained text, Git history and optional session requests; eligible assertion annotations keep their own authority |
| `occurrences` | Exhaustive retained literal matches, including overlaps, in path/byte order |
| `neighbors` | Shared incoming/outgoing adjacency for source, test, configuration, data, deployment and runtime relationships |
| `reachability` | Supported transitive static call paths |
| `page` | A retained report section or pinned enumeration |
| `explain` | Exact retained source, assertion provenance or a relationship witness |
| `describe` / `status` | Supported arguments and independent readiness layers |

CLI, MCP and Pi use one operation catalog. `operations` discovers CLI read
schemas; `call <operation> --args '<JSON>'` invokes them. Compare, lineage,
evolution, policy and imported-session inspection remain available through the
same catalog. Canonical write/producer commands remain CLI actions.

`Duty` is one typed vocabulary shared by CLI parsing, schema generation,
routing and coverage templates: debug, refactor, impact, extend, continue,
explain, history, run, review and test. Specify it to override keyword inference.
The enum order breaks ties. Explain/history/test default to depth 1; other
duties default to depth 2. Depth is bounded at 3.

## Task retrieval

One evidence index serves structure, dependencies, tests, declared domains,
execution, data, runtime, Git history and recorded sessions. Lexical retrieval
runs beside a resolved target as well as an ambiguous or missing target.
Located ambiguous identities remain candidates. Eligible domain Claims can
suggest files, but never enter authoritative adjacency or establish membership.

Candidates combine the strongest signal in each facet, an independent-signal
bonus, duty/role relevance, traversal distance, endpoint fanout and hub degree.
Traversal uses localized endpoint files. Holding/support files remain provenance.
The default selected slice has at most **32 files**; `--max-files` or the
`max_files` operation argument changes that bound. Exact anchors dominate rank.
These integer weights are initial heuristics, not calibrated probabilities.
`/candidates` retains the complete discovered ranking, including unselected files.

Git co-change can add files before selection. Complete non-merge commits receive
inverse-size weighting, so a large initial commit contributes little per file.
Session discovery uses cached successful reads/confirmed edits and recorded
requests, requires the same repository/root, and excludes failed reads, edit
attempts and transcript-only path mentions. Discovery considers at most 200
publications, 64 paths per relevant session and 64 bounded requests per summary.
Limits and omissions remain explicit in `coverage.retrieval` and typed gaps.

```sh
briefcase --json brief "debug the retry handler" --target src/retry.rs \
  --max-files 24 --agent-context --max-bytes 12000
briefcase --json call agent_context --args \
  '{"task":"review storage migration","target":"src/store.rs","max_files":32,"max_bytes":8000}'
```

Agent manifests use projection schema version **2.0**. `anchor` describes an
explicit target; when it resolves to one selected file, that file is delivered
there and `files` contains only supplemental paths in case-rank order. Directory,
ambiguous and inferred anchors do not remove paths from the supplemental list.
`max_files` still bounds the retained case. Delivery fills `max_bytes` without a
separate file-count ceiling; the legacy `limit` argument is accepted but ignored
by `agent_context`. The default manifest budget is 8,000 bytes.

Each file has one short `why`, a case-local `ref`, an optional `source_offset`
(default 0) and at most one compact source span. To read it, copy
`fetch_source.arguments`, add that file's `ref` and `source_offset`, then call
`fetch_source.operation` (`explain`). The anchor uses the same template. Per-file
`fetch` objects and structured reason arrays from projection v1 are replaced by
this shared template and the primary reason string. Retained cases and briefs
keep their existing contracts and identities.

`sections` lists all nonempty evidence tables. Copy `fetch_evidence.arguments`,
set `section` to one of those keys, then call `page` and follow its cursor.
`continue_with` restarts the pinned candidate inventory at offset 0 so removing
an anchor or omitting selected files cannot skip candidates. Full coverage,
retrieval diagnostics and readiness remain available through `explain` with
`ref: "/coverage"` or `ref: "/readiness"` and the same `case_id` as `report_id`.
The three-gap preview and `coverage.delivery` describe presentation, while
`coverage.completeness` always preserves the retained case's evidence states.

The compact advisory `freshness` includes projection `state`, `view_count`,
source capture age/state, `pending_or_interrupted`, `git_state`, `dirty_state`,
`dirty_overlap_count` and a Boolean `fallback`. A consistent projection does not
establish equality with live files. Unknown ages, unavailable checks and bounded
dirty-path diagnostics remain unknown; dirty overlap counts only the delivered
anchor and supplemental paths. Ordinary reads retain the full freshness envelope.

For controlled evaluation, `exclude_facets` accepts any of `structure`,
`dependency`, `tests`, `domain`, `execution`, `data`, `runtime`, `history`,
`sessions` or `lexical`. Disabling a facet changes case identity and records a gap.

## Reports and source authority

Every core read returns a `report_id`. Ranked results retain their bounded
candidate set; literal/adjacency enumerators pin the view or context index and
request. Follow `continue_with`, or select a section explicitly:

```sh
briefcase --project-root /path/to/repo --json call page --args \
  '{"report_id":"<task-case ID>","section":"/slice/test","offset":0,"limit":100}'
briefcase --project-root /path/to/repo --json call explain --args \
  '{"report_id":"<search report>","ref":"/items/0"}'
briefcase --project-root /path/to/repo --json call explain --args \
  '{"report_id":"<search report>","ref":"/items/0/evidence_matches/0"}'
```

For a brief, `id`, `report_id` and `case_id` identify the same retained
`task-case:` artifact. Changing `limit` or `max_bytes` changes delivery without
changing that case. The case retains the full tables; the brief itself is not
stored. Older `task-brief:` reports remain pageable and explainable.

`coverage.completeness` records `anchors`, `slice`, `neighborhood`, `history`,
`sessions`, `gap_inventory` and `verification`. Each is `established`, `partial`,
`unknown` or `not_required`. `outcome: complete` means complete for the retained
evidence scope, stated by `outcome_scope: retained_evidence`. It does not
establish runtime truth or task completion. Duties requiring executed tests
remain partial under the current retained evidence model. `coverage.delivery`
only counts delivered and omitted rows. Gaps are ordered by dimension name
and area; their reasons retain producer scope.

Slice rows include global `rank`, integer `score`, `selection_reasons`, relevant
`spans`, a `/candidates` reference and `reached_by: {depth, via_link}`. A graph
signal can be inspected with `explain` using its link ID. Lexical/history/session
hints may have no graph link; their candidate records retain their original
report, commit or recorded-event references. A retrieved file does not establish
feature ownership or executed test coverage. Candidate-only files cannot satisfy
a required role's established-association count.

`neighbors` returns compact coverage counts and a pinned index detail call.
Producer occurrences are retained once per content-addressed detail report.
Task gaps aggregate local occurrences by producer/run with predicate/reason
counts, at most three examples and an exact detail continuation. Outside-task
counts stay in health metadata. Unlocalized health is explicitly non-actionable.

Search is bounded to 1,000 lexical candidates and 1 MiB of excerpts before
path/class filtering. `component` filters an exact retained file or directory
prefix. `classes` selects text, evidence, derived, declaration, claim, history
or session candidates. Assertion annotations overlap the excerpt where span
locators are available, are deduplicated and are capped at 32 per excerpt.
Omissions do not establish absence. Search does not accept graph expansion or
historical-assertion mode; use neighbors and pinned explanations for those reads.

Captured text and recorded requests are untrusted data. A textual match does
not establish truth or ownership. Static test associations do not establish
executed coverage. Runtime observations retain their environment, interval and
sampling limits. Later retractions annotate retained reports and explanations.

## Evidence producers and profiles

Data analysis, deployment capture and runtime ingestion produce scoped evidence
for the context index. They have no separate read-product families. `brief` and
`neighbors` expose their relationships alongside source relationships.
Deployment joins require the selected source snapshot and exact retained build
inputs. Runtime joins require explicit retained resource identities; service
name similarity alone is insufficient.

Mixed projects use explicit profiles in `.briefcase/sync.json`:

```json
{"profiles":[
  {"language":"rust","calls":true},
  {"language":"typescript","project":"ui/tsconfig.json","calls":true}
]}
```

Compatible profiles compose only at the same source snapshot. Static call
extraction defaults to enabled when supported; set `calls:false` or use
`--no-calls` to opt out. Missing providers and exhausted budgets remain gaps.
Rust LSP startup/version, one workspace-readiness window and native lookups share
one deadline. Null definitions are single misses after that window. Build scripts,
proc macros and network fetching stay disabled; installed sysroot discovery and
all Cargo targets are enabled. Partial native call runs remain retryable and
carry a `native_resolved_subset` guarantee scope. They cannot satisfy complete
static-call readiness. Deadline checks surround cooperative phases and atomic
publication; this is not hard real-time process preemption.
Profile scope is operator-selected; Briefcase does not silently guess a
mixed-language/project analysis matrix.

`inspect-external <absolute paths...>` explicitly checks selected installed
files against retained source digests. Its derived `INSTALLED_FROM` association
means observed byte equality. It does not establish installation causality,
active configuration, process identity or execution.

Pi and Claude prompt hooks request `agent_context`, with a default 8,000-byte
context budget including framing text. Set `BRIEFCASE_CONTEXT_MAX_BYTES` to an
integer from 4,096 to 60,000 to change it. The final gateway budget includes
freshness and byte accounting; MCP supplies its complete JSON-RPC serializer,
including both result representations. Diagnostics and span detail yield before
ranked supplemental files. Omitted evidence stays addressable through pinned
`page`/`explain` calls.
Pi also stores a post-task advisory coverage report.
Its report keeps initial/review case IDs and completeness dimension changes.
Claude's completion hook also compares full case slices, opened/closed gaps
and advisory dirty paths outside the initial case.
Set `BRIEFCASE_CONTEXT=0` to disable it. Claude hooks use supported prompt/stop
events. Codex and Muse retain session-start bootstrap; per-prompt delivery is
not automatic for those adapters. Start their coding tasks with the shared
`agent_context` operation through CLI/MCP. Integration status reports this
delivery distinction explicitly.
Completion feedback never submits another agent prompt or blocks task completion.

## Migration and validation

Simplification and human map/presentation implementations, commands, schemas,
renderers and product-specific evaluators were removed. `ask` aliases `brief`;
`recall`/`retrieve` alias `search`; dependencies/callgraph alias filtered
neighbors. These compatibility names are not advertised as separate products.
Optional feature/workflow declarations remain metadata consumed by context.
Removed map and simplification names fail discovery and execution.

See [CONSOLIDATION.md](CONSOLIDATION.md) for implementation changes and remaining
validation work, and [VALIDATION.json](VALIDATION.json) for the check record.
Rust compilation, Rust tests and lints were not run in the implementation
host because the Rust toolchain is unavailable. The supplied archive also
omits the original package/conformance fixtures. Eight regression scenarios
from the findings are now in [evaluation/task-scenarios.json](evaluation/task-scenarios.json).
See [the implementation record](evaluation/IMPLEMENTATION.md) for validation and
remaining empirical checks. This source needs Rust validation before release.
