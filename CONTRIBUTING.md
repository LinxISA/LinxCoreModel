# Contributing to LinxCoreModel

LinxCoreModel is the Rust modeling workspace under `tools/LinxCoreModel` in the
larger `linx-isa` superproject.

## Scope and Ownership

- Keep repository-local changes inside this workspace unless the work explicitly
  requires a superproject integration change.
- Do not reintroduce `linxcore-*` crate names inside this workspace. The crate
  graph is now `camodel`, `funcmodel`, `cosim`, `isa`, `elf`, `runtime`,
  `trace`, `dse`, and `lx-tools`.
- Keep owner boundaries explicit. In particular:
  - `camodel` uses domain/stage modules
  - `funcmodel` uses engine/memory/syscall/trace domains
  - shared CLI logic belongs in `crates/lx-tools/src/cli/`

## Required Local Checks

Run these before opening or updating a pull request:

```bash
bash tools/ci/check_repo_layout.sh
bash tools/regression/run_crosschecks.sh --require-smoke
```

If you do not have a local bring-up ELF under
`out/bringup/linux_user_compiler_smoke_O0.elf`, you can still run the non-smoke
gates:

```bash
bash tools/regression/run_crosschecks.sh
```

## Pull Requests

- Keep changes focused. Do not mix workspace refactors, behavior changes, and
  unrelated superproject edits in one PR.
- Include validation evidence and note whether the smoke/crosscheck gates were
  run with a local ELF or in test-only mode.
- Update docs when you change public crate names, verification commands, or
  owner/module boundaries.

## Superproject Relationship

This repository is intentionally narrower than the `linx-isa` superproject. Use
superproject governance only where it directly applies here:

- mirror reusable patterns like `tools/ci`, `tools/regression`, and
  `docs/bringup/gates`
- do not copy unrelated kernel/compiler/emulator process into this repo
- do not rename architectural references to “LinxCore” or “LinxISA” in the
  wider superproject just because this workspace dropped redundant crate
  prefixes
