# Briefcase

Briefcase retains source evidence and builds task context with explicit source
pins, provenance and coverage gaps. It shares a read service across the CLI,
agent adapters and optional MCP transport. It is also the native Python import
evidence producer used by [Archclt](https://github.com/sij0sh/archclt).

## Install

Download a platform archive and `SHA256SUMS` from the reviewed GitHub release.
The POSIX installer verifies the checksum before replacing the executable:

```sh
sh scripts/install.sh --version v0.1.2
```

Use the installer from the matching source release, or download and inspect its
versioned source first. `--repo`, `--version` and `--dir` select the release and
installation. The default destination is `~/.local/bin`, outside your project.
Linux and macOS x64/ARM64 have release build lanes. Windows installer support
remains experimental until native Windows artifacts are validated.
See [distribution](docs/DISTRIBUTION.md) for source builds and publication status.

## Read a repository

```sh
briefcase --project-root /path/to/repo --json brief \
  "rename the session importer safely" --target src/session.rs --duty refactor
briefcase --project-root /path/to/repo --json neighbors src/session.rs \
  --direction in --predicate TESTS
briefcase --project-root /path/to/repo --json occurrences old_binary_name
```

The first core context read retains text and file inventory. It does not execute
project code or compiler adapters. Reads preserve the source pin; use `sync` to
refresh. Capture omissions, unresolved analysis, pending preparation and source
freshness stay explicit. A successful operation does not prove complete coverage.

`operations` discovers supported read schemas. `call <operation> --args '<JSON>'`
invokes them. `page` expands a retained section and `explain` follows source or
relationship provenance. Literal search is a locator; a task case records the
scope and limits of an investigation.

## Archclt integration

Set `BRIEFCASE_BIN` to the absolute reviewed executable outside source checkouts.
Briefcase 0.1.2's public JSON operations supply the Python import evidence mapped
by Archclt 0.2.0. Other language analysis supported by Briefcase does not imply
that Archclt enforces those languages. Language producers may need Python, Node,
Rust or other external compiler toolchains; inspect their readiness explicitly.

## Develop

```sh
npm ci --ignore-scripts --prefix tools/typescript
bash scripts/verify-context
bash scripts/verify-lint
cargo build --locked --release --bin briefcase
```

`scripts/verify-context` is the required lane and needs Node plus the pinned
TypeScript compiler. `scripts/verify-core` needs evaluation fixtures that this
checkout does not ship, so it stays opt-in. `scripts/verify-mvp` runs every lane
and skips the opt-in one. TypeScript analysis always needs an explicit compiler:
`--typescript-compiler` or `BRIEFCASE_TYPESCRIPT_COMPILER`.

Rust and a C/C++ toolchain are needed for source builds. The standard source
crate includes the embedded adapters, contracts and extension templates. It
excludes local captures, dependency installations and generated evaluation data.
Some historical integration lanes need evaluation fixtures absent from this
source checkout; they remain unverified. See [distribution](docs/DISTRIBUTION.md)
and the architecture records for the exact validation boundary.

MIT licensed.
