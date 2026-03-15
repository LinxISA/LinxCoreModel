#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT_DIR="${ROOT}/out/crosschecks"
ELF_PATH="${ROOT}/out/bringup/linux_user_compiler_smoke_O0.elf"
RUN_ID="local-$(date -u +%Y%m%dT%H%M%SZ)"
REQUIRE_SMOKE=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out-dir)
      OUT_DIR="$2"
      shift 2
      ;;
    --elf)
      ELF_PATH="$2"
      shift 2
      ;;
    --run-id)
      RUN_ID="$2"
      shift 2
      ;;
    --require-smoke)
      REQUIRE_SMOKE=1
      shift
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

mkdir -p "$OUT_DIR/logs" "$OUT_DIR/func-smoke" "$OUT_DIR/cycle-smoke" "$OUT_DIR/cosim"

GATE_ROWS=()
overall=0

run_gate() {
  local gate="$1"
  local domain="$2"
  local classification="$3"
  local logfile="$4"
  shift 4
  local -a cmd=("$@")

  echo "== ${gate}"
  printf 'command:'
  printf ' %q' "${cmd[@]}"
  printf '\n'

  set +e
  "${cmd[@]}" >"$logfile" 2>&1
  local rc=$?
  set -e

  local status="pass"
  if [[ $rc -ne 0 ]]; then
    status="fail"
    overall=1
  fi

  GATE_ROWS+=("${gate}|${domain}|${classification}|${status}|${logfile}|$(printf '%q ' "${cmd[@]}")")
  echo "status: ${status}"
  echo "log: ${logfile}"
  echo
}

run_gate \
  "Repository layout" \
  "Repo" \
  "layout_ok" \
  "$OUT_DIR/logs/layout.log" \
  bash "$ROOT/tools/ci/check_repo_layout.sh"

run_gate \
  "Cargo fmt" \
  "Workspace" \
  "fmt_clean" \
  "$OUT_DIR/logs/fmt.log" \
  cargo fmt --all --check

run_gate \
  "camodel tests" \
  "camodel" \
  "cargo_test_pass" \
  "$OUT_DIR/logs/camodel.log" \
  cargo test -q -p camodel

run_gate \
  "funcmodel tests" \
  "funcmodel" \
  "cargo_test_pass" \
  "$OUT_DIR/logs/funcmodel.log" \
  cargo test -q -p funcmodel

run_gate \
  "trace tests" \
  "trace" \
  "cargo_test_pass" \
  "$OUT_DIR/logs/trace.log" \
  cargo test -q -p trace

run_gate \
  "cosim tests" \
  "cosim" \
  "cargo_test_pass" \
  "$OUT_DIR/logs/cosim.log" \
  cargo test -q -p cosim

run_gate \
  "workspace tests" \
  "Workspace" \
  "cargo_test_pass" \
  "$OUT_DIR/logs/workspace.log" \
  cargo test -q

run_gate \
  "func/cycle synthetic crosscheck" \
  "Crosscheck" \
  "func_cycle_match" \
  "$OUT_DIR/logs/crosscheck-unit.log" \
  cargo test -q -p camodel crosscheck_func_and_cycle_engines_on_sample_runtime

SMOKE_PRESENT=0
if [[ -f "$ELF_PATH" ]]; then
  SMOKE_PRESENT=1
  run_gate \
    "functional CLI smoke" \
    "Crosscheck" \
    "func_cli_smoke_pass" \
    "$OUT_DIR/logs/func-smoke.log" \
    cargo run --quiet --bin lx-run -- --engine func --elf "$ELF_PATH" --max-steps 100000 --out-dir "$OUT_DIR/func-smoke"

  run_gate \
    "cycle CLI smoke" \
    "Crosscheck" \
    "cycle_cli_smoke_pass" \
    "$OUT_DIR/logs/cycle-smoke.log" \
    cargo run --quiet --bin lx-run -- --engine cycle --elf "$ELF_PATH" --max-cycles 512 --out-dir "$OUT_DIR/cycle-smoke"

  run_gate \
    "cycle vs func commit crosscheck" \
    "Crosscheck" \
    "cycle_func_cosim_match" \
    "$OUT_DIR/logs/cosim-smoke.log" \
    cargo run --quiet --bin lx-cosim -- --engine cycle --elf "$ELF_PATH" --qemu "$OUT_DIR/func-smoke/commit.jsonl" --out-dir "$OUT_DIR/cosim"
elif [[ "$REQUIRE_SMOKE" == "1" ]]; then
  echo "required smoke ELF missing: $ELF_PATH" >&2
  exit 1
fi

export ROOT OUT_DIR RUN_ID SMOKE_PRESENT
export GATE_ROWS_JOINED
GATE_ROWS_JOINED="$(printf '%s\n' "${GATE_ROWS[@]}")"

python3 - <<'PY'
import json
import os
from datetime import datetime, timezone
from pathlib import Path

root = Path(os.environ["ROOT"])
out_dir = Path(os.environ["OUT_DIR"])
run_id = os.environ["RUN_ID"]
smoke_present = os.environ["SMOKE_PRESENT"] == "1"

def normalize_path_text(text: str) -> str:
    root_prefix = f"{root}/"
    out_prefix = f"{out_dir}/"
    if root_prefix in text:
        text = text.replace(root_prefix, "")
    if out_prefix in text:
        text = text.replace(out_prefix, "out/crosschecks/")
    return text

def load_json_if_present(path: Path):
    if not path.exists():
        return None
    text = path.read_text()
    start = text.find("{")
    if start == -1:
        return None
    return json.loads(text[start:])

gates = []
for raw in os.environ.get("GATE_ROWS_JOINED", "").splitlines():
    gate, domain, classification, status, logfile, command = raw.split("|", 5)
    gates.append(
        {
            "gate": gate,
            "domain": domain,
            "classification": classification,
            "command": normalize_path_text(command.strip()),
            "status": status,
            "required": True,
            "owner": "maintainers",
            "waived": False,
            "evidence_type": "log",
            "evidence": [f"log:{normalize_path_text(logfile)}"],
        }
    )

if not smoke_present:
    for gate, classification in [
        ("functional CLI smoke", "func_cli_smoke_skipped"),
        ("cycle CLI smoke", "cycle_cli_smoke_skipped"),
        ("cycle vs func commit crosscheck", "cycle_func_cosim_skipped"),
    ]:
        gates.append(
            {
                "gate": gate,
                "domain": "Crosscheck",
                "classification": classification,
                "command": "skipped: smoke ELF not present",
                "status": "skip",
                "required": False,
                "owner": "maintainers",
                "waived": False,
                "evidence_type": "terminal",
                "evidence": ["terminal: optional smoke gate skipped because no local ELF was present"],
            }
        )

func_run = load_json_if_present(out_dir / "logs" / "func-smoke.log")
cycle_run = load_json_if_present(out_dir / "logs" / "cycle-smoke.log")
cosim_report = load_json_if_present(out_dir / "logs" / "cosim-smoke.log")

run = {
    "run_id": run_id,
    "generated_at_utc": datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M:%SZ"),
    "profile": "repo-local-crosscheck",
    "lane": "local",
    "trace_schema_version": "1.0",
    "gates": gates,
}

metrics = {}
if isinstance(func_run, dict):
    metrics["func_exit_reason"] = func_run["metrics"]["exit_reason"]
    metrics["func_commits"] = func_run["metrics"]["commits"]
if isinstance(cycle_run, dict):
    metrics["cycle_exit_reason"] = cycle_run["metrics"]["exit_reason"]
    metrics["cycle_commits"] = cycle_run["metrics"]["commits"]
    metrics["cycle_cycles"] = cycle_run["metrics"]["cycles"]
if isinstance(cosim_report, dict):
    metrics["cosim_match"] = cosim_report.get("mismatch") is None
    metrics["cosim_matched_commits"] = cosim_report.get("matched_commits")
if metrics:
    run["metrics"] = metrics

report = {
    "generated_at_utc": run["generated_at_utc"],
    "repo": "LinxISA/LinxCoreModel",
    "runs": [run],
}

(root / "docs" / "bringup" / "gates" / "latest.json").write_text(json.dumps(report, indent=2) + "\n")
(out_dir / "gate-report.json").write_text(json.dumps(report, indent=2) + "\n")
PY

echo "wrote gate report: $ROOT/docs/bringup/gates/latest.json"
echo "wrote gate artifact: $OUT_DIR/gate-report.json"

exit "$overall"
