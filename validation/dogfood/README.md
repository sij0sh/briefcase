# Briefcase dogfood record

Status: bootstrap candidate. This record is not an independent operator approval.

## Pins

- archctl source: `98ffa18c78b8b830e3cc8b2adae50890ea9ad9a9`
- archctl wheel sha256: `690bbda83b2abc74c6fc55bd127aede387a18d2c503bc844599379dceddd932f`
- Briefcase source: `adf8ff83d647825f3b3869b91b91ff1707b3fa1e`
- Briefcase binary sha256: `3a99588311f587fc0c7f9895084bb862e5cf6f9d8d53dac2f6cff0e0b1caac8f`

## Baseline

- Reviewed candidate: `639113f9e57d7a2d7df8dddc078766408281eb95`
- Gate: passed with `--scope all --run`
- Report: `bootstrap/gate-summary.md`
- Evidence coverage: complete for declared components and Python imports
- Required commands: Rust library, impact, tools, scripts, Node adapter, clippy

## Limitation

The required commands pass, but archctl v1 has no Rust structural evidence for
this repository. The component boundaries and ADRs are review intent. They are
not enforced by import-edge checks.

The full core test lane is outside the baseline because evaluation fixtures and
session assets are absent from the supplied source bundle. Those failures are
not represented as passes.

## Reproduce

```bash
ARCHCTL_BIN="$HOME/.local/opt/archctl/98ffa18c78b8b830e3cc8b2adae50890ea9ad9a9/venv/bin/archctl"
BRIEFCASE_BIN="$HOME/.local/opt/briefcase/adf8ff83d647825f3b3869b91b91ff1707b3fa1e/briefcase"

PATH="$(dirname "$BRIEFCASE_BIN"):$PATH" \
  "$ARCHCTL_BIN" --root /home/joshsimon/Projects/briefcase \
  gate --baseline-ref 639113f9e57d7a2d7df8dddc078766408281eb95 \
  --briefcase "$BRIEFCASE_BIN" --scope all --run
```

## Next

The first real change should add a reviewed Rust boundary test. Until then, do
not claim structural enforcement.
