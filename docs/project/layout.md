# Workspace Layout

LinxCoreModel is the Rust modeling workspace for LinxISA/LinxCore. It is kept
under `tools/LinxCoreModel` in the larger `linx-isa` superproject.

## Crate Map

- `camodel`: cycle-accurate execution model and stage/owner trace generation
- `funcmodel`: functional execution model and Linux-user syscall shims
- `cosim`: commit-stream comparison and M1 lockstep helpers
- `trace`: `linxtrace.v1` and commit JSONL writers
- `runtime`: guest runtime bootstrap and memory/syscall host state
- `elf`: static ELF loading
- `isa`: architectural state, decode, trace schema, and shared types
- `dse`: sweep/report support
- `lx-tools`: `lx-run`, `lx-cosim`, `lx-trace`, `lx-sweep`

## Owner Boundaries

### `camodel`

- `core/`: engine entrypoints, shared state, config, uop model
- `frontend/`: fetch, decode stages, dispatch, checkpoint assignment, restart gating
- `issue/`: IQ residency, qtags, ready tables, `P1/I1/I2`
- `backend/`: execute stages and LSU owner state
- `control/`: `ROB/CMT/FLS`, redirect, traps, dynamic-target recovery
- `decode/`: committed-stream to uop construction and classification helpers
- `trace/`: CA stage-event shaping

### `funcmodel`

- `core/`: engine state and run options
- `exec/`: functional execution loop
- `memory/`: guest memory helpers
- `syscalls/`: Linux-user syscall handling
- `trace/`: functional trace glue

## Naming Contract

This workspace intentionally dropped redundant `linxcore-*` crate names.
Historical names should not be reintroduced in code, manifests, docs, or CI:

- `linxcore-cycle` -> `camodel`
- `linxcore-func` -> `funcmodel`
- `linxcore-cosim` -> `cosim`
- `linxcore-isa` -> `isa`
- `linxcore-elf` -> `elf`
- `linxcore-runtime` -> `runtime`
- `linxcore-trace` -> `trace`
- `linxcore-dse` -> `dse`

## Superproject Relationship

This repo mirrors selected governance patterns from the superproject:

- `.github/` for review and CI policy
- `tools/ci/` for structural checks
- `tools/regression/` for repeatable gate execution
- `docs/bringup/gates/latest.json` for machine-readable gate output

It does **not** inherit unrelated superproject responsibilities like kernel,
compiler, emulator, or RTL release process.
