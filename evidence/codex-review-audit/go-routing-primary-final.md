# Go routing integration evidence

Reviewed candidate 854aba9e67910214b6e03e4558242f40f0b23251 on main f91c7b1e3f84a6e2e5e002cdc4fe1ad11698d3d3. All 90 candidate files, including 16 source/test files, were copied unchanged. Historical evidence remains unchanged. Independent aggregate review and primary inspection found no remaining actionable defect in the three original routing requirements or contained local-replacement follow-up scope.

The selected Go project carries its OS restriction through compatible-default and fallback routing. Remembered/recommended Mac placement is reconsidered when compatible non-Mac capacity is available; OS-constrained probe failures retain their cause. Required imported local replacements stay reachable across contained module boundaries, with version selection and unpruned requirement edges distinguished from source ownership. This is bounded local inspection, not a full Go dependency solver or general buildability proof.

Final integration checks on a Linux x86_64 build node:

```text
cargo clippy -p prod-code-mcp --lib
warning: prod-code-mcp (lib) generated 103 warnings
Finished dev profile in 17.30s
exit 0 in 18.3s

cargo test -p prod-code-mcp --lib cluster::
test result: ok. 78 passed; 0 failed; 0 ignored; 0 measured; 1376 filtered out; finished in 14.95s
exit 0 in 47.5s
```

This single final compile/test gate includes nested roots, compatible defaults, remembered placement, OS-error context, local replacements, module versions, pruning, escaped import strings and pressure controls. It does not run unrelated tests. The tests use actual temporary manifests/source files and loopback protocol fixtures with simulated OS and engine status; no live macOS compilation, cluster change or deployment is established.

Changed source bytes match retained successful scoped rustfmt evidence, which was reused. Final non-deny Clippy reports 103 existing warnings and none names a changed file. Prior strict lint failures remain recorded; no strict warning-clean claim is made. The source/changelog whitespace check passes. Four extra blank EOF lines in retained historical evidence are deliberately unchanged, so the complete historical-evidence whitespace check is not claimed clean.

The public final hash manifest links every retained candidate file to the checked state. Historical original failed gates, replacement-source inversions and lower-version graph regressions remain in their corresponding evidence directories.
