---
name: Bug report
about: Something behaves wrongly or slower than it should
labels: bug
---

## Reproduce

The exact command(s), the workspace shape (language, size, worktree or main checkout),
and the output as it is today:

```sh
$ prod-code ...
```

## Expected

What the output should have been.

## Evidence

Gateway log lines (`journalctl --user -u prod-code-gateway -o short-precise`),
`PROD_CODE_TIMING=1` phases, benchmark output, or a minimal repository that reproduces it.
