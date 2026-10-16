# Task context

The current context API is documented in [README.md](README.md). Implementation
and migration details are in [CONSOLIDATION.md](CONSOLIDATION.md).

Task context traces retained evidence from source, data, deployment and
runtime producers to build a case, delivered as a brief. The content-addressed
Case owns typed gaps and completeness; the brief owns presentation. `search`,
`occurrences`, `neighbors`, `page` and `explain` share its context/read protocol.
No map or presentation engine is required.

The [task evaluator](scripts/task-evaluate.py) measures labelled file recall,
explicit gaps, false-complete outcomes and first/full-brief delivery. Scenarios must supply authored
required/follow-up paths; no evaluation baseline or performance figures were
invented. [VALIDATION.json](VALIDATION.json) records what was run here.

Use `--scenarios` for evaluator input; `--cases` remains a deprecated alias.
`false_complete_rate` counts complete outcomes that miss authored required
paths divided by scored scenarios; errors are excluded from that denominator
and remain failures. `false_complete_given_complete_rate` uses only complete
outcomes as the denominator. Neither rate has been measured on the missing
evaluation corpus.
