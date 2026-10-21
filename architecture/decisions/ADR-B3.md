---
id: ADR-B3
title: CLI, MCP and Pi share one read service
status: accepted
components:
- package
- cli
- service
- retrieval
- integrations
supersedes: []
---
# CLI, MCP and Pi share one read service

## Decision and rationale
CLI, MCP and Pi resolve operations through one catalog and one validation and
response gateway. A shared gateway keeps argument handling, pinned fallback,
freshness and response budgets consistent across transports. Transports frame
responses; they do not reimplement retrieval.

## Alternatives
Per-transport operation parsers and response rules would drift, and one
transport could accidentally bypass a validation or freshness rule.
