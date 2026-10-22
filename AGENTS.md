# Working on Briefcase

Read `architecture/README.md`, `architecture/decisions/`, `architecture/verification.yaml`
and the relevant source before changing a module boundary or verification command.

Use the reviewed archctl installation outside this checkout for local
architecture verification. Set `ARCHCTL_BIN` and `BRIEFCASE_BIN` to the pinned
paths in `validation/dogfood/README.md`.

Architecture-affecting work follows:

Investigate -> Change Contract -> Implement -> Fresh evidence -> Local gate ->
Review packet.

Routine work uses existing authorization. Changes to architectural authority or
to executable verification commands require review.

## Evidence limitation

Archctl v1 maps Python imports only. Briefcase's Rust component boundaries are
declared for review, not enforced by structural evidence. A passing archctl gate
must not be described as Rust structural enforcement.

Use the pinned Briefcase binary as the evidence producer, even while changing
Briefcase source. Do not use a candidate checkout as the evaluator or evidence
producer.

## Verification

The required commands are recorded in `architecture/verification.yaml`.
Missing toolchains and omitted evaluation assets remain explicit gaps. Never
convert a skipped or missing check into a pass.
