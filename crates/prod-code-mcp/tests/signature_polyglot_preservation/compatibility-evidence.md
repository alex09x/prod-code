# Compatibility review repair for #1333

This repair starts at `ad9c03041e76cfe721f451e24f940f35f6b7b5cd`, retaining
the earlier repair and its evidence against task base
`b131b3b7655089defb73180a19e13328d25cd867`. No publication, installation,
infrastructure change, delegation or other worktree was used.

## Commands and failure evidence

All compilation and execution ran on a Linux build node through this sanitized
prefix: `prod-code exec --remote <build-node> --timeout-secs 900 --`.
All source blocks below are exact UTF-8 with LF endings and a final LF.
Hashes were emitted by actual `sha256sum` commands in the fixtures.

Before production edits, this command exited 101 (1 passed, 3 failed):

```
cargo test -p prod-code-mcp --test signature_polyglot_preservation compatibility:: -- --nocapture
```

The three failures were unnecessary no-write refusals: the two async fixtures
reported `has an unresolved await operand`; the C++ float reported
`has argument side effects that cannot safely survive this signature change`.
All original/generated programs compiled or executed successfully, and refused
outputs had identical bytes and hashes. The user-defined literal control passed.

After the tokenizer and float fixes, the same command exited 101 (3 passed,
1 failed). The added dollar-method fixture exposed declaration name extraction
rejecting `$fetch`, independently of operand analysis. An initial single-line
class fixture reported `could not locate declaration for function at line 1`.
Correcting it to an independently positioned method on line 2 still reproduced
`could not locate declaration for function at line 2`, with this command (exit 101):

```
cargo test -p prod-code-mcp --test signature_polyglot_preservation compatibility::dollar_identifiers_preserve_both_async_directions -- --nocapture
```

After allowing `$` in JavaScript/TypeScript method name extraction, that exact
command exited 0 (1 passed, 21 filtered). It executed both free-function async
directions and method async removal, in both JavaScript and TypeScript files.
The final two-target gate is recorded below after completion.

The semantic MCP calls `code_outline` with paths
`crates/prod-code-mcp/src/signature_polyglot/await_operand.rs`,
`crates/prod-code-mcp/src/signature_polyglot/numeric_literal.rs`, and
`crates/prod-code-mcp/tests/signature_polyglot_preservation/remaining.rs` all
returned `MCP tool call requires approval, but approval policy is never`.
Fallback `prod-code outline` and `prod-code def ... --symbol ...` succeeded.
A literal log search `rg -n -C 14 "panicked|unresolved|assertion|left:|right:"
<scratch-log>` was blocked by the hook with `Recursive grep/find outside the
workspace is always blocked`. A bounded Python read of that one log retrieved
the complete diagnostics. These tool-policy failures are not test passes.

## Default export regex

Original `main.js`, selected declaration 2:16, reference 3:43:

```js
export default /[)]/;
async function f(x) { return x; }
async function main() { console.log(await f( 1 )); }
main();
```

`package.json` is exactly `{"type":"module"}` followed by LF.
Request Keep(x), async=false, apply=true, force=false. Original hash
`0dc673c61b31e27d05a9e2915a00807d65cbecbce229db8fb2e8be9a640af8cc`.
Before repair generated source/hash were unchanged. After repair:

```js
export default /[)]/;
function f(x) { return x; }
async function main() { console.log(f( 1 )); }
main();
```

Hash `1a5e414660a2ed81d54527e076ddde245ebcb5382fb9df901ff5221f932b7dc2`.
Enabling async from those exact synchronous bytes selects 2:10, reference 3:37,
and generates:

```js
export default /[)]/;
async function f(x) { return x; }
async function main() { console.log((await f( 1 ))); }
main();
```

Hash `3bed0d67b1a285e1c365ce6941187f9666c3a03385e257e6ee0986091be22a7f`.
Every `node main.js` exited 0, stdout `1\n`. Exact-source assertions protect
the unrelated regex and unchanged argument spaces.

## Dollar identifiers

Original `main.js` (also exact `main.ts` bytes), declaration 1:16, reference 4:37:

```js
async function $fetch(x) { return x; }
async function outer(x) { return x; }
async function main() {
    console.log(await outer(await (($fetch( 1 )))));
}
main();
```

Original hash `aeaf31b3b4ca9071f846543aa7dd9e07b50e2d2bc753014a0e57ff3cdb78827f`.
Request Keep(x), async=false, apply=true, force=false. Before production edits,
generated source/hash were unchanged. Successful output:

```js
function $fetch(x) { return x; }
async function outer(x) { return x; }
async function main() {
    console.log(await outer((($fetch( 1 )))));
}
main();
```

Hash `a8c1dc32527f45e5d31c91a1ebb8df0ede717e9aa07db36a543335a9ce88ea87`.
Enabling async from those bytes, declaration 1:10, reference 4:31, generates:

```js
async function $fetch(x) { return x; }
async function outer(x) { return x; }
async function main() {
    console.log(await outer((((await $fetch( 1 ))))));
}
main();
```

Hash `c6bccec5754a05ea684bf62781b36e58646fd68d8c777d8c6abf1fa79b2624da`.
Every `node -e SOURCE` exited 0, stdout `1\n`. The outer callee's await and
the argument bytes are asserted unchanged.

Method original (both file extensions), declaration 2:11, reference 7:39:

```js
class C {
    async $fetch(x) { return x; }
}
const c = new C();
async function outer(x) { return x; }
async function main() {
    console.log(await outer(await ((c.$fetch( 1 )))));
}
main();
```

Hash `d1461a91a8f5a5540325d213ac671bfbaa2088dc5dd1a5bb3f6e087168b4bf1d`.
Successful output:

```js
class C {
    $fetch(x) { return x; }
}
const c = new C();
async function outer(x) { return x; }
async function main() {
    console.log(await outer(((c.$fetch( 1 )))));
}
main();
```

It removes only the first `async ` and the `await ` immediately after `outer(`,
preserving every other byte. Generated hash
`8601830be4aeee75b03125574059a065ddb8d4c6dbf701c0617313da0fba1efd`.
All original/generated Node executions exited 0 with stdout `1\n`.

## C/C++ hexadecimal floats

Original `main.cpp` and `main.c`, declaration 2:8, reference 3:31:

```c
#include <stdio.h>
double f(double a, double b) { return a * 10 + b; }
int main() { printf("%.1f\n", f(0x1.8p+2, 2.0)); }
```

Hash `85d24a39f822cf703b60e1d99c2c58ef1f1ed00602c5fbb9afaa04cb90292ed5`.
Request Keep(b), Keep(a), apply=true, force=false. Before production edits,
generated source/hash were unchanged. Successful output:

```c
#include <stdio.h>
double f(double b, double a) { return a * 10 + b; }
int main() { printf("%.1f\n", f(2.0, 0x1.8p+2)); }
```

Hash `f899b0deebf9c88777ca70d7c5a8352bd143237c05abd55895fdfc1aefbdf9ed`.
All `c++ -std=c++17 main.cpp -o program`, `cc -std=c11 main.c -o program`
and `./program` executions exited 0; stdout was `62.0\n` before and after.
The same complete source transformations with each replacement literal below
also compiled and ran identically in C and C++:

| Literal | Original SHA256 | Generated SHA256 | stdout |
| --- | --- | --- | --- |
| `0X1P-1` | `f99efbfa9cd1f678fda6c40534586163042ee7f9566d8c06ba80c0680caf646c` | `b878950c30968b01dea699e9d5b2f67a57b715d835939527f4355e16f165d9ae` | `7.0\n` |
| `0x.8p2f` | `b7aab0bae8f531a1467527883f79fada1425c638e863c59656e378449166b644` | `5ae8241be2845b420ca42c713f29bde3af80f291e4e76e8df5e23402ef2c1769` | `22.0\n` |
| `-0x1.p0L` | `9580b8c0c56dfedce63df5f311703905dad5d3e3dcaeb257187d8c72c9cad03a` | `35355c59ee1a93cd76475ca5575b0a9c34f6027ca164d5b59aaec763ee453cc1` | `-8.0\n` |

Effectful C++ control, declaration 3:8, reference 4:31:

```cpp
#include <cstdio>
long double operator "" _effect(long double x) { puts("effect"); return x; }
double f(double a, double b) { return a; }
int main() { printf("%.1f\n", f(1.0, 0x1.8p+2_effect)); }
```

Keep(a) refuses with `side effects` for both force=false and force=true.
Original and generated hash both
`c5dcef1eada3b1177158304759680298c6813e9e6cbea584e060522f8eaa75c2`.
Every C++ compile and program run exited 0; stdout `effect\n1.0\n`.
The complete-token pattern retains the mandatory p/P exponent and built-in
suffix whitelist; it does not infer purity from a numeric prefix.

## Evidence boundaries

New fixtures invoke the public `signature::change_with` API with exact selected
name-token references. Reference responses and overlay diagnostics are scripted;
they do not establish live Node/TypeScript analyzer behavior. Node executes the
exact TypeScript fixture bytes as a JavaScript subset, without a TypeScript
compiler claim. Runtime/compiler checks are real and required, independent of
empty diagnostics. The retained identity fixture separately captures real
clangd references/declarations. No new runtime behavior is inferred from hashes.

## Final validation

Scoped formatting, exit 0, using the remote prefix above:

```
rustfmt --edition 2024 crates/prod-code-mcp/src/signature_polyglot/await_operand.rs crates/prod-code-mcp/src/signature_polyglot/numeric_literal.rs crates/prod-code-mcp/src/signature_polyglot/syntax.rs crates/prod-code-mcp/tests/signature_polyglot_preservation.rs crates/prod-code-mcp/tests/change_signature_polyglot_tests.rs
```

Scoped lint attempt, exit 101:

```
cargo clippy -p prod-code-mcp --test signature_polyglot_preservation --test change_signature_polyglot_tests -- -D warnings
```

Clippy reported `could not compile prod-code-mcp (lib) due to 103 previous
errors`. All 103 diagnostics were outside the changed polyglot scope, including
`config/mod.rs:52` (`collapsible_if`), `signature/call_sites.rs:111`
(`result_unit_err`) and `to_method/helpers.rs:186` (`needless_lifetimes`). The
complete diagnostic set was inspected; no unrelated code was changed and no
lint rerun suppressed those errors. This is a failed check, not clean lint.

The required final gate, after formatting, exited 0:

```
cargo test -p prod-code-mcp --test signature_polyglot_preservation --test change_signature_polyglot_tests
```

`change_signature_polyglot_tests`: 12 passed, 0 failed.
`signature_polyglot_preservation`: 22 passed, 0 failed.
This includes real Node/Python execution, C/C++ compilation and program execution,
real clangd identity evidence, existing analyzer-error and forced-unmatched
refusals, scalar compatibility, and default-free C++ added parameters.
The final compiler/test gate was run once. `git diff --check` also exited 0.
Changed production source sizes are 298, 101 and 318 lines; the new regression
module has 218 lines. Source attribution headers are preserved throughout.
