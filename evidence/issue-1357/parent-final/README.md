# Combined-main Rust method verification

The reviewed candidate `d8dec7f3fd958be8fa26841dc3a0de8cc305c1c3` was combined with main `ca8df11699ef7a05bfe6fc1339ea55c4badf055f`. The branches had no overlapping changed paths. All 41 changed crate files remain byte-identical to the independently reviewed candidate; see `source-hashes.json` and `aggregate-review.json`.

The final Linux x86_64 verification used scoped rustfmt checks on all 41 crate files, then non-denying Clippy for the MCP library/preservation target, gateway library/native target and Rust engine library. All exited zero. Warnings remain: 103 MCP library, 56 gateway library, one Rust-engine library and one native-test warning, plus existing Go/generic dependency warnings. This is not a strict-warning or full-workspace lint claim.

One final gate ran `python3 evidence/issue-1357/run-final.py --output-dir evidence/issue-1357/parent-final/gate` through the remote execution CLI. It completed with exit zero in 556.559 seconds client wall time:

- `cargo test -p prod-code-mcp --test rust_method_preservation`: 55 passed, zero failed/ignored/filtered; test time 8.33 seconds.
- `cargo test -p prod-code-gateway --test rust_method_native`: 8 passed, zero failed/ignored/filtered; test time 464.68 seconds.

The actual gate retains 116 source hashes and 98 artifact hashes, all verified against the returned bytes. All preserved inputs match. Independent inspection of the final semantic records confirms ten distinct validation-call identities. Eighteen original/returned programs have actual compiler and runtime exit zero; ten returned-program runtime records equal their corresponding originals. `native-proof-audit.json` records the checks. Semantic answers came from the candidate native in-process gateway/engine path, not scripted replies; the separate preservation target uses scripted semantic fixtures and real compiler/runtime checks.

Full terminal logs and command results are retained here. Build endpoint, developer-home and checkout prefixes in parent terminal logs are redacted; original hashes and published hashes are recorded in `terminal-redactions.json`. One historical native log also contained six home prefixes; `privacy-redactions.json` documents that redaction and the one corresponding published artifact-manifest hash update. Private originals remain preserved. No diagnostic, exit or test count was removed.

Historical failed/interrupted runs remain in their original evidence directories. Their results are not converted to passes. Whitespace warnings in historical raw terminal transcripts are retained deliberately; the source-only staged diff check passed. No new full coverage measurement, deployment, installed-daemon attestation, arbitrary macro-hygiene proof or all-platform verification is claimed.
