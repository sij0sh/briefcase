---
id: ADR-B1
title: Retained evidence and pinned views are the query authority
status: accepted
components:
- package
- contracts
- store
- capture
- retrieval
- reconcile
supersedes: []
---
# Retained evidence and pinned views are the query authority

## Decision and rationale
Source, runtime, history and session observations are retained as evidence with
source pins and provenance. Queries read pinned views. Observed evidence can
challenge declared knowledge but never becomes an accepted architectural rule
by itself. A later live tree does not silently change an existing report.

## Alternatives
Reading the mutable working tree during a query would make reports irreproducible
and would erase the distinction between observation and authority.
