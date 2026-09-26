# Changelog

## Unreleased

### Fixed
- **`prod-code lsp` fails loudly when the gateway goes away** (#394). The bridge noticed only on
  the editor's next message, answered nothing until then, and exited 0, which an editor reads as a
  clean shutdown and does not restart. It now says on stderr which gateway closed or broke the
  connection and exits 1 at once, so the editor starts it again with a fresh session. An editor
  that closes its side first still gets exit 0.

## v0.3.15 — 2026-09-25

### Changed
- **Index questions wait until the language server has indexed** (#391). The engines declare
  `window.workDoneProgress` and follow the work each server begins and ends (`$/progress`); for
  basedpyright and pyright, which report none, the `Found N source files` log line. The index
  questions (`workspace/symbol`, `references`, `implementation`, `rename`, call and type
  hierarchy) wait for that, up to 30 s; an answer still given while the server indexes comes
  with a note under it, in the form "(the language server was still indexing (n/m, p%) for 30 s
  when asked: this answer may be incomplete)", never silently partial. The handshake says when the
  gateway gates an engine (`index_gated`), and the client then takes an empty answer as final
  instead of the #381 guess from the engine's age. `refs --symbol func0` right after first
  contact with a C++ checkout of 900 files that all call it: 0, then 315, then 715 references
  (asked 5 s apart) before; 899 on the first question after a 14.3 s wait for clangd's index,
  then 899 in 0.3 s.

### Fixed
- **A request from the server taken for the answer to a question** (#391). A server numbers its
  own requests from 1, as the client does: gopls's `window/workDoneProgress/create` and
  `workspace/configuration` were matched to the engine's pending questions, and, passed on by the
  gateway, to the client's, which then read an empty answer. Only a message without a method is
  an answer now, in the engines, the client and the CLI, and the gateway no longer passes a
  server's own requests on.
- **A workspace copy named like a hidden directory** (#391). A checkout whose directory name
  begins with `.` or `_` got a copy of that name, which Go tools skip: gopls found the module's
  symbols in no package. Such a copy is now named `dot-…` or `under-…`.

## v0.3.14 — 2026-09-25

### Added
- **References across checkouts** (#375). `code_references` takes `also_in` (other checkouts'
  directories) with `symbol`, and `prod-code refs --symbol NAME --in DIR` (repeatable): the name
  is resolved in each checkout, each is asked on the node its own workspace is placed on, and
  every answer comes under its checkout's directory with a total; one that fails says why
  without hiding the others. A field of a shared dependency's type used by two services: 107 + 93
  = 200 references in 3.1–7.1 s in one call, where it took two calls from two directories.
- **`prod-code validate --compile`** (#376), what `compile: true` is for the MCP tools (#364):
  the project's check command runs on the proposed text in a shadow copy on the node, with
  `--with` and `--diff` too. A test calling another crate's private function validated as clean;
  with `--compile`, 3 × E0603 in 10.4 s. The `compile` description now names that case.

### Fixed
- **Worktree copies no longer fill a build node's disk** (#386). The gateway deleted a
  `<repo>--wt-*` copy only after 7 idle days; 47 copies (the largest 21 GB, at least 350 GB of the
  446 GB of workspace storage) filled a 913 GB disk in two days, none older than 5.3 days. Below `PROD_CODE_PRUNE_BELOW_FREE_PERCENT` free (15 by
  default, 0 disables) the janitor now deletes idle copies that are not loaded, oldest first,
  however young (never one used within the hour), until the share is back, logging each with its
  idle time and the free space before and after.
- **A full disk no longer empties the files a sync writes** (#385). The gateway wrote a synced
  file with `fs::write`, which truncates first, and ignored a failure: on a node whose disk had
  filled, 22 files of one checkout were left 0 bytes while the sync reported success and the
  client's watermark counted them delivered. A synced file now goes to a temporary file next to it
  and is renamed over it, so a failed write keeps the old text; the path is not recorded as synced,
  the engine is not told the new text, and the path comes back in `stale_paths` (and at every
  handshake) until the client has sent it again.
- **A name declared in a file the analyzer does not load** (#379) was "no symbol named ... in
  the workspace index", the answer for a name that exists nowhere. When the index has no symbol
  by that name, the checkout's source files are searched for its declaration, and the answer
  names it and why the analyzer has nothing there: no hover at it (no target includes the
  file; for Rust, no `mod` chain from a crate root) or an item the index does not list (inside
  a function body). In a Rust checkout whose `src/utils2/` only disabled binaries declare:
  `` `AnomalyDetector` is declared at src/utils2/data_quality.rs:70:12 ``.
- **References of a dependency's item asked at its declaration** (#373). `refs --symbol` lands on
  the declaration inside the dependency, where the server finds nothing to search for; an empty
  answer there is now asked again from a use in the checkout whose definition is that declaration,
  and says so. `refs --symbol DataBTC7` in a Rust checkout: "No references found" before, 40
  references in 1.1 s after. A file no target compiles costs two of the search's 40 questions.
- **A position on no name is an error** (#373) that shows the line, instead of "No references
  found"; an empty answer at a name says which name and line it was. `prod-code refs FILE LINE COL`
  goes through the same code as `code_references`.

### Changed
- **A symbol lookup that finds nothing answers in a fraction of the time** (#381). Every empty
  `workspace/symbol` answer was followed by an 800 ms pause and a retry, in the checkout's project
  and in each nested project a miss goes on to ask. The handshake now carries the engine's age
  (`engine_age_ms`): a warm engine's empty answer is final, one loaded less than 30 s ago is
  asked again up to three times (0.8, 1.6, 3.2 s) while it indexes, and a gateway that does not
  say keeps the single retry. A miss in a checkout with three nested projects, against the same
  warm gateway: 3.52–3.58 s before, 0.28–0.30 s after.
- **`impact` runs Go tests in their packages** (#371): `go test ./internal/push -run ...` instead
  of `go test ./... -run ...`, which built every package's tests to run a filter most of them never
  match. One package of a large module: 0.7 s instead of 24.8 s; `impact --run` 2.1 s in all.

## v0.3.13 — 2026-09-25

### Added
- **A package's outline that fits** (#368). `code_outline` / `prod-code outline` take `kinds`
  (`--kinds function,struct`), `exported_only` (`--exported`: Go's capitalised names, a method
  only on an exported type; Rust's `pub`; Swift's `public`/`open`; TypeScript's `export`; Python
  without a leading underscore; test files left out), `max_bytes` and `max_items`. A directory's
  outline has a 40,000-byte budget by default (`0` for none), counts the symbols it lists, and when
  it stops names the files it did not reach. A directory with no source files of its own lists its
  subdirectories that have some. One Go package went from 79 KB and 1,667 symbols to 40 KB by
  default, or 6.9 KB and 141 symbols for its exported functions, types and methods.

## v0.3.12 — 2026-09-25

### Fixed
- **Swift in a repository rooted in another language** (#358).
  - A name search asks first the nested projects whose sources name the symbol, and walks the
    checkout the way git sees it. In a Go module with nested Swift packages, the six projects a
    search asked were a C++ dependency's build output and loose scripts, so a Swift class was
    "no symbol named ...".
  - A search given a nested project's path reads the outlines of its files that name the symbol
    when the server has no index (sourcekit-lsp before a build).
  - An outline lists members (methods, properties) of servers that answer with nested symbols
    (sourcekit-lsp, clangd, pyright, the TypeScript server), not only the top-level types.
  - A Swift `extension` of a type no longer makes the type's name ambiguous.
  - A Swift reference or caller search that finds nothing builds the package's index on its node
    once (`swift build --build-tests`) and asks again, and says so; when the build fails, the
    answer says why instead of a bare "No references found".
- **An MCP session sees a file another client pushed** (#360). A long-lived session sent a
  changed file to the server only when its own sync pushed it; after the CLI, an `exec` whose
  formatter's output came back, or another agent had already pushed it, definitions and hovers
  came from the text the session first opened. Every open file whose text changed is sent now.
- **Outlines of scripts, Markdown and files no server serves** (#362).
  - A file of another language inside a nested project (a Python script in a Swift package) is
    served by its own language's engine; it went to the project's server, which waited 45 s
    and answered nothing. The C family inside a Swift package stays with sourcekit-lsp.
  - A Markdown file's outline lists its headings.
  - A file no language server serves (shell, protobuf, TOML, ...) gets an error that says so,
    from the MCP tool and the CLI; the CLI printed `null`.
  - `.m` and `.mm` count as C-family sources.

### Added
- **`compile: true` for `code_validate_edit` and `code_validate_edits`** (#364). The project's
  check command (`cargo check` for Rust, `go build`, `tsc`, ...) runs on the proposed text in a
  shadow copy on the node, and its errors are reported with the analyzer's. rust-analyzer runs
  no borrow checker, so a reference to a local (E0515) or a use after a move (E0382) validated
  as clean; the tools' descriptions now say so.

## v0.3.11 — 2026-09-25

### Fixed
- **A file of another checkout is asked about in that checkout's session** (#353). An agent
  whose tools run in one checkout (a TypeScript site) and read a Rust file of a clone next to it
  got "Language server process has exited": the query went to the site's TypeScript server,
  which crashed on the Rust file. The MCP server now opens a session in the file's own
  checkout, as the command line already did.
- **A language server that crashed is started again** (#355). When the TypeScript server,
  pyright, clangd, sourcekit-lsp or gopls exited, the request in flight waited out its 30 s
  timeout and every later query in that workspace failed with "Language server process has
  exited" until the gateway restarted. A request in flight now fails at once, the gateway loads
  a workspace whose server exited afresh for the next session, and the MCP server reopens its
  session once when told the server exited.

## v0.3.10 — 2026-09-25

### Fixed
- **A Rust symbol lookup finds a common name such as `run`** (#348). The workspace search walks
  rust-analyzer's index in name order and stops at its limit, so hundreds of names that only
  hold the letters (`a_dry_run_…`) filled it and `prod-code def --symbol editor_proxy::run`
  offered ten dependency functions instead. Symbols with the very name now come first.
- **An editor hears when a save did not reach the node** (#350). `prod-code lsp` pushes the
  checkout before it passes a save on; when that push failed, it passed the save on silently
  and the check on save ran on the node's previous copy. The editor is now shown a warning with
  the reason; the save still reaches the server.

## v0.3.9 — 2026-09-25

### Added
- **`code_definition {body: true}` / `prod-code def --body`** (#306): the definition's code,
  numbered, with its doc comments: the item's outline range, or where there is none its brackets
  or indentation, at most 300 lines. An agent no longer greps `func X -A45` to read it.
- **An editor gets the language's own server on the node** (#332).
  - `prod-code lsp` starts, for the editor's session, rust-analyzer, gopls, clangd,
    basedpyright, the TypeScript server or sourcekit-lsp in the node's copy of the checkout.
    The gateway only carries the protocol and translates paths. The editor's `initialize` and
    settings reach the server, and the server's requests and protocol extensions reach the
    editor.
  - rust-analyzer's check on save runs `cargo check` on the node, and a save is pushed before
    the server hears of it.
  - The editor's process id is kept from the server: basedpyright exited within 4 s when it
    named no process on the node.
  - `prod-code lsp --language <lang>` picks the server for one language of a mixed checkout.
  - `PROD_CODE_EDITOR_SERVERS=off` on a gateway serves editors from the shared engines again.
  - `PROD_CODE_LSP_TRACE=<file>` logs every message the bridge carries.
- **Files only the node has open in the editor** (#333): a definition in the standard library,
  a dependency or a generated file is copied into a read-only mirror under the user's cache
  directory and named by the copy.
- **A Zed extension** in `editors/zed` (#334): one server per language (`prod-code-rust`,
  `-go`, `-cpp`, `-python`, `-typescript`, `-swift`). It takes the user's settings for the local
  server it stands in for.
- **An editor on `prod-code lsp` gets completion, diagnostics, code actions and formatting
  for Rust** (#310, PR #320). The in-memory engine answers completion (with the `use` of an
  item not yet imported added on resolve), signature help, inlay hints, document highlights,
  code actions with their edits, and formatting by `rustfmt` on the node. An editor's session
  gets the diagnostics of each document it opens or changes pushed to it, 300 ms after the
  last edit. A request the Rust engine does not answer is refused instead of left without a
  reply, which an editor waited on for good.
- **`prod-code report-issue --label` and `code_report_issue {labels}`** (#321): the issue is
  filed with one type (`bug` when none is given) and the areas it is about; a label the
  repository does not have is refused before anything is sent. `CONTRIBUTING.md` says which
  labels every issue and pull request carries.

### Fixed
- **`prod-code metrics` after a restart** (#305): a window longer than what memory holds is
  summed from the nodes' daily event files, without counting an event twice.
- **validate and diagnostics no longer refuse code over an unproven `Send`/`Sync`/`Unpin`**
  (#327): rust-analyzer does not prove what rustc proves through a recursive `async fn`. Such an
  E0277 is shown as unconfirmed and not counted; `cargo check` decides.
- **The checkout's own symbol wins a tie with the standard library's** (#345): `Load` in a Go
  module no longer lists `internal/runtime/atomic::Load` and 20 more.
- **Symbol lookup** (#318, #324, #325, #326, #328, #342):
  - A module path qualifies a free function (`prod_code_mcp::report::report`).
  - A bare name is the type's, not a same-named enum variant's.
  - `symbols` ranks the names that hold the query first and leaves out letters-in-order
    matches when better ones exist.
  - A dependency's own symbol of the exact name (`ra_ap_ide::Analysis`) is found even when a
    workspace name resembles it, and its methods qualify by `Type::method`.
  - A name the root project lacks is looked up in nested projects of other languages, each on
    its own node, through their outlines when their server keeps no index (a Swift class in a
    Go module).
  - `introduce_parameter_object` writes field shorthand.
- **Command-line fixes** (#322, #323, #329, #330):
  - `validate --diff` no longer panics on a blank line after the patch. A blank line past the
    hunk's counted lines is not taken for a context line.
  - `test`, `check`, `lint` and `benchmarks` take `--path <crate, package or directory>`, as
    `code_test {path}` does.
  - `status` probes the node the checkout is placed on, not the first configured address.
  - `--symbol NAME FILE` on the position commands picks the NAME that FILE declares or uses.
- **When `prod-code lsp` cannot start, the editor is told why** (#338): the answer to its
  `initialize` is the reason, where the bridge used to exit before the editor asked (Zed showed
  only "server shut down"). On macOS, a node the app that started it may not reach
  (`No route to host`) comes with where to allow local network access.
- **A workspace-wide symbol search answers from the checkout, not from a nested crate of its
  own** (#335, PR #336). The file opened to load the project could be one of a crate the root
  workspace leaves out, and every search then answered from that crate.
- **`prod-code lsp` pushes the checkout before its session and keeps it current** (#316,
  PR #319). A node that had never seen the project detected no language in an empty copy and
  answered every request with nothing. The editor's `initialize` is answered with the
  language server's own capabilities (whole-document sync) and `serverInfo` `prod-code` with
  the real version.
- **gopls sees a file a sync rewrote** (#317, PR #319): the gateway sends
  `workspace/didChangeWatchedFiles` after every sync, where gopls went on answering from a
  file's old content until it restarted.

## v0.3.8 — 2026-09-24

### Fixed
- **Tracked `vendor/` directories and the libraries a build links reach the node, and a large
  sync no longer exceeds the frame limit** (#313). The sync skipped every directory named
  `vendor`, and every file over 5 MiB, even when git tracked them, so a Go package linking a
  vendored C library through cgo failed on the node with a missing header. A tracked `vendor/`
  is now synced, and a library a build links (`.a`, `.so`, `.dylib`, `.lib`, `.o`) may be up to
  128 MiB; data trees and other large files stay out as before. A sync used to go in one
  message, which a large tree could push past the 64 MiB frame limit: it is now cut into
  messages of at most 24 MiB of content, a file larger than that travels alone, and the frame
  limit is 256 MiB. Existing checkouts are probed once more to pick up what was skipped.

## v0.3.7 — 2026-09-24

### Fixed
- **A Go module with macOS-only cgo can use prod-code: a macOS node serves Go for it, and only
  for it** (#308). #248 rightly places such a module on macOS, but the macOS nodes served Swift
  only, so every tool failed there with a placement error. A macOS node can now run with
  `--engines swift,go`. Placement, in the gateway and in the client's fallback, keeps every
  workspace that does not need macOS off a macOS node while another live node can serve it, so
  plain Go stays on the Linux nodes however quiet the Mac is, and a workspace already on the Mac
  that does not need it moves back.

### Fixed
- **`prod-code report-issue` works in a checkout the cluster cannot place** (#307). It used to
  fail with the placement error before drafting anything, for example in a Go module with
  macOS-only cgo when no macOS node serves Go, so that very limitation could not be reported.
  The node is now looked up best-effort: without one the issue is still drafted and filed, and
  it says why the checkout could not be placed (node addresses scrubbed like the rest).

## v0.3.6 — 2026-09-24

### Fixed
- **Every agent is told to report prod-code bugs, without private details** (#302). The
  instructions the MCP server sends at `initialize`, which every agent reads whether or not it
  has a skill installed, now say to file a prod-code bug with `code_report_issue` once the tool
  is clearly at fault and then carry on. They say what goes in the body, that the issue is
  public so private details never go in it, and that details it needs go into the reporter's
  own private record, named by `private_ref`.
- **The first Swift check after a workspace loads finds type errors** (#295). sourcekit-lsp
  loads a package's build settings in the background after it starts, and until then it checks
  with fallback settings that report syntax errors only, so the first checks after a load said
  "0 errors" for code that did not compile. A SwiftPM workspace's load now opens one of its
  files with a line only a type check faults, and waits (up to 45 s) until sourcekit-lsp reports
  it; on a macOS node that took 0.8 to 1.0 s. A root without `Package.swift` does not wait.
- **`prod-code report-issue --help` says that similar issues, open or closed, are listed** (#299).
  The duplicate search has covered closed issues since #296; the help still said "open".

## v0.3.5 — 2026-09-24

### Added
- **Agents can report prod-code bugs as GitHub issues** (#290). MCP `code_report_issue` and
  `prod-code report-issue --title … --body …` file an issue in `alex09x/prod-code` through the
  machine's `gh` CLI, so an agent that hits a wrong answer, a hang or a missing capability can
  report it right away and carry on. Before anything is sent, LAN addresses become `<node>`,
  the home directory and `/Users/<name>` or `/home/<name>` become `~`, and the host name becomes
  `<host>`. An Environment section adds the client version and platform and the node's platform
  and engines. Issues with a similar title, open or closed, are listed first, and nothing is
  filed unless `force` is set: the agent comments on an open one instead, or checks whether a
  closed one is fixed in a newer release. Details that cannot be public stay in the reporter's
  own private record, and `private_ref` puts that record's id in the issue. `dry_run` shows the
  scrubbed issue without filing it.
- **Extract parameter and introduce parameter object work on C, C++ and Swift** (epic #13,
  refactorings beyond Rust). `code_extract_parameter` writes the new parameter as `T name` in C
  and C++, with `*` and `&` bound to the name (`const char *label`), and as `name: T` in Swift,
  or `_ name: T` when the function already takes a positional argument. A literal gives its own
  type (`80` is `int` or `Int`, `0.5f` a `float`, `"x"` a `const char *` or a `String`);
  otherwise clangd's `Type:` line or sourcekit-lsp's hover does, without clangd's `(aka …)`.
  clangd leaves declarations out of its references, so the header's prototype and an in-class
  method declaration are asked for separately and change together with the definition, and
  `f(void)` becomes `f(T name)`. `code_introduce_parameter_object` writes a C or C++ `struct`
  into the header above the prototype (above the class, for a method), rewrites every
  declaration of the function with its definition, and makes each call pass
  `(struct Opts){.a = x}` in C, `{.a = x}` in C++20 or later, and `{x, y}` before C++20, as the
  build declares it: clang accepts designators in C++17 as an extension, so the analyzer's own
  check cannot tell. Swift gets a `struct` of `let` properties, public when the function is,
  and each call passes `Opts(a: x)` under the new parameter's label. A list that ends in C's
  `...`, a C++ pack or an unlabeled Swift variadic is refused.

### Fixed
- **C and C++ checks judge the sources against the proposed header, not the one on disk**
  (#292). clangd parses an included header from disk unless it runs with
  `--use-dirty-headers`, and a source built before the header's new text is open keeps the old
  header's errors. A correct change to a prototype and its callers came back from
  `code_validate_edits`, and from every refactoring's check, as "Too many arguments to function
  call" and "Conflicting types", and `apply` refused it. The gateway now starts clangd with the
  flag, and a change's headers reach the analyzer before its sources.
- **C, C++ and Swift checks answer for the text that was sent, not an older one** (#293). For
  a server that does not advertise pull diagnostics, the gateway answered with the last
  diagnostics published for the file, whatever text they were for, so the same correct change
  came back with different errors from run to run. A server that answers a pull is now asked
  for one: sourcekit-lsp answers without advertising it, and the first thing it publishes for
  a file is an empty list, so every Swift check had said "0 errors", even for a type error.
  clangd, which does not, is waited for until it has published for the version last sent. A C
  or C++ workspace now validates on a second clangd without a background index: clangd kept a
  closed file in its index as it was last built, so a check's proposed text went on answering
  `references` for other sessions, and a later refactoring missed a caller.
- **A caller the language server did not report is checked with the change** (#294). Before
  the first `swift build`, sourcekit-lsp has no index and reports no references, so a Swift
  refactoring rewrote the declaration alone and passed its check. Files of the same language
  that call the function by name but got no reference are now checked together with the
  rewritten files, so a caller left behind shows up as an error and `apply` refuses it. The
  report lists those files and, for Swift, says to build first.

## v0.3.4 — 2026-09-24

### Fixed
- **Signature refactorings in Python, TypeScript and Go no longer miss call sites while the
  language server is still indexing** (#284). A language server that has just read the project
  answers `textDocument/references` with nothing. A refactoring that changes a signature took
  that for "no callers", rewrote only the declaration and reported a clean check. For a
  language other than Rust, an empty answer is now asked again three times, 800 ms apart, as
  `impact` already does for its call hierarchy (#202).

### Added
- **The extract parameter refactoring works on TypeScript, JavaScript, Python and Go** (part of
  epic #13, refactorings beyond Rust). `code_extract_parameter` and `prod-code
  extract-parameter` used to handle Rust functions only. They now also turn a selected
  expression in a TypeScript, JavaScript, Python or Go function or method into a new last
  parameter. The parameter is written the way the language writes one: `name: T` in
  TypeScript, `name T` in Go, `name: T` or `name` in Python, and `name` in JavaScript. The body
  reads the parameter at the selection, or at every identical occurrence with `replace_all`.
  Every call site the language server reports passes the original expression, so no caller
  changes behaviour; imports of the function are left as they are. The type comes from the
  literal (`80` is a `number` in TypeScript and an `int` in Python and Go), from the TypeScript
  server's, basedpyright's
  or gopls's hover when it covers the whole selection, or from `type`. As in Rust, an expression
  that names a local of the function cannot be written at a call site: the check reports it
  and `apply` writes nothing. A parameter list that ends in `...rest`, `...T` or `*args` is
  refused, because the new argument would not reach the new parameter. `verify: compile` stays
  Rust-only and says so.
- **Introduce parameter object refactoring for TypeScript, Python and Go** (part of epic #13,
  refactorings beyond Rust). `code_introduce_parameter_object` and `prod-code parameter-object`
  bundled parameters only in Rust. They now do the same in the other three languages, chosen by
  the declaring file's extension. TypeScript gets an `interface` with the parameters' declared
  types, and each call passes `{ a: x, b: y }`. Python gets a `@dataclass` with the parameters'
  annotations and defaults, `from dataclasses import dataclass` is added once, and each call
  passes `Opts(a=x, b=y)`. Keyword arguments are matched by name. A Python caller in another
  module gets the name added to its import from the declaring module. Go gets a `struct` whose
  fields keep the parameters' names, a group like `width, height int` is split correctly, and
  each call passes `Opts{a: x, b: y}`, with the package qualifier when the call has one. A
  method (TypeScript class method, Python method with `self`, Go method with a receiver) gets
  the new type above its class, or above the method in Go. Uses in the body read the fields
  through the new parameter. A parameter without a declared type takes the type the language
  server's hover gives it. Python falls back to a plain class when basedpyright reports
  `Unknown`. A reference that is not a call with the declaration's arity is reported, not
  rewritten, as in Rust. basedpyright lists the keyword in a call such as `build(n, height=2)`
  as a reference to the parameter `height`, so only references inside the function body are
  rewritten as uses. Rust output is unchanged.

## v0.3.3 — 2026-09-24

### Performance
- **The first validation after a gateway restart takes 2 s instead of 24 s** (#235). A validation
  reads the file as it is on disk first, so that errors the file already had are not counted
  against the edit, and it read it from the main rust-analyzer engine. The background warm-up of
  #233 warms the validation engine, so that first pass was cold: 21 of the 24 s for
  `crates/prod-code-client/src/main.rs`. Both passes now run on the validation engine. Measured
  on a Linux x86_64 node after a restart and the warm-up: 24.19 s before, 2.21 s after.
- **A new git worktree of a Rust project no longer compiles every dependency on the build node**
  (#278). When the gateway seeds a new worktree's copy from the main checkout's copy, it now
  also copies that copy's `target/debug` compiled crates, build-script outputs and cargo
  fingerprints, keeping their modification times. It skips the copy when the disk would keep
  less free space than the cache's size. Registry crates have the same source path in every
  copy, so only the workspace's own crates are compiled. For this repository, on a Linux aarch64
  node, `cargo test --workspace --no-run` compiled 8 crates in 37.0 s after a 5.9 s copy,
  against 305 crates in 106.2 s. The cache is the worktree's own afterwards, with no shared
  directory and no shared build lock.

## v0.3.2 — 2026-09-24

### Fixed
- **Go to definition, hover and references work on Rust types from dependency crates** (#271).
  `--symbol Framed` called tokio-util's `Framed` ambiguous: the resolver told a definition
  from its `pub use` re-export by reading the file on the local disk, and a dependency's
  source exists only in the build node's cargo registry. Such files are now read from the
  gateway, and a position query on one no longer tries to open it on this machine.
- **`code_outline` reports why a file cannot be outlined instead of an empty outline** (#270).
  The gateway turned every error of the Rust engine's document symbols into an empty list, so
  `prod-code outline README.md` in a Rust workspace printed a header and nothing, with exit
  code 0. An agent could not tell a refused file from one that declares nothing. The error is
  now an LSP error response: `README.md is not a Rust file, and no language server of this
  workspace outlines it`.
- **A build node running a remote build or test is no longer shown as idle** (#273).
  `prod-code status`, `prod-code cluster` and MCP `code_status` counted language-server
  sessions and queries only. A `cargo test` started through `exec`, `code_check` or `code_test`
  was counted nowhere, so the check before a gateway restart (`Active Sessions: 0`) passed in the
  middle of it. Each running command is now listed with its workspace, how long it has run and
  its command line, and the cluster table has a `commands` column.

## v0.3.1 — 2026-09-24

### Fixed
- **The remote build node's copy of a checkout no longer keeps stale files from a killed
  command** (#262). A command such as `cargo fmt --all`, `clippy --fix` or a code generator
  rewrites files on the build node, and the client writes the changes back when the command
  ends. When the client left first, the gateway killed the command and the rewritten files
  stayed on the node. The next sync sends only what changed locally, so later checks ran on
  code that was not in the checkout: a formatting check passed on the node and failed on
  `main`.
  - Before a command that sends its changes back, the gateway now also keeps the old contents
    of every file up to 1 MiB, within 256 MiB per command.
  - When the client disconnects, its connection fails, or the changes cannot be sent, the
    gateway kills the command, waits for it to exit and puts every file back: changed files
    get their old contents, new files are removed, deleted files come back. The warm analyzer
    is told, so it answers from the old text again.
  - A file whose old contents were not kept is removed from the node's copy and reported as
    stale in the next handshake and sync answers. The client then sends its own version of it,
    whether or not git sees a change.
  - A file that a sync from the checkout delivered while the command ran is left alone, since
    it is the checkout's newer text. If the command changed it again after it arrived, it is
    reported as stale too.

## v0.3.0 — 2026-09-24

### Performance
- **Git worktrees of a C/C++ project share one compiler cache on the build nodes** (#243). Each
  worktree has its own server copy, so every one compiled from scratch. When the node has
  ccache, every command now runs with `CCACHE_BASEDIR` set to its workspace, `CCACHE_NOHASHDIR=1`
  and ccache as CMake's C and C++ compiler launcher. The caller's own variables still win.
  - Through `prod-code exec` on a Linux build node, the fmt library (114 compiles) built in
    24.0 s in one worktree and in 1.0 s in a second one.
  - sccache was measured first and shared 2 of 114 compiles: it keys by absolute path and reads
    its base directories only when its server starts.
  - ccache is installed on the three Linux build nodes.
- **Semantic search no longer waits for the background embedding pass** (#240). A question and
  the background pass shared one model, so a search waited for the batch of 64 declarations in
  progress: 555 ms instead of 4 ms right after a workspace loaded. Questions and declarations
  now each have their own model instance, loaded from the same directory. That costs one more
  copy of the model, tens of megabytes for BGE-small.
- **The gateway warms rust-analyzer for the files an agent is editing** (#233). The first
  validation of a large file after the analyzer starts inferred every function and computed
  every diagnostic cold: 53–58 s for the 4,246-line `crates/prod-code-client/src/main.rs` on a
  Linux build node.
  - When a sync writes Rust files, the gateway loads the validation engine in the background if
    needed. Loading it warms the 10 most recently modified files, and the synced files are
    warmed on the engine that is already loaded.
  - A warm-up infers each file's functions on 16 threads and computes its full diagnostics, all
    on snapshots without the engine lock. The three newest files then also go through the
    engine's own diagnostics call.
  - The same validation after the warm-up took 24 s. The rest is one diagnostics pass that the
    session buffer's open makes cold again (#235).
  - The diagnostics pass logs its parts (`analyzer_ms`, `unused_imports_ms`,
    `unresolved_paths_ms`) at debug level, and a text change applied to the database is logged
    with its old and new length.

### Fixed
- **A symbol name resolves only to a symbol of that name** (#253). `--symbol NAME` in the CLI
  and `symbol` in the MCP tools took the best-scored `workspace/symbol` hit. The index also
  returns fuzzy matches, so a Rust struct field, which rust-analyzer does not index, resolved
  to an unrelated function, and `code_rename` or `code_safe_delete` would have acted on it.
  - A hit counts only when its name is the requested one, ignoring ASCII case, a trailing
    parameter list (`bar()`) and a qualifier the server put in the name (`Type.bar`).
  - `Type::member` that the index does not list is looked up among the children of the type's
    symbol in its file's outline; the children of an `impl` block for the type count too.
  - When nothing matches, the error lists up to five of the closest names the index returned
    (`did you mean: a, b, c`) instead of picking one.
- **A Go module with macOS-only cgo is built and tested on a macOS node** (#248). cgo code that
  includes `libproc.h`, `mach/`, `CoreFoundation/`, `IOKit/` or other Apple headers, or links a
  `-framework`, compiles only on macOS. Such a Go module used to be placed on a Linux build node
  like any other, and every remote build and test failed with errors about missing headers. The
  client now reads the module's `.go` files at startup, up to 4,000 of them. When one imports
  `"C"` with such a preamble and Linux would build it, the checkout goes to a node that runs
  macOS. Files that Linux skips anyway do not count: a `//go:build darwin` line, a `_darwin.go`
  name, an include inside `#if`, a `#cgo darwin` flag. Gateways now report their platform in
  their status, and a cluster placement request can name the OS it needs. A gateway too old to
  report its platform does not count as macOS. With no macOS node the client stops with the
  reason, for example `this Go module uses macOS-only cgo (proc.go: libproc.h): no reachable
  gateway runs macOS`. Checkouts that need no macOS are placed as before.
- **The peak memory of a remote build or test is the command's own, not the gateway's** (#255).
  `prod-code exec`, `code_check` and `code_test` reported the gateway's resident set as the
  peak of every command: `prod-code exec -- true` showed more than 21 GB on a Linux build node,
  because Linux carries a process's memory high-water mark into a child it forks. The gateway
  now starts each command through a small shim, its own binary run with `--exec-shim`, which
  starts the command and reports what the command and its descendants used. The peak is now
  the largest resident set among the command's processes, and exit codes and signals pass
  through unchanged.
- **A remote command no longer overwrites a file edited here while it ran** (#254). `exec`,
  `code_check` and `code_test` write back the files a command changed on the build node, and
  they wrote the node's text over the local file without comparing it to anything. An edit made
  during a long run was lost when the node's version differed.
  - A file modified here after the command started is now kept, and the output names it.
  - A file the node already has with this machine's text is neither written nor reported. That
    happens when another command syncs the checkout during the run. Such files had been reported
    as "changed by the command" and set off a false platform warning.
- **A remote build or test no longer waits forever on a dead connection** (#256). The client
  set no TCP keepalive, so when a connection to a build node died without a close, a command's
  final message was lost and `exec`, `code_check` or `code_test` waited indefinitely. One check
  waited nine hours after the node had finished it. Every connection to a gateway, and every
  connection a gateway accepts, now has TCP keepalive: probes after 30 s of silence, 10 s
  apart, reset after 3 unanswered. A lost exec fails and says its result is unknown.
- **`cargo fmt` no longer triggers the platform warning when rustfmt drops a closure's braces**
  (#244). The layout rule of #239 ignored whitespace and commas only, and rustfmt also removes
  the braces around a closure whose body is one expression. Braces now count as layout.
- **Files of another language in a mixed repository are served by their own language server**
  (#247).
  - A loose Python script in a Rust repository (a directory with no project of its own) went to
    rust-analyzer, and its outline came back empty. Its directory is now served by the Python
    engine: the client names the engine in the handshake (`preferred_engine`) with the
    directory as `engine_subpath`. `scripts/coverage.py` in this repository is outlined with
    its 5 declarations. A file at the root itself stays with the root.
  - rust-analyzer no longer adds a file that is not Rust to its database when a client opens
    it. README.md had been parsed as Rust and outlined as `[Interface] or`. Its outline is now
    an error saying no language server of the workspace outlines it.
- **A checkout stays on its build node through a gateway restart** (#238). One refused
  connection to the remembered node re-placed the checkout, so a two-second restart for a
  deploy moved it to another node and left its warm rust-analyzer behind. An unreachable
  remembered node is now asked again up to 4 times, 750 ms apart, before the checkout moves.
  A node that answers but cannot serve the engine is still left at once.
- **A file's diagnostics get 300 s instead of the 60 s interactive budget** (#237). A cold full
  check of a 4,246-line file took 85 s on an aarch64 node, so the client gave up and sent the
  same work again.
- **The exec platform warning no longer fires on `cargo fmt`** (#234). The #140 warning named
  every written-back file that mentions `cfg(target_…)` or `libc::`, including files a
  formatter only laid out again, so every format-and-check run showed a false alarm. A
  write-back whose characters are the same apart from whitespace and commas, in any order, is
  now layout: rustfmt re-wraps lines, adds trailing commas and sorts imports. It is not named.
  An edit that adds or removes characters, such as `&mut win` to `&win`, still is.
- **impact asks a cold language server again before it reports no callers** (#202). A Python
  server that had just started answered the call hierarchy with nothing, and `impact` said "none
  reach the changed functions", so an agent would skip the tests. For every language but Rust, an
  empty answer for the first changed function is asked again up to three times, 800 ms apart,
  before it is believed. rust-analyzer answers from a database that is already loaded.
- **exec, check, lint and test name the platform they ran on, and warn when a write-back may be
  wrong here** (#140). A `clippy --fix` on a Linux node rewrote `&mut win` to `&win` for
  `libc::openpty`, whose Apple signature takes `*mut`, and the checkout stopped building on
  macOS. The only message was "15 file(s) … written back".
  - `ExecExit.platform` carries the node's OS and architecture (`linux x86_64`). The exec
    summary and every check/lint/test summary print it.
  - When files were written back from a node whose OS differs from this machine's, and they
    hold platform-dependent code (`cfg(target_…)`, `cfg(unix)`, `libc::`, `#ifdef __APPLE__`,
    `//go:build`), or the checkout is an Apple project, the result names them and says to
    build here before trusting it.
- **supertypes reads supertraits written as `where Self: …`** (#227). `trait Circle where Self:
  Shape {}` answered "requires no supertrait", because only the bounds after the colon were
  read. Bounds on `Self` in the `where` clause count too, and `where` is matched as a word, not
  inside a name like `Somewhere`. The tool's description now names only the servers it was
  checked against, clangd and gopls.
- **`--symbol` no longer calls a re-exported name ambiguous when its `pub use` spans lines**
  (#225). `pub use messages::{ …, WireMessage, … };` put `WireMessage` on a continuation line,
  which was not recognised as part of a `use`. So `hover --symbol WireMessage` listed the
  re-export and the enum as two candidates. Every line of a `use`, up to its `;`, now counts.
- **move_method: three gaps found by the review of the post about it** (#207).
  - A path call (`Order::price_with(f(), g(), 2)`) swapped its arguments without the effects
    check a method call gets. It is now blocked the same way.
  - Only the uses the analyzer resolves to the moved parameter become `self`. Before, every
    whole-word `tax` in the body did, including one bound again by `if let Some(tax)` or
    `for tax in`. The textual rebinding check is gone.
  - With `force`, a blocked call is rewritten too. Before, it was left calling a method that
    had moved.
- **make_static: `force` rewrites a blocked call too** (#209). A call whose receiver runs
  something was left as `load().twice(3)` after `twice` stopped taking `self`, so the forced
  result did not compile.
- **`move` no longer pastes a method into a module as a free function** (#196). It cut
  `Order::price_with` out of its `impl` and appended it, still taking `&self`, after `impl
  Tax` in `src/tax.rs`. It now refuses and names `code_move_method`.
- **extract_function no longer replaces a duplicate whose later code reads a shadowed name**
  (#189). The selection binds `gross`, and the new function does not return it. Where the code
  after a duplicate still reads `gross` and an outer `gross` exists, the call type-checked and
  compiled, and the read silently took the outer value. A duplicate is now refused before any
  type check when the code after it reads a name that the selection binds and the call does
  not. The report names the variable.
- **`move` no longer moves a module as a one-line item** (#188). On `mod b;` it cut the line,
  pasted it into a new file as `pub mod b;` and requalified callers to `c::b::b`. It now refuses
  and names `code_move_module`.
- **validate reports a type or module that does not exist** (#181). rust-analyzer has no
  diagnostic for an unresolved type path (rustc's E0412/E0433), so a file naming `NoSuchType`,
  `missing_crate::Thing` or an item another crate does not export passed with 0 errors.
  `cargo check` then rejected it. Diagnostics now read what rust-analyzer highlights as an
  unresolved reference, and report each such path segment as `unresolved-path`, when:
  - it starts its path, or it follows `crate`, `self`, `super` or a module;
  - no `use` in an enclosing scope imports the name (a broken import is reported at the `use`).
  A name missing from another crate is a warning, since that crate may hold code the analyzer
  cannot see. A crate that `include!`s build-script output is one example.

  On the three Rust repositories at hand (122 files), the only reports on unchanged code were
  10 warnings in such a crate. `tools.rs` (4,381 lines) takes 1.09 s warm instead of 0.80 s.
- **impact's first line no longer counts unknown callers as zero** (#170). When the Swift index
  could not be built, the header still said `0 caller(s), 0 test(s)` above the line saying they
  were unknown. It now says `callers and tests unknown`.
- **impact on Swift no longer says no test reaches a change it never looked at** (#166). The
  macOS node runs Swift 5.10, whose sourcekit-lsp finds a caller in another file only through
  the index a build leaves. Before any build, `impact` answered "affected tests: none reach the
  changed functions", and an agent would skip the tests. Two changes:
  - for a Swift package, `impact` first runs `swift build --build-tests` on the node, which is
    incremental (4.9 s on a scratch package with no `.build`), and says so in the report;
  - when that build fails, the report says the callers and tests are unknown and to run the full
    suite, instead of none.
- **Validate no longer refuses a new file for the analyzer's derive expansion** (#159).
  rust-analyzer reports "type annotations needed" (E0282) at a `#[derive(Deserialize)]` line when
  it cannot type its own expansion of the derive, and `cargo check` compiles the file. An existing
  file subtracts this through its baseline. A new file has none, so validate rejected it.
  `fixit.rs` was rejected with 10 such errors. An E0282 on a `#[derive(...)]` line is now set aside
  in every validated file and listed apart ("… on a #[derive(...)] line are not counted"). Any
  other code on that line, and an E0282 anywhere else, still counts.
- **introduce_variable no longer changes what the code does in two cases** (#155). Both used to
  write a different program, and the type check accepted it:
  - a method call on a name the expression reads (`s.bump()` between two copies of `s.x + 1`)
    now counts as a possible change, like an assignment;
  - an expression that can panic (`/`, `%`, indexing, arithmetic that overflows in a debug
    build) is bound earlier only when one occurrence runs every time the binding does. That
    occurrence must sit at the binding's block level, with no `return`, `break`, `continue`, `?`
    or panicking macro before it, and no `&&`, `||` or closure earlier in its statement. Two
    divisions each under their own `if b != 0` used to be hoisted above both guards, so
    `ratio(1, 0)` panicked. That change is now refused;
  - an occurrence written `(expr)` on its own is replaced by the bare name (`a + x1`, not
    `a + (x1)`).
- **`prod-code outline` no longer lists every local variable** (#151). The CLI had its own copy of
  the outline renderer. It printed every `Variable` the analyzer reports: 147 of the 206 entries
  for `move_item.rs`. The CLI and `code_outline` now share one renderer:
  - a variable inside the range of a function or method is a local, is hidden, and is counted in
    the last line (`147 local variable(s) hidden; pass --locals to list them`);
  - a top-level `static`, which the analyzer reports with the same kind, now stays in both. The
    MCP tool used to hide it along with the locals;
  - `prod-code outline --locals` lists the locals, like `include_locals: true`.
- **Safe delete at a parameter removes the parameter, not a line of the body** (#138). The engine
  fell back to the smallest item containing the position when no item was named there, so
  `safe-delete` on a parameter deleted the function's only expression and reported success.
  Changes:
  - at a parameter, `code_safe_delete` / `prod-code safe-delete` now remove it with its argument
    at every call through `change_signature` (roadmap 7.1.1, cascading parameter removal),
    refused while the body uses it and type-checked before writing;
  - the engine refuses a position that names no item;
  - an answer with no edit is reported as an error, not as "deleted; 0 path(s)";
  - the CLI goes through the same tool.
- **A removed local variable is not a stale reference** (#136). The multi-file check takes the
  names an edit removed from a diff of the document symbols, and rust-analyzer lists a
  function's `let` bindings among them. Removing two locals (`edit`, `touched`) from one function
  flagged 51 lines of other files, each with its own `let edit`. Symbols of kind `Variable` no
  longer count; the same check now reports none.
- **Validation reports an unused import** (#134). rust-analyzer computes no diagnostic for an
  unused import, so `validate` called a file with one clean while `clippy -D warnings` rejected
  it. The analyzer does offer `remove_unused_imports` exactly on a `use` item that has an unused
  name, so the Rust engine now asks each `use` item for its assists and reports the ones that
  offer it as `unused_imports`, a warning, as rustc would.
- **A re-exported name resolves to its definition** (#128). The workspace index lists a
  `pub use m::name;` next to the definition it names, and `--symbol name` called the pair
  ambiguous (for `scan_workspace_files` in this repository). A tie whose line is a `use`
  declaration now gives way to the definition, and the definition is what is returned: sorted by
  path, `lib.rs` came first and would have been picked otherwise. Two definitions of the same
  name are still ambiguous.
- **The Swift half of a mixed repository reaches the macOS node** (#125). A checkout whose root is
  a Rust crate was always sent to a Rust node, so `code_test {path: "swift"}` failed with "failed
  to start swift", and even `--remote <macOS node>` ran on Linux because the seed was expanded
  into the whole cluster and the remembered placement won. Now:
  - a tool call that names a path in a nested project of another language is routed to a node
    serving it (`cluster::route_for_path`, before the tool runs), placed under
    `<workspace>#<engine>` so the checkout's own placement is kept;
  - a CLI run from inside such a project uses the same key;
  - nodes given with `--remote` on the command line are used as given (`PROD_CODE_REMOTE` stays
    a seed for discovery);
  - with no node serving the engine, the error says so ("no reachable gateway serves swift").
- **Sync mirrors what git lists, whatever the extension** (#123). A checkout's files were filtered
  by an extension allowlist, so test fixtures (`.txt`, `.bin`, `.recording`), `include_bytes!`
  data and `.github/` never reached the gateway, and a remote `cargo test` failed with "couldn't
  read `tests/fixtures/capture.bin`". In a git checkout the synced set is now git's: tracked
  files and untracked files that are not ignored (`is_synced_git_path`, `git ls-files --cached
  --others --exclude-standard` for a full scan). Data trees, build output, vendored dependencies,
  `.git` and the size limits still keep files out. `RELEVANCE_VERSION` is 5, so every client's
  next sync is a first contact with a manifest reconciliation.
- **A directory gone locally is gone from the gateway copy** (#124). Deleting a synced file
  removes the directories it leaves empty, and the manifest reconciliation clears empty
  directories left from before; the per-node caches are never touched.
- **An applied edit's report shows its diff again** (#122). Every write tool rendered its diff by
  reading the old text from disk, after `apply` had already written the new one, so an applied
  change reported "0 changed line(s)" and no hunks. `refactor::apply_workspace_edit` now keeps,
  per file, the text before the edit and the bytes it wrote, and `refactor::text_before_apply`
  returns that old text while the file still holds exactly what the edit wrote (and the file as
  it is otherwise). The twelve renderers were moved onto it with `prod-code codemod`. Found while
  applying `convert_to_method` on a scratch crate.
- **`code_make_static` refuses a method in a trait `impl`** (#113). The trait decides whether its
  methods take a receiver; an implementation that dropped it would no longer implement the trait,
  and the first version would have dropped it. Found while writing the post that describes the
  tool, where the claim that "the check reports it" could not be backed.
- **The coverage gate no longer fails on a file with no code** (#102). A crate root of `pub mod`
  and `pub use` lines has no regions, and naming it failed the whole run with "no coverage data"
  after a seven-minute instrumented build, whose report was a temporary file. Such a file is
  now listed as `no code`; only a path that does not exist fails. The report is kept at
  `target/coverage-report.json`, so a rerun over other files can pass `--report` and skip the
  build.
- **An assist's prelude paths are spelled as the names in scope** (#97). rust-analyzer writes
  a prelude item an assist introduces by its full path, so every function `extract_function`
  made in this repository returned `std::prelude::v1::Result<T, anyhow::Error>`. Where an assist
  adds such paths to a file that had none, `code_assist` drops the prefix and applies the shorter
  text only if the overlay check accepts it; otherwise it applies rust-analyzer's. The report
  says how many were shortened. `prod-code assist` goes through the same handler.
- **An assist refused inside a macro call says so** (#99). Inside a macro call's input
  rust-analyzer offers few refactorings — inside `tokio::select!` only `inline_macro` — and the
  refusal said only "not offered here; available: inline_macro". It now names the macro and says
  to move the code out into a function first, which is how the gateway's 1,400-line
  `select!` arm had to be split (#86).
- **An analyzer panic no longer fails a whole validation** (#94). rust-analyzer panics on some
  valid code (a `move` closure passed to `std::thread::scope`, for one), and the panic came back
  as `query task failed: task N panicked`, which failed `validate` and every write tool without
  naming the file. A panic while diagnosing a file is now one error at the top of that file,
  `prod-code::analyzer-panic`, which quotes the panic, says nothing in the file was checked, and
  points at the compiler as the check that remains. It is never set aside as a diagnostic the
  file already had, and the other files of the same validation are still reported.
- **A rename that does not compile is refused** (#98). The analyzer computes a rename without
  checking the result, so renaming a function to a name already declared in the same scope
  produced a second definition and reported success; splitting the gateway's session loop
  left eight duplicate functions that way. `code_rename` now applies the edit in memory, checks
  the renamed files in one overlay, and refuses with the errors unless `force` is given, in which
  case it writes and names them. `prod-code rename` goes through the same handler and gains
  `--force`. A rename that also moves module files says that part was not checked.
- **Validating a proposal to a large file takes seconds, not tens of seconds** (#86). After a
  proposal changes what a crate declares, rust-analyzer infers every body of the file again, one
  function on one thread. Two functions made that the whole cost: `execute_tool` in the MCP
  tools (1,786 lines) and the client-message arm of the gateway's session loop (1,412 lines of
  `tokio::select!` input, where no refactoring assist works). `execute_tool` is now a dispatch
  over 32 handlers; the session loop calls `on_client_message`, whose LSP fast paths are ten
  handlers. Both splits were made with prod-code's own `extract_function` assist, `rename` and
  `codemod`. The engine also infers a file's functions on several threads before its diagnostics
  pass. Measured on a Linux build node, a new proposal that adds an item: `tools.rs` 25.0 s →
  3.0 s (3.95 s from the split alone), the gateway's `lib.rs` 33.4 s → 4.8 s.
- **The gateway no longer floods the journal with the analyzer's query log** (#95). The default
  filter was `info`, and rust-analyzer's crates and salsa log every query they execute at that
  level. One validation of a large file emitted about 1.2 million lines, the system journal kept
  its burst of 10,000 and dropped the rest, and the gateway's own lines were dropped with them.
  The analyzer's crates and salsa now default to `warn`; one validation writes 26 lines.
  `RUST_LOG` still overrides the default.
- **`code_extract_field` no longer writes a field into a pattern it took for a literal** (#90).
  Braces were a pattern only when `=>`, `=`, `|` or `:` followed them directly, so
  `for Store { a } in all`, `Store { a } if … =>`, `Some(Store { a }) =>` and
  `matches!(s, Store { a, .. })` were read as values to initialise. `in` and a match guard's `if`
  now count, a `)`, `]`, `}` or `,` sends the question to the brackets around the braces, and
  braces ending in a bare `..` are always a pattern, since a literal's update names its source.
- **A dry run no longer slows down the next query** (#73). Validation opened proposed texts
  as overlays in the same engine every other query uses; when a proposal changed what a widely
  imported file declares, the overlay and its revert made rust-analyzer re-infer every body that
  resolves through that crate, and the next ordinary query paid for it — `references` took
  21.5 s after an encapsulate dry run, 0.1 s settled. Validation sessions now say so in the
  handshake (`purpose: "validation"`) and the gateway serves them from a second engine for the
  same workspace, loaded by the first of them, fed by every sync and command write-back like
  the main one. The on-disk baseline of #79 is taken from the main engine, so the validation
  engine only ever holds proposals. Measured on a Linux build node against this repository:
  `references` right after a dry run 21.5 s → 0.13 s; the same dry run repeated 43.7–44.5 s →
  1.0–1.1 s. A new proposal still costs what type-checking the files it touches costs (24.5 s
  for `move snake_case --to lang.rs`), now on the second engine alone.
- **An error the file already had is no longer counted against an edit** (#79). The analyzer
  reports `type annotations needed [E0282]` on every `#[derive(..., Deserialize)]` in this
  workspace, with or without an edit — 124 of them in `prod-code-protocol/src/messages.rs` — and
  the overlay check behind `code_validate_edit` and every write tool counted them, so an edit
  that changed nothing was "rejected" and any refactoring touching such a file refused to write.
  Each file's diagnostics on disk are now taken before the proposed texts are opened, and one
  with the same severity, code and message on a line with the same text is set aside and
  counted in the report rather than listed as the edit's. A second copy of an old error on a
  new line is still the edit's.
- **A write tool checks the name is where the analyzer says before rewriting a call.**
  `code_introduce_parameter_object` and `code_extract_parameter` edited the call that followed
  whatever position the analyzer reported; when that position was stale (#75) and the position
  plus the callee's length happened to land on another call's `(`, the argument went into that
  call — in this repository `is_some_and(|g| g.applied, 2)`: column 35 plus the six letters of
  `render` is 41, which is `is_some_and`'s parenthesis. A position that does not hold the
  callee's name is now reported under "not rewritten" with the reason, and nothing there is
  touched.
- A test that bound a port, dropped it and expected a refused connection failed now and then
  when another test's listener took the port first; it uses a port nothing can bind.
- **A command that rewrites files no longer leaves the analyzer on the old text** (#75).
  `code_exec` sends the files a command changed back to the client, which records them as synced,
  so no later sync carried them to the node — and the warm engine was never told. It kept
  answering from the text it had before the command, and because the file a query is about is
  always opened fresh, the drift only showed in *other* files: references, definitions and every
  write tool's call-site rewrite, off by however many lines a formatter moved them. A command's
  changes now reach the engine the way a sync does, and a changed manifest reloads it. The
  gateway has to be redeployed for this.
- **A write tool can ask the compiler before writing** (#63). The overlay check every write tool
  runs does not report an unresolved type or a module path that does not resolve, so "the
  analyzer accepts the result" was weaker than it read. `code_move`,
  `code_introduce_parameter_object`, `code_extract_parameter`, `code_change_signature` and
  `code_schema_rename` take `verify: "compile"` (`--verify compile` on the CLI): the proposed
  files become one hypothesis in a shadow of the workspace, `cargo check` runs there against the
  warm target directory, and the change is written only if the compiler accepts it too. It adds
  the check's own time — 2.3–2.6 s on this repository — and the report says which check ran.
- **A multi-file edit is written whole or not at all** (#70). Every write tool ends in
  `apply_workspace_edit`, which renamed and deleted during its first pass and wrote contents one
  file after another: the first failure returned a bare OS error and left whatever was already
  written in place. It now snapshots every path the edit touches before the first side effect,
  and on any failure puts each one back — the bytes it had, or no file where there was none —
  and says so: `the edit failed partway and was undone: N file(s) put back as they were`.
  Directories are never removed.
- **change-signature no longer loses a declaration that a call above it moved** (#58). The
  structural rewrite renders a call site it changes on one line; when that call sat above the
  declaration across several lines, everything below it moved up, and the declaration was looked
  for again at its old line and column and not found — reported as `the declaration moved while
  its call sites were rewritten`, naming nothing. The declaration is a declaration, not a call,
  and the rewrite never touches it, so it is now found by its own text. If that text is gone or
  appears twice, the refusal names the function and its signature. The command from the issue —
  reordering `execute_lsp_query`, 178 changed lines in 6 files — now succeeds.
- **Half a second off every CLI invocation** (#56). The gateway probed for installed language
  servers on every `StatusRequest`, `Gossip`, `ClusterRequest` and placement decision, and one
  of those probes is `npm root -g`, which spends 213 ms starting node. A CLI invocation asks
  two such questions before it can send its query — where the cluster is, and which node holds
  this workspace — so it paid the probe twice. The answer changes only when somebody installs a
  language server, so it is now taken once at startup, refreshed by the janitor's existing
  minute tick on a thread that may block, and read from memory on the request path. Measured on
  a Linux build node over loopback: `StatusRequest` 436.8 ms → 0.2 ms (median of 7), and
  `prod-code status` end to end 1315.5 ms → 5.0 ms (median of 5, development build).

### Added
- **Rust symbols in dependency crates are found by name** (#246). `symbols`, `--symbol NAME` and
  `code_symbols` searched only the workspace's own crates, so a type from a dependency
  (`Framed` from tokio-util) could not be found. When the workspace has no match, rust-analyzer's
  library symbol search now runs. Each hit names its crate and module (`tokio_util::codec::framed`),
  read from rust-analyzer's moniker for it. Workspace hits still come alone when there are any.
- **Outline a whole directory or package in one call** (#245). `code_outline` and
  `prod-code outline` take a directory. They return the declarations of every source file
  directly in it, one block per file, through one language-server session, and end with
  `N file(s) outlined, M skipped`. Files no language server outlines, such as manifests and
  READMEs, are skipped rather than asked for. Subdirectories are not descended into.
- **Supertypes** (roadmap 7.5, #224). `code_supertypes` / `prod-code supertypes` answer the
  upward half of the type hierarchy (`code_implementations` is the downward one).
  - A Rust type's traits: the derived ones read from its `#[derive(…)]` attributes, including a
    macro's such as serde's, and the written ones from its `impl Trait for Type` blocks. An
    inherent `impl Type` is not listed.
  - A Rust trait's supertraits, from its header.
  - Other languages: their server's `typeHierarchy/supertypes`, or a note that it has none.
- **Callers and callees to a depth** (roadmap 7.5, #222). `code_callers` / `code_callees` take
  `depth`, and `callers` / `callees` take `--depth N` (1 to 6). Deeper levels come as an
  indented tree. A function already shown is marked `(shown above)` and not expanded again, so
  recursion ends, and a tree stops at 300 functions and says so. The CLI now answers through the
  same code as the MCP tools, and its separate copy is gone.
- **Intent search ranks by meaning too** (roadmap 8.4, #218).
  - The gateway embeds every declaration with a small sentence-embedding model (BGE-small,
    int8 ONNX, run in process), in a background pass after the index is built. A file that
    changes is embedded again.
  - A question is ranked lexically and by cosine, and the two lists are fused by reciprocal
    rank. The result says how far the embedding has got, or that the search is lexical only
    when the gateway has no model.
  - On this repository, 13 questions phrased in other words than the code find an expected
    answer in the top three 7 times fused, against 5 lexically and 6 by meaning alone.
- **schema-rename: OpenAPI and GraphQL by structure, and one change across repositories**
  (roadmap 7.6, #216).
  - An OpenAPI document (YAML or JSON with an `openapi` or `swagger` key) is rewritten only
    where the field is a key (`order_id:`) or a whole value (`required: [order_id]`,
    `name: order_id`). A GraphQL schema is rewritten only where the name is outside a `#`
    comment, a string and a `"""` description. Mentions in prose are listed, and the summary
    counts them under `openapi` and `graphql`.
  - `--repo PATH` (repeatable; MCP `repos`) adds repositories to the same change. Each is
    planned and checked by its own analyzers. `--apply` writes all of them or none: when one
    cannot be written, the ones already written are put back.
- **Typed runs: environment, streamed events and resource use** (roadmap 6.1, #214).
  - `check`, `lint`, `test` and `benchmarks` take `--env KEY=VALUE` (repeatable). The four MCP
    tools take `env` as an object (`{"RUST_BACKTRACE": "1"}`). It is refused with `fix`.
  - `--events` prints one JSON line per diagnostic (cargo's JSON) and per test result
    (`cargo test`, `go test -json`) as its line arrives, and then the report as the last line
    (`{"event": "report", "report": …}`). Other runners report at the end only.
  - The report carries the run's CPU time and peak RSS (`usage`), and its summary prints them:
    `rust test: OK in 439.5s; 657 passed, 0 failed; cpu 911.5s user 68.2s sys, peak 7140 MB`.
- **extract_function: near-duplicates and other files** (roadmap 7.1.2, #212).
  - Copies are matched token for token, so whitespace no longer matters.
  - With `parameterize` (`--parameterize`), a copy may differ from the selection in literals
    of the same kind. Each literal that differs becomes a parameter, typed as the analyzer's
    hover types the selection's own literal (`value: u32`), and every call passes its own
    literal. The parameters are kept only if such a copy is.
  - With `other_files` (`--other-files`), the crate's other files are searched. A copy there
    calls the function through its module path (`crate::net_price(o, 100)`), and the function
    becomes `pub(crate)`. A method is not offered across files.
  - Each place is still type-checked before it is kept, now across every file it touches.
- **diagnose suggests the compiler's fixes when the tests do not build** (roadmap 8.2, #206).
  - The dossier lists the machine-applicable fixes rustc has for the build errors, for example
    `src/lib.rs:10: mismatched types: consider borrowing here`.
  - It names `prod-code check --fix`, which applies them.
- **`lint --fix` for Python, TypeScript and C++; C++ lint with clang-tidy** (roadmap 7.2, #205).
  - The linter's own fix mode runs on the node: `ruff check --fix`, `eslint --fix` / `biome
    lint --write`, or `clang-tidy -fix`. The files it rewrote come back into the checkout, each
    is named, and the lint runs again.
  - `lint` on a CMake or Meson C++ project runs clang-tidy over its sources with the
    compilation database. clang-tidy 22 is installed in user space (`uv tool install
    clang-tidy`) on the Linux build nodes.
  - Go's `go vet` has no fixes, and the report says so.
- **impact finds tests by attribute and registration, and has a CI mode** (roadmap 8.1, #201).
  - A caller is a test by its attribute (`#[tokio::test]`, `#[rstest]`, `@Test`), by the gtest
    or Catch2 registration it sits in (`TEST(Price, Doubles)` becomes `Price.Doubles`, which
    `ctest -R` selects), or as a `test*` method of a `unittest.TestCase`. Pytest is given that
    file on the command line, because it would not collect it by name.
  - `prod-code impact --ci` runs the selection, or the whole suite when the selection cannot
    be trusted, and says which and why. It writes a Markdown summary to
    `$GITHUB_STEP_SUMMARY`, and exits with the tests' status.
- **validate takes a diff or a WorkspaceEdit** (roadmap 7.7, #200): `prod-code validate --diff
  PATCH` (`-` for stdin), and `code_validate_edits` with `diff` or `workspace_edit`.
  - Each hunk is applied in memory to the file on disk: at the line its header gives, or at
    the nearest place its old lines are, when the file has moved since.
  - A hunk that fits nowhere is refused by number.
  - A new file is created in the overlay. A deleted file is named, because its users are not
    checked.
  - The touched files are checked together, and nothing is written.
- **An associated function moves to another type** (roadmap 7.1.1, #198): `prod-code
  move-method FILE LINE COL --to-type Order` (MCP `code_move_method` with `to_type`).
  - The type is found by name, and an ambiguous name is refused with the candidates.
  - `Self` in the function is spelled as the old type (`crate::tax::Tax`).
  - The function joins the new type's `impl`, or a new one after the type. An `impl` it
    leaves empty is removed, with the blank line above it.
  - Every path to it, called or used as a value, names the new type.
  - When a name in the body resolves only where the function was, the report says to import
    or spell it.
  The `move` refusal now names both forms: `--to-param` for a method, `--to-type` for an
  associated function.
- **A method moves to the type of one of its parameters** (roadmap 7.1.1, #196): MCP
  `code_move_method` and `prod-code move-method FILE LINE COL --to-param NAME [--apply]`.
  `Order::price_with(&self, tax: &Tax, extra)` becomes `Tax::price_with(&self, order:
  &crate::Order, extra)`:
  - `tax` is the receiver now, borrowed as it was;
  - `self` becomes a parameter in its place, typed as it was borrowed;
  - the body swaps the two, and `Self` is spelled out;
  - the method joins `impl Tax`, or a new `impl` right after the type;
  - calls swap receiver and argument: `o.price_with(t, 1)` → `t.price_with(&o, 1)`, and
    `Order::price_with(o, t, 2)` → `crate::tax::Tax::price_with(t, o, 2)`. `&o` is right even
    when `o` is a reference, because `&&Order` coerces to `&Order`;
  - a call whose receiver or argument does something blocks the write, since the order they
    run in would change, until `force`. So do recursion and a use as a value.
- **safe_delete removes a trait method's parameter everywhere** (roadmap 7.1.1, #194). This
  works on a parameter in the trait or in an implementation (`_unused` counts too):
  - the parameter goes by position from the trait's declaration and from every
    implementation the analyzer lists;
  - every call loses the argument. A method call passes it at that position; a path call
    (`Shape::area(c, …)`) passes the receiver first;
  - nothing is written while a body uses the parameter, while an argument would be dropped
    that does something (a call, a macro, `?`, `.await`), or while the method is used as a
    value. `force` overrides that;
  - the whole change is type-checked first.
  Before, this failed with "its declaration … is no longer in src/lib.rs exactly once". The
  trait's declaration and an implementation had the same text.
- **A whole module moves to another parent** (roadmap 7.1.1, #188): MCP `code_move_module` and
  `prod-code move-module src/a/b.rs --to src/c/b.rs [--verify compile] [--apply]`. `a::b`
  becomes `c::b`:
  - the file moves, with the directory of its submodules (`src/a/b/…` to `src/c/b/…`);
  - `pub mod b;` leaves `a` and is declared in `c` with its attributes and doc comment;
  - every path the analyzer lists as naming the module is spelled anew. A qualified one gets
    the new parent. A bare `b` in the old parent, or `b` in a grouped import, gets its own
    `use`. An import in `c` that would clash with the new declaration is dropped;
  - `super::` in the moved file meant `a`, so it becomes `crate::a::`;
  - the whole change is type-checked in one overlay first, and the old files and the empty
    directory are removed only when it is written.
- **extract_function names the function and replaces the selection's duplicates** (roadmap
  7.1.2, #186): MCP `code_extract_function` and `prod-code extract-function FILE LINE COL --to
  L:C --name NAME`.
  - rust-analyzer's `extract_function` does the selection. The function gets `NAME` instead of
    `fun_name`.
  - Every other place in the same file whose text is the selection's (whitespace aside) gets
    the same call, one at a time. A place is kept only if the result type-checks with it.
    A duplicate left behind is reported with the analyzer's error, for example a name the new
    function does not return but the code after the duplicate reads (E0425).
  - rust-analyzer does not check borrows. So when a duplicate is replaced, `apply` runs `cargo
    check` in a shadow first. `--no-duplicates` extracts the selection alone.
  - The call is read from the text before and after the extraction. The text before the
    selection and the end of the file after the new function must be unchanged, and the rest of
    the function that held the selection must come right before the new one; otherwise no
    duplicate is tried. A line diff cannot be used here: it pairs the selection with the new
    function's body, which repeats it. (An earlier version of this entry said the result was
    checked to rebuild the text exactly. That comparison could never fail, and #192 removed it.)
- **Every exec summary shows the CPU time and peak memory of the command** (roadmap 6.1/6.4,
  #180). The gateway reaps the command with `wait4` and sends `ExecExit.usage`: user and system
  CPU and the largest resident set of the command and its children. `prod-code exec` prints
  `(server 12.3s, cpu 80.1s user 9.2s sys, peak 1450 MB)`, and MCP `code_exec` shows the
  same in its status line. An older gateway sends no usage, and the summary stays as it was.
  - The waiter reaps the command only after `waitid(WNOWAIT)` has seen it exit, under the lock
    the timeout kill takes. So a kill can never reach a process group whose id was reused.
- Benchmarks with parsed results (roadmap 6.4, #178): MCP `code_benchmarks` and `prod-code
  benchmarks [FILTER]`, a fourth kind next to check, lint and test.
  - Rust runs `cargo bench --workspace [FILTER]` and Go runs `go test -run '^$' -bench FILTER
    ./...`.
  - The results are read from criterion (`time: [low estimate high]`, with a long name taken
    from the line before), libtest (`N ns/iter (+/- M)`) and Go (`T ns/op`). Each comes out as
    one line: name, estimate and range.
  - `prod-code bench` is still the gateway's own load benchmark.
- `async` in change_signature (roadmap 7.1.1, #176): `async: true|false` on
  `code_change_signature`, and `--async true|false` on the CLI.
  - `async` is added to the declaration, placed before `unsafe`, or taken away. Every call the
    analyzer lists gains or loses `.await` right after its closing parenthesis, in the same
    type-checked edit. A reference that is not a call is left alone.
  - A call that would `.await` from a function that is not `async` is listed, and it blocks the
    write unless `force` is set. The analyzer also reports that case, as E0728.
  - A request that also changes the order of the parameters is refused.

  On a scratch crate, making `load` async awaited its four calls in one change, and clippy
  passed.
- Rename follows the old name into comments and test names (roadmap 7.1.1, #174): `comments:
  true` on `code_rename`, `prod-code rename … --comments`. In every file the rename touches:
  - the old name is replaced where it stands as a whole word in a comment (`Orders` stays);
  - its snake_case form is replaced between underscores in the name of a test function (one with
    `#[test]`, `#[tokio::test]` or another attribute ending in `test`). `order_total_rounds_up`
    becomes `trade_total_rounds_up`; `reorder_lines` stays.

  The report counts the comment mentions and lists each test renamed, and the whole change is
  type-checked with the rename. A rename that moves files is refused with `comments`, and the
  words around the name are not adjusted ("an `Trade`").
- Extract a delegate (roadmap 7.1.2, #172): `code_extract_delegate` (MCP) and `prod-code
  extract-delegate <file> <line> <col> --fields a,b --methods m,n --name Helper --field helper`.
  rust-analyzer offers delegates for one field at a time. This moves a group:
  - the fields leave the struct for `Helper`, declared after it with the same `#[derive]`, and
    one field `helper: Helper` takes their place, as visible as the widest of them;
  - the methods named move to `impl Helper`. Each keeps a forwarding method with the same
    signature in the struct, so no caller changes. A moved method may use only moved fields and
    moved methods;
  - every other access to a moved field goes through the new field (`a.city` becomes
    `a.address.city`), found through the analyzer's references, in every file;
  - every literal of the struct builds the helper. A literal or pattern with `..` is refused.

  On a scratch crate, `street` and `city`, with `address` and `moves_to`, moved into `Address`.
  Two accesses were rerouted (one of them in another file). Clippy passed, and the same test
  passed before and after.
- Suspects in a failure dossier (roadmap 8.2, #168). `code_diagnose_failure` / `prod-code diagnose`
  now list, for each failing test, the changed functions whose callers graph reaches it. The
  nearest come first, each with the number of calls between it and the test, and with the diff
  of its file when no failure site already shows it. On a scratch crate, `add` and `scale` both
  changed in `src/math.rs`. Only `add` was listed for the failing `doubles` (2 calls away:
  `add` <- `double` <- `doubles`). `impact` now walks from each changed function separately,
  caching the analyzer's answers, and reports which test each walk reached (`reaches`).
- Loops into iterator chains (roadmap 7.1.5, #164): `code_loop_to_iterator` (MCP) and `prod-code
  loop-to-iterator <file> <line> <col>`. A `for` loop whose body only builds up the variable
  declared by the `let mut` just above it becomes one statement:
  - `let mut sum = 0; for p in prices { sum += p * 2; }` becomes
    `let sum: u64 = prices.iter().map(|p| p * 2).sum();`;
  - `if x % 2 == 0 { n += 1; }` into a `usize` becomes `.filter(|&x| x % 2 == 0).count()`;
  - `out.push(..)` into an empty `Vec`, with or without an `if`, becomes `.collect()` (with
    `filter_map` for the `if`).

  The closure binds the loop's pattern as the loop did. A name that holds a reference is
  iterated with `.iter()`: the analyzer's hover gives the type, and clippy flags `into_iter` on a
  reference. `mut` is kept only when the analyzer asks for it (`need-mut`). A loop is refused
  when its body has `break`, `continue`, `return`, `?` or `.await`, uses the accumulator a
  second time, or starts from a value that is not the identity. On a scratch crate, all three
  shapes passed `clippy -D warnings`, and the same assertions passed before and after.
- Prune orphans (roadmap 8.6, #162): `code_prune_orphans` (MCP) and `prod-code prune [--apply]
  [--force]`:
  - everything on the dead-code scan's `dead` list is removed with the analyzer's safe delete.
    Exported symbols and methods a trait may reach are left alone;
  - each answer is reduced to the lines it really changes, and the answers are merged into one
    edit. The engine answers a deletion by replacing the whole file, so without the reduction
    any two deletions in one file overlapped. A deletion that still overlaps another waits for
    the next run;
  - the whole result is type-checked in one overlay, and written in one transactional edit only
    when it is clean (or with `force`).

  On a scratch crate, the first run removed `leftover` and `struct Unused`. That made `helper`
  an orphan, and the second run removed it. The third found nothing, and `cargo check` passed.
- Apply the compiler's own fixes (roadmap 7.2, #158): `fix: true` on `code_check` / `code_lint`
  and `prod-code check --fix` / `lint --fix` (Rust):
  - every suggestion rustc or clippy marks `MachineApplicable` is applied to the checkout in one
    transactional edit, with all of its parts. Other suggestions (`MaybeIncorrect`,
    `HasPlaceholders`) never are;
  - a suggestion that several targets report is applied once;
  - a fix is skipped whole, and the report says why, if it touches a file outside the workspace,
    overlaps a fix already taken, or targets a line that no longer reads as the compiler saw it;
  - the check runs again after the fixes, and the report shows what was fixed, what was skipped
    and what is left.

  On a scratch crate, `lint --fix` removed an unused import, a needless `mut` and a
  `len() == 0`, and clippy then passed. The whole run took 0.72 s.
- Extract a trait from a chosen subset of methods (roadmap 7.1.2, #153): `code_extract_trait`
  (MCP) and `prod-code extract-trait <file> <line> <col> --methods a,b --name NAME`. rust-analyzer's
  `generate_trait_from_impl` takes the whole block, names the trait `NewTrait` and keeps it
  private. Every caller in another module then fails with E0599. This tool does four things:
  - moves only the named methods into `trait Name` and `impl Name for Type`, right after the block.
    The rest stay inherent, and the block goes when nothing is left in it;
  - puts doc comments on the trait's declarations and keeps attributes on the implementation.
    The trait is as visible as the widest moved method;
  - adds `use crate::…::Name;` to every other file that references a moved method, spelled with
    the crate's name from another crate;
  - type-checks every touched file in one overlay before anything is written. Generic `impl`
    blocks and trait implementations are refused.
- Introduce a variable for every occurrence (roadmap 7.1.2, #150): `code_introduce_variable`
  (MCP) and `prod-code introduce-variable <file> <line> <col> --to LINE:COL --name NAME`.
  rust-analyzer's `extract_variable` replaces only the selection. This replaces every whole-token
  occurrence of the expression in the enclosing function:
  - `let w1 = w + 1;` goes above the statement that holds the first occurrence, in the innermost
    block that holds them all, so an occurrence inside an `if` and one after it both see it;
  - the change is refused when evaluating once is not the same as evaluating at each place. That
    is an expression that calls a function or method, expands a macro, uses `?` or awaits. It is
    also a name the expression reads that is assigned, mutably borrowed, rebound or has a field
    assigned between the binding and the last occurrence, or anywhere in a loop that runs a later
    occurrence again;
  - the result is type-checked in the overlay before anything is written.
- Move into a module that does not exist yet (roadmap 7.1.1, #148): `code_move` / `prod-code move
  --to src/util.rs` creates the file and declares it in its parent module, all in the same
  change and the same overlay check:
  - the parent is `src/lib.rs` or `src/main.rs` for `src/util.rs`, and `src/a.rs` or
    `src/a/mod.rs` for `src/a/util.rs`;
  - the declaration is `pub mod util;` for a `pub` item and `mod util;` otherwise, placed after
    the parent's last `mod` line or at the top, and the blank line a cut item left there is
    dropped;
  - a new directory with no module file of its own is refused.

  The declaration is added after the imports are rewritten, because inserting it first shifted
  the positions the analyzer had given and the source file lost its `use`.
- Rename a field with its accessors (roadmap 7.1.1, #146): `code_rename` takes `accessors`,
  `prod-code rename` takes `--accessors`. The methods of the field's struct named after it (`f`,
  `get_f`, `set_f`, `f_mut`, found in the symbol index by name and container) are renamed along
  with the field, with every call:
  - each rename is the analyzer's, computed against the checkout as it is;
  - the resulting whole-file texts are merged into one change per file by a token-level
    three-way merge (`rename_accessors::merge_three`). Identifiers are whole tokens, so two
    renames on one line (`c.set_timeout(c.timeout() * 2)`) merge, and two that would change the
    same identifier differently are refused;
  - the result is type-checked in one overlay before it is written.
- Change a function's return type and visibility (roadmap 7.1.1, #144): `code_change_signature`
  takes `returns` and `visibility`, `prod-code change-signature` takes `--returns` and
  `--visibility`:
  - both are written into the declaration in the same edit as the parameter list. `()` removes
    the return type, a type is added where there was none, and `private` removes the visibility;
  - every file that calls the function now goes into the overlay check (`also_check`), not only
    the files the rewrite touched. A body that no longer returns the new type and a caller that no
    longer fits (`let t: u32 = total(xs)` against `-> u64`) are both reported, and nothing is
    written while one remains.
- Inline a parameter (roadmap 7.1.1, #142): `code_inline_parameter` (MCP) and `prod-code
  inline-parameter <file> <line> <col>`. When every call passes the same argument for a
  parameter, the argument moves into the body and the parameter goes:
  - the body starts with `let max: u32 = LIMIT;`;
  - the parameter leaves the declaration, and its argument leaves every call (method syntax and
    a path call with a receiver are counted correctly);
  - only an argument that means the same in the body is accepted: a literal, an `ALL_CAPS`
    constant, a CamelCase value or a path. A lowercase name may be the caller's local, and the
    analyzer would not necessarily report it unresolved;
  - calls that disagree are listed with their values, and the function used as a value or
    called inside itself blocks the write;
  - the result is type-checked before anything is written.
- Invert a boolean field or local variable (roadmap 7.1.4, #132): `code_invert_boolean` and
  `prod-code invert-boolean` accept a `bool` field or a `let` binding besides a function
  (`crates/prod-code-mcp/src/invert_value.rs`):
  - every read gains a `!` or loses the one it had, and one that goes on (`.then_some(…)`) is
    parenthesized;
  - every write stores the negation: an assignment, the `let` initialiser, a field in a struct
    literal (`!(v)`, `true`/`false` flipped, `!v` unwrapped), a shorthand `S { enabled }` →
    `S { disabled: !enabled }`;
  - a borrow, a compound assignment (`|=`, `&=`, `^=`), a pattern that binds the name, a use
    inside a format string, `#[derive(Default)]` or a serde derive on the struct are reported and
    block the write unless `force`;
  - a local without `: bool` is inverted only when the analyzer's hover says it is `bool`,
    because `!` on an integer compiles as a bitwise not;
  - struct braces are told from a block by the CamelCase type before them, so `if c { flag }` is
    a read and not a shorthand field.
- Convert a function to a method (roadmap 7.1.3, #120): `code_convert_to_method` (MCP) and
  `prod-code convert-to-method <file> --line N --character C`, the other direction of
  `make_static`. An associated function whose first parameter is the `impl`'s own type (`T`,
  `&T`, `&mut T`, `Self`) gets that parameter as its receiver (`self`, `&self`, `&mut self`);
  the parameter's uses in the body, found through the analyzer, become `self`; and
  `Type::f(&mut x, a)` becomes `x.f(a)`, the borrow dropped because method syntax takes it and
  anything but a path or call chain parenthesized. The receiver is evaluated first, as the first
  argument was, so nothing is reordered. The function used as a value and a call inside the
  function itself stay as they are, still valid through the path. A trait impl's function and a
  free function are refused. Type-checked in one overlay; `verify: "compile"` adds `cargo check`.
- Type migration writes conversions (roadmap 7.1.4, #119): `code_migrate_type` takes `convert`,
  `prod-code migrate-type` takes `--convert`. At every site where the old and new types meet
  (an E0308 naming both, on one line) `.into()` is written — parenthesized unless the expression
  is a path, a call chain or a literal — and the overlay is type-checked again. A conversion whose
  line still has an error is taken back and its site is reported as tried; the rest are checked
  again, up to four rounds. If the kept conversions cause an error anywhere that was not in the
  original report, none is kept and the report says why. `apply` writes the declaration with the
  kept conversions. On a scratch crate, `u32` → `u64` converted the two widenings and left the
  four narrowings, and `String` → `Box<str>` converted both sites, with `cargo check` agreeing in
  both cases. Diagnostics now carry the end of their range internally. The analyzer's range for
  a method call is the method's name alone, so it is extended over the argument list. Type names
  are compared as the analyzer spells them: path prefixes at any depth are stripped, which also
  fixes `Vec<std::string::String>`, and the `Global` allocator argument is dropped.
- The scripted test gateway shows notifications (`didOpen`, `didChange`) to its script, so a
  test can answer diagnostics from the text it was actually sent.
- Make a parameter generic (roadmap 7.1.4, #117): `code_generify` (MCP) and `prod-code generify
  <function> --param NAME --bound TRAIT [--as T]`. The parameter's type becomes a type parameter
  with the given bound — `fn total(v: &Vec<u32>)` becomes `fn total<T: AsRef<[u32]>>(v: &T)` — with
  the reference kept and the new parameter appended to any generics the function already has. A
  name already in use, an `impl`/`dyn` type and a missing parameter are refused. Callers are not
  edited, since the type argument is inferred, but every file that calls the function is
  type-checked with the new signature in one overlay, and an error stops the write unless `force`:
  a body that uses more than the bound promises, or a caller that no longer compiles as it is,
  such as `label("all".into())` once `label` takes a `D: Display`.
- Invert a predicate (roadmap 7.1.4, #115): `code_invert_boolean` (MCP) and `prod-code
  invert-boolean <file> --line N --character C --to NEW`. A function returning `bool` gets the new
  name and returns the negation of what it returned — in place for a one-expression body, as a
  block otherwise, and at every `return` of the function (not of a closure or a nested `fn`).
  Every call becomes `!new(…)`, a call that had a `!` loses it, and a call followed by `.`, `?` or
  an index is parenthesized. A reference that is not a call is named, since under the new name it
  would mean the opposite; a recursive predicate is refused. Type-checked in one overlay.
- Make a method static (roadmap 7.1.3, #111): `code_make_static` (MCP) and `prod-code
  make-static <file> --line N --character C`. A method whose body never mentions `self` loses its
  receiver; `value.method(args)` becomes `Type::method(args)` and `Type::method(value, args)`
  loses its first argument. A receiver that does something when it is evaluated — a call, `?`,
  `.await`, a macro, an index — is not dropped silently: the call site is reported and nothing is
  written while one remains, unless `force`. A method that uses `self` is refused. Type-checked in
  one overlay; `verify: "compile"` adds `cargo check`.
- Wrap a return type with its callers (roadmap 7.1.4, #109): `code_wrap_return` (MCP) and
  `prod-code wrap-return <file> --line N --character C --wrapper option|result [--error TYPE]`.
  rust-analyzer's `wrap_return_type_in_option` / `_in_result` rewrite the signature and every
  returned value, and no caller; this fills the `Result`'s `_` error type with `error`, adds `?`
  at every call whose caller already returns the same wrapper, and reports every other caller
  with its line — turning a `None` or an error into something else there is a decision. Calls
  later in the declaring file are mapped through the assist's rewrite; a recursive call is left
  for a person. Nothing is written while a caller is blocked, unless `force`; the result is
  type-checked in one overlay, and `verify: "compile"` adds `cargo check`.
- The CLI reaches what the MCP tools already did (#93). `def`, `hover`, `refs`, `callers`,
  `callees` and `impls` take `--symbol NAME` instead of `<file> <line> <col>`.
  `prod-code symbols <name>` searches declarations by name; `symbols <file>` still outlines an
  existing file, and `prod-code outline <file>` does so by its own name. `prod-code validate
  FILE --from NEW --with OTHER=NEW2 …` checks several proposed files together in one overlay,
  so a constant added in one file and re-exported in another is not reported as unresolved.
  `change-signature` has its own help line, which had been printed under `migrate-type`.
- Extract a field (roadmap 7.1.2): `code_extract_field` (MCP) and `prod-code extract-field
  <file> <line> <col> --to LINE:COL --name <field> --type <T>` promote an expression inside a
  method into a field of the type the method belongs to. The method reads `self.<field>`
  (`replace_all` for every identical occurrence), the struct declares the field last, and every
  place that builds the struct — `Type { … }` anywhere in the workspace and `Self { … }` in its
  `impl` blocks — initialises it, by default with the expression itself and otherwise with
  `init`, which is required when the expression reads `self`. A return type, an import or an
  `impl` header that names the type is not a construction site; a pattern ending in `..` is
  left alone; a pattern that lists every field is reported with its line and blocks the write.
  The change is type-checked in one overlay, and `verify: "compile"` adds `cargo check`.
- Encapsulate a field across the workspace (roadmap 7.1.3): `code_encapsulate_field` (MCP) and
  `prod-code encapsulate-field <file> --line N --character C` make a public field private and
  rewrite every access to it outside its declaring file — a read into `x.field()`, a plain
  write into `x.set_field(v)`. The getter returns the value for a primitive `Copy` type and a
  shared reference otherwise (`by_value` overrides); the setter is generated only when
  something writes the field; both go into the struct's first inherent `impl` with the field's
  old visibility, or a new `impl` after a non-generic struct. Accesses inside the declaring file
  stay direct. A use that cannot become a method call — a struct literal or pattern outside the
  file, a compound assignment, `&mut x.field` — is reported with its source line, and nothing
  is written while one remains; a position that does not hold the field's name is reported and
  left alone. The whole change is type-checked in one overlay, and `verify: "compile"` adds
  `cargo check` in a shadow, which is the only check that sees a borrow the getter no longer
  allows. The bracket matcher the write tools share now skips comments, so an apostrophe or a
  brace in a comment inside an `impl` does not end the block early.
- Change a declared type and see the whole job first (roadmap 7.1.4): `code_migrate_type` (MCP)
  and `prod-code migrate-type <file> --line N --character C --to <Type>` rewrite the declaration
  in memory — a struct field, a parameter, a return type or an annotated `let` — type-check the
  workspace in one overlay, and report every site the new type does not fit, grouped by file
  with the line of source at each. Where an error is exactly the old type meeting the new one,
  the report says what conversion would fix that site; it does not write it. Diagnostics that
  land on a `#[derive(…)]` line are counted separately, because an error inside what a derive
  generates is reported at the derive and there is nothing at that position to edit. `apply` writes the
  declaration alone and refuses while any site remains. This is the first half of a migration,
  not an automatic one, and says so.
- Promote an expression to a parameter (roadmap 7.1.2): `code_extract_parameter` (MCP) and
  `prod-code extract-parameter <file> <line> <col> --to <line>:<col> --name limit` take an
  expression out of a function body and make it a parameter, passing what the body used to say
  at every existing call site — so no current caller changes behaviour and the next one can
  choose. The parameter is added at the end of the list, keeping the list's shape; the type is
  the analyzer's where it gives one in a readable shape and the caller's otherwise;
  `replace_all` puts the parameter in every identical occurrence inside the body. A reference
  that is not a call with this arity is named rather than mangled, and an expression that names
  a local or anything private to the function is refused with that as the reason rather than
  with a raw diagnostic.
- Bundle parameters into a struct (roadmap 7.1.2): `code_introduce_parameter_object` (MCP) and
  `prod-code parameter-object <symbol> --param a --param b --name Opts` take several of a
  function's parameters and make them fields of a new `pub struct` written directly above it,
  in declaration order and with the types the declaration gave — a single lifetime is introduced
  when any of those types borrows. The declaration takes one parameter in place of them, every
  use of them in the body is rewritten to reach through it at the positions the analyzer
  reports, and every call site is rewritten in place: the bundled arguments become one struct
  literal where the first of them was, the others stay where they were, and an argument that is
  a closure, a method chain or a string containing a comma survives. A call site in another
  module of the same crate gets the import; one in a file that is not a module of the crate at
  all, such as a test, names the type in full instead. A use that is not a call with this
  arity is named rather than mangled. The whole change is type-checked in one overlay before
  anything is written.
- Move a declaration to another module (roadmap 7.1.1): `code_move` (MCP) and
  `prod-code move <symbol> --to <file>` take a function, struct, enum, trait or const out of one
  module and put it in another, with the imports that keep every user of it compiling. The item
  travels whole — signature, body, doc comment, attributes — and takes with it the `use`
  statements it actually spells, narrowed to the names it needs. Every file the analyzer lists
  as using it has its import rewritten (a grouped import keeps its other names) and any
  path-qualified reference requalified; a file that spelled the name bare gets the new import,
  one that only ever qualified it gets none. The positions the analyzer reported are adjusted
  for the hole the cut leaves in the file the item left, which is what makes the source file's
  own references find their new import. The whole change is type-checked in one overlay before
  anything is written, so a move that reaches for something private to the module it left is
  reported — with that explanation — rather than written. Nothing is written without `apply`.
  Rust only; the target module must already exist.
- `PROD_CODE_TIMING=1` now reports the work every invocation does *before* the query —
  `[timing] startup total=… discover_nodes=… workspace_identity=… engine_project=…
  pick_node=…`. The query timer started after all of it, which is why #56 could report a
  half second that no phase accounted for.

## v0.2.2 — 2026-09-22

Seven new tools since 0.2.1 — search by intent, slice a symbol's dependencies, try several
fixes at once, rewrite code structurally, build a fixture, change a signature with its call
sites, rename a schema field across languages — and the test suite that holds them: every file
in the workspace is at or above 80% of regions, up from 53%.

### Added
- Cross-language schema rename (roadmap 7.6): `code_schema_rename` (MCP) and
  `prod-code schema-rename <field> --to <new>` rename a schema field across every language
  that spells it — `order_id` in the `.proto` and in Rust, `OrderID` with a `json:"order_id"`
  tag in Go, `orderId` in TypeScript, the column in the SQL. All spellings (snake, camel,
  Pascal, Go's initialism form, SCREAMING, kebab) are found by a whole-word scan, which is
  discovery only; every identifier is then renamed by the analyzer of its own sub-project, so
  the change follows the symbol into files the scan never looked at, and only what no analyzer
  owns — schema files, and the name inside string literals — is edited textually at the
  positions that were found. Two renames that want the same characters are never merged: the
  second is skipped and reported. The result is type-checked per project before anything is
  written. A test bed is in `fixtures/polyglot-order`.
- Change signature (roadmap 7.1.1): `code_change_signature` (MCP) and
  `prod-code change-signature <fn> --param …` change what a function takes, with its call
  sites. `params` is the list the function should end up with — `name` keeps a parameter,
  `name: Type = expression` adds one and passes `expression` at every call site, anything not
  listed is removed. The arity and the types come from the declaration, so the structural rule
  that rewrites the call sites is built rather than guessed, and it is resolved in the
  declaring file's own scope, so calls match however they are spelled. What was rewritten is
  reconciled against the analyzer's reference list and anything it did not touch is named;
  dropping a parameter the body still uses is refused with the usages; and the declaration and
  every call site are type-checked together in an overlay before anything is written. Rust
  only.
- Fixture generation (roadmap 8.5): `code_generate_fixture` (MCP) and `prod-code fixture
  <Type>` build a compile-ready value for a type from the declaration the analyzer resolves the
  name to, filling every field by type, recursing into types declared in the workspace down to
  `depth` and falling back to `Default::default()` beyond it. The fixture is then type-checked
  in an in-memory overlay of the file that declares the type, so a missing field or a type
  without `Default` comes back as the analyzer's error rather than as a failed build. Nothing
  is written. Rust only.
- Structural codemod (roadmap 8.7): `code_codemod` (MCP) and `prod-code codemod
  "pattern ==>> replacement"` rewrite code on the syntax tree through rust-analyzer's own SSR
  engine, with `$name` placeholders bound by the match. A call split over three lines matches,
  a comment that looks like the pattern does not, and paths are resolved rather than compared
  as strings. The result is a unified diff of what would change; `apply: true` writes it.
  Rust only, and not interactive: the search resolves usages across the workspace, so a call
  takes tens of seconds on a warm engine and minutes on a cold one.
- Intent search (roadmap 8.4, first step): `code_search` (MCP) and `prod-code search "..."`
  find code by what it does when you do not know what it is called. The gateway indexes every
  declaration in the workspace copy together with the doc comment above it, its signature and
  its container, and ranks them with BM25 over those fields (name weighted highest) against the
  words of the question. Declarations belonging to tests are excluded unless the question is
  about tests, because a test's name repeats every word of the thing it tests. Lexical, not
  embeddings: the dense half of 8.4 is still open. The index is built on the first query and
  then kept current by the sync layer telling it which files it wrote, so a query never walks
  the tree. Measured: on a 1000-declaration repository a question answers in 2-42 ms; on a
  26712-declaration, 1051-file Go repository the first query costs 568 ms (the index build) and
  every later one 33 ms.
- Program slicing (roadmap 7.3): `code_slice` (MCP) and `prod-code slice` return only the code
  a symbol depends on. From the seed declaration the analyzer's own edges are followed, the
  functions it calls and the types, constants and traits its body mentions, each returned as a
  whole declaration with its file and line range; `depth` bounds the walk and `max_bytes` the
  result. Names resolving outside the workspace are listed, not expanded. Measured on this
  repository: a 31-line function's slice is 1.8 kB against 21 kB of source (92% smaller, 1.2 s);
  the gateway's shadow-run entry point is 11.5 kB against 284 kB across two files (96% smaller,
  1.3 s). No new wire message: it is built from documentSymbol and definition queries.
- Shadow runs (roadmap 7.4, second step): `code_shadow_run` / `prod-code shadow-run` run a
  command once per named hypothesis (complete proposed file contents) in a private shadow of
  the server workspace. On Linux a shadow is an overlay mount at the workspace's own path
  inside a user namespace, so warm build caches stay valid and hypotheses run in parallel;
  without user namespaces they run one at a time in place with the files restored. Every
  hypothesis reports exit code, parsed test counts and output tail; the outcomes are ranked
  (passed, fewest failures, most passed, smallest diff) and the winner comes back as a
  unified diff (`apply: true` writes it). Gateway `--shadow-dir` places the upper
  directories; leftovers are swept at start.

## v0.2.1 — 2026-09-20

### Added
- `code_validate_edits {edits: [{path, text}], also_check}`: several proposed files are
  validated together in one in-memory overlay, plus any extra files to check. When an edited
  file drops or renames a symbol, errors in other files that mention it carry a note naming the
  removed or renamed symbol and the file it vanished from; rust-analyzer stays silent on a
  qualified call to a function that no longer exists, so a `prod-code::stale-reference`
  warning is synthesised on that line.
- MCP hot reload: `prod-code mcp` polls its own binary every 3 s. When the installed file
  changes it finishes the in-flight request, sends `notifications/tools/list_changed`,
  re-executes itself with the same arguments and environment (`initialize` declares
  `tools.listChanged`), and the resumed process sends the notification again, so a running
  agent session gets the new tools and schemas without a restart.
- Gateway `--engines rust,go,cpp` allowlist: a node advertises and serves only the listed
  engines and refuses handshakes for the others, so a macOS node can be Swift-only and
  placement never sends Rust work to a workstation.
- `PROD_CODE_TIMING=1` prints the client's per-phase timing (connect, sync, handshake, query)
  to stderr; `divergent-bench --persistent` reports the same phases.
- Symbol-addressed queries: every position tool (`code_definition`, `code_references`,
  `code_hover`, `code_callers`, `code_callees`, `code_implementations`, `code_rename`,
  `code_safe_delete`, `code_assists`, `code_assist`, `code_type_at`) accepts `symbol`
  (`Metrics::record`, `pkg.Func`, `Class.method`) instead of `path`/`line`/`character`; the
  name is resolved through the analyzer's workspace symbol index (`workspace/symbol`, served
  in-process for Rust, forwarded for the LSP engines). Ambiguous names list the candidates.
- `code_symbols {query}`: workspace symbol search by name with file:line:col and container.
- `code_test` / `code_check` / `code_lint` with `path` narrow to the Cargo crate
  (`-p <name>`), Go package tree (`./dir/...`) or pytest path containing it.

### Fixed
- The engine of a checkout whose manifest sits one directory below the root is detected from
  that child (`project/go.mod`, `server/Cargo.toml`), as long as every child with a manifest
  agrees; a polyglot monorepo still resolves to "any engine".
- A workspace whose engine is unknown is no longer placed on a node that advertises a single
  engine. A Swift-only macOS node used to qualify for it, and the work failed there.
- Position tools' MCP schemas declare the `symbol` parameter (the tools accepted it, agents
  could not see it). `code_outline` hides local variables unless `include_locals` is set and
  `max_depth` limits nesting.
- A worktree is placed on the node that holds its origin repository (placement is keyed by the
  origin checkout), so the gateway can seed the worktree copy from the origin's files.
- Diagnostics reports (`code_diagnostics`, `code_validate_edit(s)`, `prod-code diagnostics`)
  drop rust-analyzer's `inactive-code` hints: code behind an inactive `cfg` is not an error.
- Worktree copies keep their own `target/` directory: no shared cargo state and no shared
  build lock between worktrees. The first load of a new worktree runs its build scripts once.
- Go engine is advertised only when both `gopls` and `go` are on the gateway's PATH (gopls
  without the go tool answers "no views"); the third Linux node got a Go toolchain.
- `GoEngine::document_symbols` surfaces gopls errors instead of returning an empty list.
- One node ran Ubuntu clangd 18, whose `workspace/symbol` reports header symbols under the wrong
  file; all Linux nodes now run clangd 22.1.6 from `~/.local/clangd`.

### Changed
- `TCP_NODELAY` on every gateway connection (client connect, gateway accept, gossip). Nagle
  plus delayed ACK stalled half of the didOpen→hover rounds by 32–43 ms; the server round trip
  is now p50 ~1 ms.
- Development process: every change is an issue and a pull request with the commands that
  reproduce and verify it (`CONTRIBUTING.md`); checks run on the build nodes, there is no
  hosted CI.
- Gateway channels moved to [`rapidfire`](https://github.com/alex09x/rapidfire) (zero-dependency
  MPSC): the per-session outgoing queue is drained in batches of 64 with one socket flush per
  batch, exec stdout/stderr chunks fan in through a bounded rapidfire channel, and metrics
  events are appended to disk by a background writer (`recv_many` batches of 256) instead of on
  the response path. Broadcast channels (engine notifications) stay on tokio.

- Usage metrics: every query, exec and sync round on a gateway is one event (agent —
  claude-code / codex / cli — client host and address, workspace, engine, method, file,
  position, duration, ok, item count), appended to `<storage>/../metrics/events-YYYY-MM-DD.jsonl`
  and summarised by `prod-code metrics [--since SECS] [--json]` across the cluster (per agent,
  host, workspace and method with p50/p95, exec runs with failures, sync volume).

- The MCP server keeps one gateway session per checkout for the life of the process: a tool
  call is one request instead of connect + sync + handshake + initialize (20 hovers: 2.8 s →
  0.6 s, ~10 ms each after the first). Local edits are pushed over the same connection before
  each call and open documents are updated; a dead connection is replaced transparently.
- The MCP server sends agent instructions at `initialize` (navigate semantically, validate
  before writing, build and test on the gateway, impact and diagnose).
- `prod-code status` probes the named node.

## v0.2.0 — 2026-09-20

Second release: every language, a real cluster, and the agent tools that make prod-code more
than a fast LSP. Since v0.1.0:

- Cluster (Phase 5 complete): gateways gossip every 5 s (`--peers`, `--advertise`) and every
  node knows the whole cluster; one seed address in `PROD_CODE_REMOTE` is enough, the client
  discovers the rest and caches it. Placement is decided by the cluster: the node that holds
  the workspace, else the quietest live node with the right engine; idle workspaces move off
  overloaded nodes. `prod-code cluster` shows the gossip view. The Rust engine now runs build
  scripts and expands proc macros (rust-analyzer's proc-macro server), so derives resolve.

- Failure dossier (Phase 8.2): `prod-code diagnose [FILTER]` and MCP `code_diagnose_failure`
  run the tests and explain each failure with the code at every mentioned location, the
  enclosing function and its callers, and what changed in the working tree.

- In-memory diagnostics and edit validation (Phase 7.7): `prod-code diagnostics <file>` and
  `prod-code validate <file>` (MCP `code_diagnostics`, `code_validate_edit`) report what the
  analyzer thinks of a file, or of a proposed new content, without a build and without
  writing: rust-analyzer diagnostics from the in-memory database, pull or published
  diagnostics from the managed servers. Type errors, unresolved names and hallucinated APIs
  are caught in well under a second on every language.

- Dead-code scan (Phase 8.6): `prod-code dead-code` and MCP `code_dead_code` list unreferenced
  functions, methods and types found through the analyzer, skipping tests and entry points and
  bucketing exported symbols and trait/interface methods separately. Batch features (impact,
  dead-code) run on one persistent gateway session instead of a connection per query.
- Rust document symbols carry their enclosing items (`containerName`: `tests`, `impl Shape for
  Circle`).

- The Rust engine analyses `cfg(test)` and `debug_assertions` code like rust-analyzer's IDE
  defaults, so `#[test]` functions exist in the call graph; callers are flagged as tests by
  the analyzer. Document symbols point at the item's name and carry its full extent.
- A synced project manifest (tsconfig, package.json, pyproject, CMakeLists, Package.swift,
  go.mod, Cargo.toml, prod-code.toml ...) restarts the workspace's engines on the next session.

- Impact analysis (Phase 8.1): `prod-code impact` (and MCP `code_impact`) lists the functions
  the working-tree diff touches, the callers that reach them through the call hierarchy and
  the affected tests, and emits (or with `--run` executes) the command that runs only those
  tests. Rust document symbols now carry their full extent.

- Definitions outside the checkout are readable (Phase 8.3): `prod-code def` shows the lines
  around a definition in the standard library, a dependency cache or a system header, and
  `prod-code source <path>` prints any such file from the gateway host; MCP `code_definition`
  embeds the snippet and `code_source` reads the file. Only toolchain, dependency and SDK
  roots are served.

- Code actions for every language (Phase 7.2): `prod-code assists | assist` and MCP
  `code_assists` / `code_assist` on Go, C/C++, TypeScript, Python and Swift through LSP code
  actions, including quick fixes driven by the server's diagnostics and command-backed
  refactorings (clangd extract-to-variable).
- Monorepos (Phase 3.1): a nested project of another language gets its own engine, its own
  placement (Swift package in a Rust repo lands on a macOS node) and its own `check` / `test`
  / `exec` working directory; `prod-code exec` runs where it was typed.

- Project tooling is detected per checkout for `check | lint | test`: TypeScript uses the
  package manager of the lock file (bun, pnpm, yarn, npm) and the configured test runner
  (vitest, jest, bun test, mocha, or the `test` script) with parsed results; Python runs
  through `uv run`, the checkout's `.venv`, or the system interpreter, with pytest or unittest
  and basedpyright pointed at the venv; C/C++ builds with CMake, Meson or Make and tests with
  ctest (after a build) or meson test. Rename now reaches every referencing file on pyright
  (files are opened for the duration of the rename), clangd (CMake is configured with
  compile_commands.json before clangd starts) and sourcekit-lsp. `exec` never pulls back
  virtual environments, node_modules or build directories.
- All six languages verified end to end on fixtures: hover / definition / references /
  symbols / callers / callees / implementations / rename / check / lint / test; Go on gopls
  (cross-file rename included), Rust in-memory.

- Per-repository Rust analysis options in `prod-code.toml` (`[rust] features = "all" | [..]`,
  `no_default_features`, `all_targets`, `sysroot`), with rust-analyzer-like defaults: all
  targets analysed and the standard library loaded from `rust-src`. Repositories that compile
  one module tree into several crates behind feature flags (BTCR's `src/strategy2`) need
  `features = "all"`, otherwise those modules resolve to nothing.
- Sync ships `rustc-wrapper` scripts and `*.sh`, and the gateway keeps the executable bit, so
  `cargo metadata` works on a workspace whose `.cargo/config.toml` sets `build.rustc-wrapper`.
  Verified on BTCR: 109 implementations of `StrategyInterface`, 235 references, callers with
  call sites, where before only syntax-level queries answered.

- Call hierarchy and implementations (Phase 7.5): `prod-code callers | callees | impls` and MCP
  `code_callers` / `code_callees` / `code_implementations` for every engine (rust-analyzer
  in-memory, gopls, clangd, native TypeScript, basedpyright, sourcekit-lsp), with call sites.
- Rust document symbols report their real kinds and lines (all were `Variable (line 1)`).
- Second macOS node: a MacBook Pro (Xcode 15.4 with iOS simulators, live GUI
  session) runs a gateway for Swift and Xcode UI tests.

- Sync watermarks are kept per gateway node: a checkout placed on a second node (or moved by
  failover) is uploaded to it in full instead of receiving an empty delta computed against the
  first node. An empty delta is still sent, so a node whose workspace copy was pruned answers
  "fresh" and the client resyncs before the query or `exec` runs. Files rewritten by the client
  for rename / assists / safe-delete are no longer recorded as synced (the gateway only computed
  those edits); the next sync uploads them, so hover after rename sees the new code.
- A third Linux node (Ryzen 9 7950X) joined the cluster with all Linux
  engines; a Mac Studio is the macOS node for Swift.

- Language engines (Phase 3.4–3.6): C/C++ (`clangd`), TypeScript (native TypeScript 7
  `tsc --lsp`, fallback `typescript-language-server`) and Python (`basedpyright`) workspaces
  get hover, definition, references and document symbols through the gateway; `prod-code
  check | lint | test` run `cmake --build` (configuring the build dir first), `tsc --noEmit` /
  `eslint` / `npm test`, `basedpyright` / `ruff` / `pytest`, with parsed diagnostics. Build and
  tool manifests (`CMakeLists.txt`, `compile_commands.json`, `.clangd`, `requirements*.txt`,
  `pytest.ini`, `tox.ini`, `Pipfile`, Bazel `BUILD`, `project.pbxproj`, ...) are now synced.
  Swift (Phase 3.7) runs on a macOS gateway node (`sourcekit-lsp` from Xcode): hover,
  definition, references, symbols, `swift build` diagnostics and `swift test` results (XCTest
  and swift-testing parsed). `cpp test` parses ctest output. Diagnostic paths are relative to
  the checkout instead of the server copy.
- Engine-aware placement (Phase 5.1): gateway status lists the engines whose language
  server is actually installed on the host; the client places a checkout only on a node that
  serves its engine, re-places a remembered node that no longer fits, and `prod-code cluster`
  shows each node's engines.
- MCP tools open files with the languageId of their extension (was always `rust`); LSP symbol
  kinds are named correctly in outlines.

- Session churn stress (Phase 5.5): `divergent-bench --persistent --churn N` kills N% of
  sessions mid-run without a goodbye and verifies the gateway retires them all.
- `prod-code check | lint | test --json` print the full structured report.

- Safe delete (Phase 7.1.1): `prod-code safe-delete <file> <line> <col>` and MCP
  `code_safe_delete` remove an item only when rust-analyzer finds no references to it in the
  workspace; otherwise the usages that block the deletion are listed.

- Load-aware placement (Phase 5.3 first step): gateways report host load and CPU count in
  their status; the first placement of a checkout picks the quietest alive node.

- Multi-gateway placement (Phase 5.1, client side): `--remote` / `PROD_CODE_REMOTE` take a
  comma-separated node list; a checkout is placed by rendezvous hashing, remembered locally,
  and fails over to the next alive node. `prod-code cluster` shows node status and placement.

- Code actions (Phase 7.1): `prod-code assists <file> <line> <col> [--to LINE:COL]` lists the
  rust-analyzer assists at a position or selection, `prod-code assist … <id> [--subtype N]`
  applies one; MCP tools `code_assists` / `code_assist`. Inline, extract function/variable/
  constant, promote to const, add explicit type, generate and rewrite assists and quick fixes
  all go through the same WorkspaceEdit path as rename.

- Typed remote verification (Phase 6.4): `prod-code check`, `prod-code lint`,
  `prod-code test [FILTER]` and MCP tools `code_check`, `code_lint`, `code_test`. The command
  runs on the gateway and the client parses the output into structured diagnostics
  (`error: [E0308] ... (src/lib.rs:12:5)`), pass/fail counts and per-failure output for Rust
  (cargo JSON, rustc text, libtest) and Go (`go build`/`go vet` lines, `go test -json`).

- Semantic rename (Phase 7.1.1): `prod-code rename <file> <line> <col> <new_name>`, MCP tool
  `code_rename`, and LSP `textDocument/rename` on the gateway. rust-analyzer computes the
  workspace-wide edit (including module file renames); the client applies it to the checkout
  and records the rewritten files in the sync watermark. Refused renames (no symbol, conflicts)
  are reported as errors instead of empty results.

## v0.1.0 — 2026-09-19

First release of prod-code, the Remote Code Intelligence gateway: one warm, in-memory
analysis server on the LAN that a fleet of AI coding agents and thin clients query instead of
each running its own language server and build on a laptop.

### Gateway and engines
- `prod-code-server`: TCP gateway with a JSON wire protocol, multi-tenant workspace manager
  with leader/follower loading, per-session views and path translation.
- In-memory Rust engine on `ra_ap_ide` (rust-analyzer as a library): the Cargo workspace is
  loaded once into a Salsa database; hover, definition, references and document symbols run
  from RAM in 1–4 ms server-side. Live buffers are applied straight into the database; a
  re-open with identical text is a no-op and keeps the caches warm.
- Per-session buffer overlays: concurrent sessions on one workspace each see their own
  unsaved edits; queries run under the engine lock together with view activation, so a
  concurrent edit can no longer cancel an in-flight query into a null result.
- Managed Go engine (gopls) and a generic LSP engine for other languages; engine kind is
  detected from the workspace manifest and reloaded if the detected kind changes.
- Janitor: engines idle for 30 minutes are unloaded (`--idle-evict-secs`), worktree workspace
  copies unused for 7 days are pruned (`--prune-worktree-days`); child language servers are
  killed with their engine; `~/.cargo/bin` is put first on PATH.

### Isolated workspace per git worktree
- Every git worktree identifies itself as `<origin>--wt-<hash>` and gets its own server
  workspace and analysis database; the main checkout keeps its own. Diverged worktrees can no
  longer see each other's edits (the shared mode remains available in the benchmark as a
  diagnostic).
- First contact sends a manifest (path, size, FNV-1a hash) instead of the tree: the gateway
  seeds a new worktree from the origin repository's copy, deletes what the client does not
  have and asks only for missing files. A fresh BTCR worktree: 0.4 s instead of ~7 s.

### Sync
- Watermark-based incremental sync per worktree: commits since the last sync
  (`git diff <base>`), dirty and untracked files, reverts of previously dirty files, and lock
  files; `git diff` is skipped when HEAD has not moved. Persistent state lives under
  `~/.local/share/prod_code/sync/`, versioned with the relevance filter.
- Pre-flight sync runs before the handshake so a new workspace directory is populated before
  engine detection; a client whose watermark disagrees with the gateway (reset server
  directory) self-heals with a full resync.
- File content travels as base64: a 10.8 MB workspace syncs in 0.76 s over 10G instead of 8 s.
- The MCP server watches the workspace tree and runs the pre-flight sync only after a change.

### Remote build and test execution (Phase 6 foundation)
- `prod-code exec -- <argv>` and the MCP tool `code_exec` run a command on the gateway inside
  the checkout's server copy, stream stdout/stderr back and return the remote exit code.
  Build artifacts stay on the server per workspace, so every worktree keeps a warm cache.
- Files the command creates, changes or deletes (formatters, generators, lockfiles) are
  written back into the checkout and recorded in the watermark. Commands run in their own
  process group and are killed on timeout or client disconnect.
- Measured on the prod-code repository itself: workspace clippy plus all crate tests in
  5.8 s on the 32-core gateway host, nothing compiled on the developer's machine.

### Clients
- `prod-code` CLI: `status`, `sync`, `hover`, `def`, `refs`, `symbols` (1-based positions),
  `lsp` (stdio bridge), `mcp`, `exec`, `bench`, `divergent-bench`; `PROD_CODE_TIMING=1` prints
  per-phase timings of a query.
- Native MCP server with `code_definition`, `code_references`, `code_outline`, `code_hover`,
  `code_status`, `code_sync`, `code_exec` for Claude, Codex and other agent frameworks.

### Benchmarks
- `bench`: persistent pipelined sessions; 40k hover/s at p50 1.4 ms, p99 3.5 ms against a
  warm Rust workspace.
- `divergent-bench`: forks four worktrees of a real repository (signature change, manifest
  change, untracked file with a new symbol), runs 10+ concurrent workers and asserts zero
  cross-worktree bleed; `--persistent` reuses one session per worker like an agent process.
  Isolated mode passes on BTCR (Rust) and CodeHaus (Go) with zero errors.

### Known limitations
- Shared (coalesced) workspaces are diagnostic only; production isolates worktrees.
- A one-shot CLI query costs ~80 ms, of which ~70 ms are two `git` subprocesses on the
  client; long-lived agents avoid this through the MCP server's change watcher.
- Cluster features (Phase 5), C/C++, TypeScript, Python and Swift engines (Phase 3.4–3.7) are
  not implemented yet; see ROADMAP.md.
