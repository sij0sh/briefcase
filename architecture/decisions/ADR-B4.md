---
id: ADR-B4
title: Behavioral verification is required for all product components
status: accepted
components:
- package
- cli
- contracts
- store
- capture
- analysis
- service
- retrieval
- history
- integrations
- reconcile
- tooling
supersedes: []
---
# Behavioral verification is required for all product components

## Decision and rationale
The reviewed verification commands exercise the Rust library, the impact
contract, repository tooling, the Node adapter and lint checks. Every product
component maps to the same required verification set. A missing or failed
command is not a pass.

## Alternatives
A narrower command set per component would reduce runtime, but it would also
make it possible to change a shared contract without running the checks that
exercise it. The set can be narrowed only after the dependency mapping is
reviewed and measured.

## Consequences
The initial model has no Rust import-edge evidence. Structural Rust boundaries
are declared for review, not enforced by archctl v1. A dedicated Rust boundary
test must be added before claiming structural enforcement.
