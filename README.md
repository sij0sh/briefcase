# Briefcase

Briefcase gives coding agents retained repository context with explicit source,
coverage and provenance. Start a task with `brief`; follow relationships through
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
| `brief` | One duty-based packet: anchors, files by role, relationships, history, sessions and explicit gaps |
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

## Reports and source authority

Every core read returns a `report_id`. Ranked results retain their bounded
candidate set; literal/adjacency enumerators pin the view or context index and
request. Follow `continue_with`, or select a section explicitly:

```sh
briefcase --project-root /path/to/repo --json call page --args \
  '{"report_id":"<brief report>","section":"/slice/test","offset":0,"limit":100}'
briefcase --project-root /path/to/repo --json call explain --args \
  '{"report_id":"<search report>","ref":"/items/0"}'
briefcase --project-root /path/to/repo --json call explain --args \
  '{"report_id":"<search report>","ref":"/items/0/evidence_matches/0"}'
```

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
Profile scope is operator-selected; Briefcase does not silently guess a
mixed-language/project analysis matrix.

`inspect-external <absolute paths...>` explicitly checks selected installed
files against retained source digests. Its derived `INSTALLED_FROM` association
means observed byte equality. It does not establish installation causality,
active configuration, process identity or execution.

Pi adds bounded prompt context and stores a post-task advisory coverage report.
Set `BRIEFCASE_CONTEXT=0` to disable it. Claude hooks use supported prompt/stop
events. Codex and Muse keep their supported session-start bootstrap hooks.
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
omits the original evaluation fixtures. This source needs Rust validation
before release.
