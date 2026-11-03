# Hosted CI and native releases

GitHub Actions uses standard hosted runners. No job connects to the development
cluster. Signing credentials belong only to the protected `release-signing`
environment; PR jobs have a read-only token and no signing credentials.

## Pull requests and main

`CI` runs on pull requests, pushes to main and manual dispatch. It checks Rust
formatting, runs workspace Clippy and existing Rust unit, binary, integration and
documentation tests. The integration targets are split into bounded shards to
fit standard runner disks. Each target runs once; existing ignored/live tests
remain ignored and are visible in Cargo's summaries. This does not claim GUI,
LAN or opt-in real-language-server acceptance.

Runtime shards provision the pinned language-server tools used by the existing
tests (gopls, TypeScript server, basedpyright and Ruff), plus clangd, ripgrep,
sccache and the selected Rust toolchain's analyzer, rustfmt, Clippy and
standard-library sources. A missing required tool is
an error, not a skipped passing test. Each shard continues after a failed target
and reports all its failed targets at the end.
The overlay lifecycle tests also require unprivileged user namespaces. Their
Ubuntu24.04 restriction is disabled only in disposable, unprivileged CI VMs;
the provisioning script refuses local or self-hosted execution. No signing
secret is present in those jobs, and no setting is changed on a production node.

Clippy warnings are reported without `-D warnings`: the repository has existing
warnings and this migration does not conceal them or call strict lint clean.
The final `ci` job requires every planned job to succeed. Source changes still
follow the development validation policy; hosted checks run on the complete PR.

A separate standard macOS ARM64 job runs the client update/package trust tests.
It verifies the checksums and Developer ID team of one current public release
and installs it only as a disposable test fixture. Required negative tests must
execute and pass; an empty test filter cannot satisfy the job. This job has no
signing/notarization secrets and does not sign a release.

Only Cargo registry/git downloads are cached, not build outputs. Test debug
information and incremental compilation are disabled on hosted runners to bound
disk usage. Runtime Linux shards override development/test optimization to level
1 in their disposable Cargo configuration, including analyzer dependencies and
source-built gateway children. Interactive query deadlines remain unchanged.
The gateway `live` target runs its process-heavy fixtures sequentially to bound
memory on the standard VM; concurrency exercised inside each fixture is retained.
No paid larger runner is selected. Release artifacts expire after
one day; GitHub's account-level storage allowances still apply.

## Release credentials

Configure an environment named `release-signing`, limited to main and release
tags, with the repository owner as a required reviewer. Restrict who can edit
and approve privileged workflow code. Environment Secrets are:

| Name | Value |
| --- | --- |
| `APPLE_CERTIFICATE_P12_BASE64` | Base64 of only the Developer ID Application identity, exported as encrypted PKCS#12 |
| `APPLE_CERTIFICATE_PASSWORD` | Random PKCS#12 wrapping password |
| `APPLE_SIGNING_IDENTITY` | Developer ID Application identity name or certificate fingerprint |
| `APPLE_NOTARY_KEY_P8` | Managed App Store Connect API private key permitted to notarize |
| `APPLE_NOTARY_KEY_ID` | API key identifier |
| `APPLE_NOTARY_ISSUER_ID` | Team issuer identifier |

Secret values never belong in a workflow, issue, artifact or command log. Import
only the required identity into an ephemeral Keychain on a separate signing VM.
Native source/build scripts run in unprivileged jobs before the signing job,
without access to this environment. Temporary certificate/key files and the
Keychain are removed on exit; GitHub destroys the hosted VM after the job.
Secret storage is not a defense against malicious privileged workflow code.

## Tag releases and dry runs

After the version/changelog PR is merged and `CI` is green for that exact main
commit, push its annotated `vMAJOR.MINOR.PATCH` tag. `Native release` validates
the tag version and main ancestry, requires successful main CI, and builds both
client and gateway natively on Linux x86_64, Linux ARM64 and macOS ARM64. Linux
builds use Ubuntu22.04 and the existing GLIBC2.35 gate.

The signing job requires environment approval. It signs both Mac executables
with Developer ID, hardened runtime and a secure timestamp. Apple notarization
must return `Accepted`; missing secrets, failed signing or notarization stop
the release. There is no unsigned/ad-hoc fallback. Raw CLI executables receive
online notarization tickets; tickets cannot be stapled to standalone binaries.
This workflow does not promise offline Gatekeeper checking or generate a DMG/PKG.

The collector accepts only the six expected binaries, checks their notarization
receipt and hashes, generates `SHA256SUMS`, creates a draft and verifies all seven
downloaded assets before publication. Existing releases are never overwritten.
Only the nine newest stable release objects are retained; Git tags are preserved.
No cluster deployment or restart is performed.

To test without publication:

```sh
gh workflow run release.yml --ref main -f tag=v0.3.51 -F publish=false
```

Manual dispatch is allowed only from main. A dry run may use a historical tag,
does not require a historical hosted CI result and never changes its release.
The three platform artifact archives, signing receipt and checksum artifact
remain available for one day. An accepted notarization or a green workflow must
be observed in the real run before claiming the pipeline is operational.
