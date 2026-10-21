---
id: ADR-B2
title: Language adapters are producers behind validated contracts
status: accepted
components:
- package
- contracts
- capture
- analysis
- store
- reconcile
supersedes: []
---
# Language adapters are producers behind validated contracts

## Decision and rationale
Python, Rust, TypeScript, Go, Java, C#, C and C++ adapters report bounded native
occurrences. The shared contracts layer validates their batches before the
ledger publishes them. Adapters do not own query authority, coverage claims or
architectural decisions. A failed or partial adapter remains an explicit gap.

## Alternatives
Letting each adapter write its own graph or query surface would duplicate
validation and make coverage semantics language-specific and inconsistent.
