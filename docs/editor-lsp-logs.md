# Editor LSP session logs

`scripts/zed-lsp-logger.py` launches `prod-code lsp` with private, rotating logs.
It inherits stdin/stdout directly and forwards termination signals to the bridge;
it does not parse, buffer or rewrite the editor's JSON-RPC stream.

Install an executable copy of the script in a stable user location and configure
the editor to run it with `--language rust` (or `go`, `cpp`, `python`, `typescript`,
`swift`). The default bridge executable is `~/.local/bin/prod-code`; `--binary`
selects another installed executable. The launcher requires Python 3.8 or newer
and POSIX named pipes. Restart the language server once after changing its binary
configuration; this does not require closing editor buffers.

For example, the Zed language server's `binary` settings can be:

```json
{
  "path": "/absolute/path/to/zed-lsp-logger.py",
  "arguments": ["--language", "rust"],
  "env": { "PROD_CODE_REMOTE": "auto" }
}
```

Logs default to `~/Library/Logs/prod-code/zed/`, one `session-*` directory per
launcher process. `--log-dir` chooses another location.

- `session.json`: language, working directory, wrapper/bridge PIDs, start/end
  times and exit code. Version and inode describe the installed bridge at launch;
  they do not attest every later hot reload.
- `transport.log`: existing `PROD_CODE_LSP_TRACE` metadata, including request
  method/ID/size and reconnect/hot-reload stages. Document payloads are not logged.
- `stderr.log`: bridge stderr, also forwarded to the editor as before. This can
  contain private paths, diagnostics and source snippets: do not publish it raw.

Each stream rotates at approximately 2 MiB with three backup files; the largest
individual record is bounded to a 4 KiB input chunk. At launch, only the twenty
newest completed session directories are retained, plus every live session.
Files are owner-only and new directories are private. Never use a shared/public
log directory. No credentials or arbitrary environment variables are collected.

After a failure, preserve its session directory and the editor's own log before
restarting. Correlate timestamps, `bridge_exited`, reconnect stages and the gateway
service log. An initial handshake refusal, an analyzer dependency/indexing error,
and a lost gateway connection are different failures. Send private evidence through
your private incident record and use `private_ref` in a public issue.

The launcher captures evidence; it does not repair reconnect or analyzer failures.
