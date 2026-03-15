## Summary

Describe what this PR changes and why.

## Validation

- [ ] `bash tools/ci/check_repo_layout.sh`
- [ ] `bash tools/regression/run_crosschecks.sh`
- [ ] (Optional local smoke) `bash tools/regression/run_crosschecks.sh --require-smoke`

## Notes

- [ ] Public docs were updated if crate names, verification commands, or repo
      structure changed
- [ ] No old `linxcore-*` crate names were reintroduced
- [ ] Superproject references were only updated where they point directly to
      this workspace
