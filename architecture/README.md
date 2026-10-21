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
