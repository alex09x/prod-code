# Pull-up literal and shared-consumer integration

Candidate8ecca11cf036d102ce7f9be5519be03b0eefb4d6 was integrated on main66a51a5a83a64b4317243c9e66a97747ef1022cf with all35 candidate files unchanged. Independent aggregate review4088 rejected the preceding candidate only for Swift initializer default forwarding; primary inspection confirms the final three-line filter excludes Swift init from that default set while retaining initializer parsing and explicit selection behavior. The inherited/explicit initializer controls cover omitted and empty method lists.

Primary inspected the aggregate production diff: literal-preserving sibling comparison and indentation, declaration-local modifier stripping, TypeScript decorators and constructor boundaries, C++ opaque boundaries/destructor handling, shared push-down/inheritance consumers and the final Swift filter. The original different-literal sibling remains distinct; ordinary declared methods still move/forward, and unsupported opaque ownership refuses before writes.

One final affected compile/test gate on Linux x86_64:

```sh
cargo test -p prod-code-mcp --test pull_up_literal_preservation
```

```text
test result: ok. 31 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.14s
[prod-code exec] exit 0 in 26.9s
```

The adjacent .txt contains actual sanitized terminal output, including emitted fixture source records. The hash manifest ties every candidate file to this integration state. Existing format evidence was reused because those source/test bytes are unchanged.

Final `cargo clippy -p prod-code-mcp --lib --test pull_up_literal_preservation` exited0:103 existing library warnings plus2 explicit_write warnings in the evidence writers. The latter intentionally write directly to stderr so passed tests retain source/evidence records despite ordinary test capture. Neither strict lint nor warning-free status is claimed; historical strict failures remain unchanged.

The target uses actual available C++, TypeScript/Node and Python compiler/runtime controls. Swift tests inspect returned/applied source using scripted diagnostics; the acceptance node has no Swift compiler. No live Swift compilation or language-server identity proof is claimed. No deployment, restart or broad test suite was performed. Source/changelog whitespace checks passed.
