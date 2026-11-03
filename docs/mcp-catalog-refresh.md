# MCP catalog discovery after an update

The installed executable, the running MCP server and the host's exposed tool catalog
are separate states. A skill describing `code_workflow` does not add that tool to a
host's API, and tool approval settings do not refresh discovery.

## Verify the boundary

1. Check the executable path configured for the host's `prod-code` MCP server, then
   run that executable with `--version`. A different `prod-code` on the shell's `PATH`
   does not establish the version of the running server.
2. Inspect the tools actually exposed by the host. Search its available-tool catalog
   for `code_workflow`; do not infer availability from a skill or a version string.
3. When the host provides an MCP inspector, request `tools/list` from the configured
   server. The request is ordinary MCP JSON-RPC:

   ```json
   {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
   ```

   An MCP client must initialize the connection first. In v0.3.49 the server's
   initialize result advertises `capabilities.tools.listChanged: true`, and its
   `tools/list` includes `code_workflow`. If the server lists the tool but the host
   does not expose it, the remaining problem is host catalog refresh. If the server
   omits it, check its actual executable and version before changing host permissions.

## Refresh or recover

Use the host's documented MCP refresh or reconnect operation if it provides one.
There is no universal MCP command that forces a host to rebuild its API catalog,
and no `prod-code` CLI command for refreshing another process's tool list.

When the current host has no supported refresh surface, finish its active work and
start a new host session using the existing MCP configuration. Fresh Claude Code,
Codex CLI and Antigravity sessions were each verified to expose and successfully
call `code_workflow` with v0.3.49. This does not prove that an older session refreshed.
Do not terminate active jobs or restart gateways to refresh an agent's catalog.

Until the tool appears, use the available individual tools such as
`code_definition`, `code_references`, `code_slice` and `code_exec`. The v0.3.49
workflow interface is MCP-only; `prod-code workflow` is not a CLI fallback.

If discovery remains wrong, report the host and version, configured executable
version, whether server `tools/list` contains the tool, and whether a fresh session
exposes it. Remove private paths, addresses and configuration secrets before filing.
