---
id: ADR-B5
title: Coverage and gaps are explicit, never inferred from success
status: accepted
components:
- package
- contracts
- store
- capture
- analysis
- service
- retrieval
- reconcile
supersedes: []
---
# Coverage and gaps are explicit, never inferred from success

## Decision and rationale
Every retained result states its evidence scope, coverage state and gaps.
Partial capture, unresolved bindings, unsupported constructs and missing
toolchains remain visible. A successful command does not establish absent
edges, complete runtime behavior or correct task intent.

## Alternatives
Treating a successful scan or command as universal coverage would turn
missing evidence into a false pass and make later review unreliable.
