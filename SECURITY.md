# Security Policy

## Reporting a Vulnerability

Use GitHub’s **Report a vulnerability** feature for this repository whenever
possible. That keeps the initial report private and lets maintainers coordinate
fixes before disclosure.

Include:

- a clear description of the issue and expected impact
- the affected crate(s) or tool(s)
- reproduction steps or a minimal proof of concept
- the commit or branch used for testing

## Supported Versions

Security fixes are applied to the default branch, `main`. Historical branches
may receive fixes only at maintainer discretion.

## Scope Notes

LinxCoreModel is a modeling workspace. Vulnerabilities in this repository may
affect trace tooling, host file access through syscall shims, or analysis tools,
even when they do not affect the RTL or architectural specification directly.
