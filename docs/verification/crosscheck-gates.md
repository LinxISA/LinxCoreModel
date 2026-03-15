# Crosscheck Gates

The canonical repo-local gate entrypoint is:

```bash
bash tools/regression/run_crosschecks.sh
```

## What It Runs

Required gates:

- `bash tools/ci/check_repo_layout.sh`
- `cargo fmt --all --check`
- `cargo test -q -p camodel`
- `cargo test -q -p funcmodel`
- `cargo test -q -p trace`
- `cargo test -q -p cosim`
- `cargo test -q`
- `cargo test -q -p camodel crosscheck_func_and_cycle_engines_on_sample_runtime`

Optional smoke gates:

- `lx-run --engine func` on a locally available bring-up ELF
- `lx-run --engine cycle` on the same ELF
- `lx-cosim --engine cycle --qemu <func commit.jsonl>` to crosscheck cycle
  against the functional commit stream

By default the smoke gates run only if a local ELF exists at
`out/bringup/linux_user_compiler_smoke_O0.elf`.

To require those smoke gates, use:

```bash
bash tools/regression/run_crosschecks.sh --require-smoke
```

## Gate Report

The latest machine-readable report is written to
`docs/bringup/gates/latest.json`.

This mirrors the superproject convention of keeping a JSON gate summary beside
human-readable docs, while keeping the checks repo-local and model-specific.
