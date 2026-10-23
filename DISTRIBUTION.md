# Briefcase distribution

The native executable and Python import producer expose the 0.1.0 JSON interface
used by Archclt 0.2.0. It needs Git and a compatible Python interpreter for native
Python analysis; other language producers have their own external toolchains.
The binary includes its adapters and agent templates. Source builds require Rust
and a C/C++ toolchain for bundled SQLite and parser libraries.

## Build

Run `cargo build --locked --release --bin briefcase`, then
`python3 scripts/package_release.py`. The latter builds a platform archive and
`SHA256SUMS` in ignored `dist/`. `cargo package --allow-dirty` verifies the standard
source crate, including embedded contracts and extension sources. The crate
manifest excludes retained evidence, local validation and evaluation outputs. Static
protocol/oracle and synthetic repair fixtures remain because the reviewed script
tests need them. Generated evaluation results and real session captures are excluded.

The repository evaluation tests also require ripgrep; release runners install
it explicitly alongside Python, Node and Rust.

The release workflow builds Linux x64/ARM64 and macOS x64/ARM64 on native runners.
It runs the reviewed behavioral checks and maps a real Python import through
Archclt 0.2.0 using the extracted release executable, then uploads
artifacts. A tag publishes the archives and aggregate checksums to GitHub.
Required repository checks remain those in architecture/verification.yaml;
release smoke checks do not replace them.

## Verify and install

Use the archive and checksum list from the same immutable release tag. The POSIX
installer defaults to `sij0sh/briefcase`; `--repo`, `--version` and `--dir` override
its selections. It verifies SHA256 before extracting or replacing the binary.
The Windows installer applies the same checksum contract; native Windows release
assets are experimental until built and validated by its release lane.

A checksum identifies downloaded bytes; trust comes from selecting the reviewed
release. Install outside source checkouts and retain release/hash provenance.

## Publication source

The canonical public source and releases are at https://github.com/sij0sh/briefcase.
Existing Git history is retained. Release archives use the explicit Cargo source
allowlist and exclude generated validation, evaluation outputs and local captures.

Library, impact, tooling and adapter checks are required. Historical core and
numbered-stage lanes reference evaluation assets absent from this source checkout;
those lanes remain unverified and must not be reported as passing. Archclt maps
Python imports, so its gate does not enforce Briefcase's Rust structure.
