# Task context

The current context API is documented in [README.md](README.md). Implementation
and migration details are in [CONSOLIDATION.md](CONSOLIDATION.md).

Task context traces retained evidence from source, data, deployment and
runtime producers to build a case, delivered as a brief. The content-addressed
Case owns typed gaps and completeness; the brief owns presentation. `search`,
`occurrences`, `neighbors`, `page` and `explain` share its context/read protocol.
No map or presentation engine is required. Selection now follows
`request → faceted discovery → candidate fusion/ranking → bounded slice → case`.
`agent_context` projects that same case for coding harnesses. All available
channels can influence candidates within reported bounds; scores do not establish
implementation membership or completeness.

The [task evaluator](scripts/task-evaluate.py) measures labelled file recall,
Recall@K, required-file ranks, labelled precision, selected source/response
bytes, actionable gaps, false-complete outcomes and actual harness delivery. Scenarios must supply authored
required/follow-up paths; no evaluation baseline or performance figures were
invented. [VALIDATION.json](VALIDATION.json) records what was run here.

Use `--scenarios` for evaluator input; `--cases` remains a deprecated alias.
`false_complete_rate` counts complete outcomes that miss authored required
paths divided by scored scenarios; errors are excluded from that denominator
and remain failures. `false_complete_given_complete_rate` uses only complete
outcomes as the denominator. The eight finding scenarios are available in
[evaluation/task-scenarios.json](evaluation/task-scenarios.json). Their updated
results and larger-corpus quality have not been measured here.


```sh
python3 scripts/task-evaluate.py --binary target/debug/briefcase --root . \
  --scenarios evaluation/task-scenarios.json --ablations all --output task-results.json
```

`--manifest-bytes` defaults to 8000, including the selected adapter framing;
`--harness pi|claude-code|manifest` selects that accounting. `--skip-manifest` supports legacy binaries
without claiming harness delivery. Ablations are sequential and warm after the
first query. Labelled precision counts only authored required paths as relevant;
other useful files may be unlabelled. Token costs are explicit byte-based estimates.
