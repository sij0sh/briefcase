# Contract files and validation profile

| File | Boundary |
| --- | --- |
| `knowledge.schema.json` | Records, tagged assertion union, scopes and fixture/interchange bundle |
| `batch.schema.json` | One typed assertion class, retained transport payload and generation expectation |
| `query.schema.json` | Named request definition and scoped response envelope |
| `predicate-registry.json` | Active/reserved vocabulary, subject/object kinds, classes and scope |
| `identity-vectors.json` | Independent canonical JSON literals and expected SHA-256 values |
| `architecture-declaration.schema.json` | The `scan --policy` authority artifact: domain assignments, report-only rules, the declared application and the feature/workflow catalog (ADR-0028, ADR-0029) |
| `task-case.schema.json` | Retained task authority, request, role slices, relationships, typed gaps and completeness |
| `task-brief.schema.json` | V1 presentation of a retained case, with case ID, delivery counts and continuation |
| `session-activity.schema.json` | Derived cross-session activity per repository path in a time window, with net change and retained commits kept separate (ADR-0030; not a knowledge record) |

Optional `fixture_label` is review metadata, not a production identity input. Readable fixture IDs are aliases used in isolated examples. Production constructors must implement the semantic identity keys in the canonical constructors under `src/architecture/identity.rs`.

## Offline structural profile

Supported keywords are `$schema`, `$id`, `$defs`, local `$ref`, `title`, `description`, `type`, `properties`, `required`, `additionalProperties`, `enum`, `const`, `oneOf`, `anyOf`, `items`, `minItems`, `maxItems`, `uniqueItems`, `pattern`, `minLength`, `maxLength`, `minimum`, `maximum`, and the `date-time` format. Other keywords and external references are rejected. The profile deliberately has no network resolution and a nesting limit.

The JSON documents use Draft 2020-12 syntax. A full standard validator can validate their structural shapes independently. The bundled profile is **not** asserted to implement the full standard or all its metaschema validation rules. Reference integrity, scope eligibility, support provenance, evidence digests, byte boundaries and capability checks are the separate semantic layer.

## Interpretation limits

Fixtures may describe future language/runtime capability with authored records, explicitly marked by producer/model names. They are not compiler exports or live observations. A record passing schema and provenance checks may still be semantically wrong. Real producer evaluation, trusted caller capabilities, active-policy selection, production identity generation, and storage durability remain release prerequisites.

## Additive substrate records

Production bundles can include `pass_manifests`. Existing authored bundles without this optional family remain valid. Each manifest pins pass/contract version, implementation/configuration, context, input assertions/artifacts, prerequisite manifest IDs, coverage, completion, and output artifact IDs. Derivations can reference a manifest and its content artifacts. Mutable pins and dependency cycles are rejected.

Input packets and inference records can reference retained exact packet, prompt, configuration, request, output, taxonomy, and projection artifacts. Domain Claims use only packet support and stay distinct from declarations. Scores are metadata. A Claim identity uses its proposition, support, scope, and inference origin without a floating-point primitive. The inference ID binds the output byte digest, including the original score. Duplicate JSON keys, normalized-key collisions, and float canonical identity input remain invalid.

Module versions can identify their retained source evidence reference. Retrieval manifests can pin chunker/tokenizer, disabled embedding mode, dimensions, configuration, source artifacts, and representation unit IDs. The canonical `search` operation ranks retained text and attaches eligible, span-scoped assertion metadata. `domains` and `drift` retain their class-specific reports. These reads do not redefine entity or assertion identity.

## Static execution extension

`REACHES_STATIC` permits only source-context Derived relationships between symbols.
`CALLS_STATIC` still requires compiler-resolved Evidence with native endpoint identities.
An unresolved occurrence may name its predicate without changing older records.
A named unresolved predicate must be active in that context.
`callgraph` and `reachability` share the existing bounded query response contract.
Completed pass manifests must satisfy their declared predicate-specific coverage requirements.

Source overlays may atomically publish Evidence and Derived batches through explicit local grants.
Each batch still contains exactly one assertion class.
The complete selection must equal the authorized batch membership.
No client or producer input can grant Claim or Declaration authority.
Uncertain correspondence stays in retained candidate artifacts and deterministic calculation records.
It adds no exact lineage predicate or canonical identity merge.

## Data, deployment and runtime producers

Context kind `deployment` with record family `deployment_contexts` scopes desired-state knowledge from retained build manifests, OCI manifests and Kubernetes YAML. Evidence in that context must cite an input in the deployment manifest. Runtime contexts can name `collector_id` and the observed `artifact_digests`.

Active additions:

- `READS`/`WRITES` (symbol → table/column; Evidence and Derived)
- `TRANSFORMS` (symbol → symbol; Evidence with a typed `transformation`)
- `MAPS_TO` (ORM symbol → table/column)
- `ROUTES_TO` (endpoint → handler symbol)
- `BUILDS` (source revision → artifact)
- `DEPLOYS_AS` (artifact → Deployment, Deployment → Service; Evidence and Declaration)
- `DECLARES_CONNECTION` in deployment context
- Derived `CALLS_OBSERVED`/`CONNECTION_OBSERVED` aggregate windows

`PRODUCES` and `CONSUMES` stay reserved; `PARTICIPATES_IN` remains a declared workflow binding.

Each producer kind writes only its vocabulary: `MAPS_TO` needs `schema`; data operations and routes need `semantic`; `BUILDS` needs `build`; `DEPLOYS_AS` needs `deployment`; observations need `runtime`. Build, deployment and runtime producers cannot write any other predicate.

Analyzer outputs are ordinary scoped evidence consumed by the context index.
`brief` and `neighbors` expose source, data, desired deployment and runtime
relationships through one operation family. Exact source-snapshot, artifact,
resource-identity and interval requirements remain explicit. The removed map,
feature projection, human export and simplification reports have no schemas or
read operations. Optional feature/workflow declarations remain metadata.

Report pagination uses `page` with a retained `report_id` and section/cursor;
`explain` expands source entries, links or legacy explanation handles. See
[README](../../README.md) and [consolidation notes](../../CONSOLIDATION.md).
