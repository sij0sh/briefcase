# Briefcase architecture model

This directory declares the reviewed component ownership, decisions and
verification commands for Briefcase.

## Structural evidence limitation

Archctl v1 maps Python imports only. Briefcase's product code is Rust. The
component boundaries and decisions here are therefore review intent, not
Rust structural enforcement. A passing archctl gate for this repository does
not prove that Rust import boundaries hold.

The required fix is a dedicated Rust boundary test. Until that test exists and
is reviewed, do not describe the archctl result as structural enforcement.

## Verification

The required commands are listed in `verification.yaml` and required for every
component by `policies.yaml`. Missing toolchains and omitted evaluation assets
remain explicit gaps. They are not converted into passes.

The enforced PR lanes are `scripts/verify-context` and `scripts/verify-lint`;
together they run every command listed here. Install Node and the pinned
TypeScript compiler first with `npm ci --ignore-scripts --prefix tools/typescript`.
`scripts/verify-core` needs `evaluation/fixtures` and `evaluation/sources`, which
this checkout does not ship; it stays opt-in and its asset tests are `#[ignore]`d.
Formatting is not enforced: `cargo fmt --check` does not pass on this tree.
