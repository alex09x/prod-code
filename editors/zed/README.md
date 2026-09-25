# prod-code for Zed

The language servers run on your build nodes, and Zed talks to them through `prod-code lsp`.
For each language that is the language's own server, started for your editor session in the
node's copy of the checkout:

| Zed server | on the node | language |
|---|---|---|
| `prod-code-rust` | rust-analyzer | Rust |
| `prod-code-go` | gopls | Go |
| `prod-code-cpp` | clangd | C, C++ |
| `prod-code-python` | basedpyright | Python |
| `prod-code-typescript` | the TypeScript server | TypeScript, TSX, JavaScript |
| `prod-code-swift` | sourcekit-lsp (a macOS node) | Swift |

Everything the server speaks reaches Zed, in both directions:

- completion, diagnostics, code actions and inlay hints;
- rust-analyzer's check on save (`cargo check` runs on the node) and its protocol extensions;
- the settings you give the server.

A save reaches the node before the server hears of it, so the check sees what you saved.
Definitions in the standard library, a dependency or a generated file are copied from the node
into a read-only mirror, `~/Library/Caches/prod-code/remote/<node>/`, and open from there.

## Install

1. Install the `prod-code` client (see the main README) and configure your nodes. `prod-code
   cluster` in a checkout should show where it is placed.
2. In Zed, run `zed: install dev extension` and choose this directory (`editors/zed`). Zed
   builds it with the Rust toolchain installed through rustup.
3. Choose the servers per language in `settings.json`:

```json
{
  "languages": {
    "Rust": { "language_servers": ["prod-code-rust", "!rust-analyzer", "..."] },
    "Go": { "language_servers": ["prod-code-go", "!gopls", "..."] },
    "C++": { "language_servers": ["prod-code-cpp", "!clangd", "..."] },
    "Python": { "language_servers": ["prod-code-python", "!basedpyright", "!pyright", "..."] },
    "TypeScript": { "language_servers": ["prod-code-typescript", "!vtsls", "!typescript-language-server", "..."] }
  }
}
```

A `.zed/settings.json` in a project does the same for that project only.

## Exactly Zed's own Rust support, remotely

Zed wires some of its Rust features to the server named `rust-analyzer`: expand macro, open
docs, the check-on-save commands and the rendering of completion labels. To keep all of them,
leave Rust on Zed's rust-analyzer and point it at prod-code instead of the local binary:

```json
{
  "lsp": {
    "rust-analyzer": {
      "binary": {
        "path": "/Users/<you>/.cargo/bin/prod-code",
        "arguments": ["lsp", "--language", "rust"]
      }
    }
  }
}
```

Zed then talks to the rust-analyzer on the node as if it were local. Your
`lsp.rust-analyzer.initialization_options` and `settings` still apply, and this needs no
extension.

## Settings

- `lsp.<server>.binary.path`, `.arguments`, `.env`: how the extension starts the server. The
  default is `prod-code lsp --language <language>`, found on the worktree's `PATH`.
- `lsp.<server>.initialization_options` and `.settings` go to the server. When a prod-code server
  has none of its own, it takes those of the server it stands in for: `lsp.rust-analyzer.*`,
  `lsp.gopls.*`, `lsp.clangd.*`, `lsp.basedpyright.*`, `lsp.vtsls.*`, `lsp.sourcekit-lsp.*`.

## When something is off

- `PROD_CODE_LSP_TRACE=/tmp/lsp.log`, in `lsp.<server>.binary.env`, logs every message the
  bridge carries, with its method, id, size and time.
- `zed: open language server logs` shows the server's own log.
- Report a bug in prod-code with `prod-code report-issue`.
