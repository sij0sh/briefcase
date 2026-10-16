# Task context

The current context API is documented in [README.md](README.md). Implementation
and migration details are in [CONSOLIDATION.md](CONSOLIDATION.md).

Task context consumes retained evidence from source, data, deployment and
runtime producers. `brief` is the authoritative task packet. `search`,
`occurrences`, `neighbors`, `page` and `explain` share its context/read protocol.
No map or presentation engine is required.

The [task evaluator](scripts/task-evaluate.py) measures labelled file recall,
explicit gaps and first/full-packet delivery. Cases must supply authored
required/follow-up paths; no evaluation baseline or performance figures were
invented. [VALIDATION.json](VALIDATION.json) records what was run here.
