# Architectural impact is retained evidence composition

Status: implemented; the evidence-first refactor updates the default guidance and registry interpretation. See the [refactor validation report](../refactor-validation.md) for current validation limits.

## Context

Task Cases identify relevant evidence. A change workflow also needs the
affected relationships, declared constraints, verification candidates and
remaining unknowns for planned work or two captured revisions. Those outputs
must preserve Briefcase's distinction between source evidence, derived
associations, declarations, inferred candidates and executed observations.

## Decision

Add the read-only `impact` operation to the existing operation catalog. Compose
the source snapshot, existing Case/index, report-only policy queries, retained
correspondence and shared findings. Retain one content-addressed v1 report and
reuse `page`, `explain` and the gateway's final transport byte budget.

Compare the full captured file inventories in post mode. Explore forward
dependencies and reverse potential consumers separately, preserving complete
ordered witness trails and both source/index pins. Keep uncertain historical
correspondence separate; only existing exact content-move evidence can remap
logical comparison keys. Keep every unknown and every source omission visible.

Do not create a second graph, assertion authority, decision registry or test
runner inside the engine. Existing report-only declarations cannot authorize
mandatory verification. Static associations suggest checks with explicit
`not_executed` status. Architectural rationale is supplied separately by the
agent and its rationale provider. `governing_decisions: []` and
`governing_decisions_status: not_connected` remain legacy wire fields; they do
not establish that no applicable reasons or policies exist. An absent registry
is not an implementation-evidence gap or a prerequisite to using the report.
Overall assessment stays partial in v1.

## Consequences

Agents and companion architecture/validation tools have one retained change
packet with inspectable proof references. The agent reconciles architectural
reasons with current requirements and implementation under the existing task
authorization. Explicitly adopted policies retain their own scope. Project
tools and external runners own execution and behavioral outcomes. File-level impact can conservatively overselect
entities in a changed file; it is not a semantic API diff. Bounded traversal,
unresolved references, unlocalized endpoints and missing language coverage can
underselect consumers, so those limitations are part of the contract.

The schema is additive and existing Case, declaration and knowledge contracts
keep their authority and version semantics. ADR approval is never inferred from
the existence of this document.
