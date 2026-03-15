#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

required_root_files=(
  "README.md"
  "LICENSE"
  "CONTRIBUTING.md"
  "SECURITY.md"
  "CODE_OF_CONDUCT.md"
  ".github/CODEOWNERS"
  ".github/dependabot.yml"
  ".github/ISSUE_TEMPLATE/bug_report.yml"
  ".github/ISSUE_TEMPLATE/regression.yml"
  ".github/ISSUE_TEMPLATE/docs.yml"
  ".github/workflows/ci.yml"
  "docs/project/layout.md"
  "docs/verification/crosscheck-gates.md"
  "docs/bringup/gates/latest.json"
  "tools/regression/run_crosschecks.sh"
)

required_crates=(
  "camodel"
  "funcmodel"
  "cosim"
  "isa"
  "elf"
  "runtime"
  "trace"
  "dse"
  "lx-tools"
)

for path in "${required_root_files[@]}"; do
  [[ -e "$ROOT/$path" ]] || {
    echo "missing required path: $path" >&2
    exit 1
  }
done

for crate in "${required_crates[@]}"; do
  [[ -d "$ROOT/crates/$crate" ]] || {
    echo "missing required crate directory: crates/$crate" >&2
    exit 1
  }
done

for old in \
  linxcore-cycle \
  linxcore-func \
  linxcore-cosim \
  linxcore-isa \
  linxcore-elf \
  linxcore-runtime \
  linxcore-trace \
  linxcore-dse
do
  [[ ! -e "$ROOT/crates/$old" ]] || {
    echo "obsolete crate directory still present: crates/$old" >&2
    exit 1
  }
done

[[ ! -e "$ROOT/crates/camodel/src/stages" ]] || {
  echo "obsolete path still present: crates/camodel/src/stages" >&2
  exit 1
}

for dir in core frontend issue backend control decode trace; do
  [[ -d "$ROOT/crates/camodel/src/$dir" ]] || {
    echo "missing camodel owner directory: crates/camodel/src/$dir" >&2
    exit 1
  }
done

for dir in core exec memory syscalls trace; do
  [[ -d "$ROOT/crates/funcmodel/src/$dir" ]] || {
    echo "missing funcmodel owner directory: crates/funcmodel/src/$dir" >&2
    exit 1
  }
done

for dir in linxtrace commit schema; do
  [[ -d "$ROOT/crates/trace/src/$dir" ]] || {
    echo "missing trace owner directory: crates/trace/src/$dir" >&2
    exit 1
  }
done

for dir in protocol compare qemu; do
  [[ -d "$ROOT/crates/cosim/src/$dir" ]] || {
    echo "missing cosim owner directory: crates/cosim/src/$dir" >&2
    exit 1
  }
done

[[ -d "$ROOT/crates/lx-tools/src/cli" ]] || {
  echo "missing lx-tools shared cli directory" >&2
  exit 1
}
[[ -d "$ROOT/crates/lx-tools/src/bin" ]] || {
  echo "missing lx-tools bin directory" >&2
  exit 1
}

if grep -R -n 'name = "linxcore-' "$ROOT/crates" "$ROOT/Cargo.toml" >/dev/null 2>&1; then
  echo "obsolete linxcore-* package name detected" >&2
  exit 1
fi

echo "repo layout OK"
