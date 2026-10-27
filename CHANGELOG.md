# Changelog

## [Unreleased]

### Fixed
- **Pulling a File Takes the Node's Executable Bit Both Ways** (`crates/prod-code-mcp`, #1477):
  A file the node reports as not executable, pulled over a local executable copy, kept its executable bits, because only `true` was applied. The executable bits are now cleared, and the other permission bits are kept. A shebang has no say, as before.
- **The Divergent-Worktree Benchmark Checks Every Copy and Seeds From the Whole Origin Name** (`crates/prod-code-client`, test, #1522):
  - Each benchmark worker queries one copy. With fewer workers than `--worktrees` some copies were never asked, and their kind still passed on the other copies. Such a run is now refused, and the report fails for any copy that gave no successful response.
  - A copy's warm seed is taken from everything before its last `--wt-`, so an origin whose own name holds `--wt-` is no longer cut short.
- **Direct CLI Queries Warn When the Server Was Still Indexing** (`crates/prod-code-client`, #1520):
  A direct query such as `prod-code impls FILE LINE COL` dropped the gateway's `prod-code/indexing` note, so a partial list, or "No implementations found", gave no hint that the server had not finished indexing. The note is now printed to stderr as a warning that the answer may be incomplete. The answer itself and the response matching are unchanged.
- **`prod-code` Without a Subcommand Refuses at `initialize` Like `prod-code lsp`** (`crates/prod-code-client`, #1519):
  `prod-code` with no subcommand runs the editor bridge, but when no gateway could be placed it exited before the editor's `initialize`. Only an explicit `prod-code lsp` gave the structured refusal. Both now answer `initialize` with the reason.
- **The Editor Bridge Ends Cleanly After the Editor's `exit`** (`crates/prod-code-client`, #1521):
  Once the editor had sent `exit`, the language server leaving and the gateway closing the session were treated like a broken connection: exit 1, or a new session with `--reconnect`. The bridge now notes `shutdown` and `exit`, and a closure after `exit` ends it with 0 when `shutdown` came first and 1 when it did not, as the protocol has a server exit. An unexpected closure is still reported or reconnected.
- **A Save Whose File Did Not Reach the Node Waits Before the Server Sees It** (`crates/prod-code-client`, #1517):
  When the sync before a `textDocument/didSave` failed, the bridge warned and forwarded the save anyway, so check-on-save ran against the node's old copy, and nothing checked again once a later sync brought the new text. Such a save is now held. It goes out, in order, right after the next sync that succeeds. While a save waits, other editor traffic retries that sync at most once a second, and at most 64 saves are held.
- **Search Reindexes Unix Files Whose Names Look Like a Drive** (`crates/prod-code-gateway`, #1459):
  On Unix, a file such as `a:module.rs` was indexed at first, but every later change to it was dropped as if `a:` were a Windows drive, so its declarations and vectors stayed stale after edits and deletion. The drive-letter rule now applies on Windows only. Traversal (`..`) and absolute paths are still refused everywhere.
- **Loop-to-Iterator Keeps Rust's `for` Semantics and Binds Swift's Copied Element** (`crates/prod-code-mcp`, #1460, #1461):
  - Rust: a source whose type is not a known standard collection is iterated with `IntoIterator::into_iter(source)`, which is what the `for` loop called. Before, `source.into_iter()` could pick an inherent `into_iter` of a custom type with other items.
  - Rust: a vector started with `Vec::with_capacity(…)` is refused. `collect()` would neither call the initializer nor ask for that capacity.
  - Swift: a loop that appends every element unchanged becomes `Array(source)`. Before, it became `source.map { p }`, with `p` bound by nothing.
- **`prod-code safe-delete` Applies the Edit in the Checkout That Holds the File** (`crates/prod-code-client`, #1455):
  A safe delete named by an absolute path in another checkout ran against the checkout of the current directory, so the edit the analyzer returned for the other checkout was refused at that boundary. The command now uses the checkout that holds the file, and falls back to the current directory's only when the file is in none. Edits outside the chosen checkout are still refused.
- **Search Returns as Many Hits as Asked, Up to 500** (`crates/prod-code-gateway`, #1458):
  Each ranking (lexical, graph and dense) was cut to 50 candidates before fusion, so `prod-code search QUERY --limit 100` could return 50 hits when 100 matched. Each list now goes as deep as the limit asks, and at least 50 deep as before. A limit above 500, the most one search returns, is refused with an error naming that bound instead of being answered with fewer hits.
- **Search Indexes Real C++ Functions and Keeps `cfg(test)` to Its Own Item** (`crates/prod-code-gateway`, #1457):
  - A C++ line counts as a function only when it reads like a declaration or definition. `if (…) {`, `for (…)`, `return f(x);`, assignments like `x = f(y);` and plain calls like `do_work(x);` no longer become "functions", so they no longer use up the 400-declaration budget per file before the real definitions after them.
  - In Rust, `#[cfg(test)]` marks only the item it is on, and `mod tests { … }` ends at its closing brace. Before, everything after the first `#[cfg(test)]` in a file, even one on a `use`, was treated as test code and hidden from ordinary searches.
- **`prod-code validate --diff --json` Answers in JSON** (`crates/prod-code-client`, `crates/prod-code-mcp`, #1456):
  Diff validation ignored `--json` and printed the human-readable report. `code_validate_edits` now takes `json: true`, and the CLI passes `--json` on. The answer is then one JSON object: `accepted`, the per-file analyzer `reports` with their error and warning totals, the notes, and, when a compiler check ran, `compiler` with its error count and report text. A rejected proposal still exits non-zero.
- **A `-r` Inside an `exec` Command No Longer Pins the Client to Its Seed** (`crates/prod-code-client`, `cluster`, #1454):
  The client decided that `--remote` was given by looking for `-r`/`--remote` anywhere among the raw arguments. `prod-code exec -- grep -r pattern .` therefore skipped cluster discovery and stayed on the default seed. Clap now reports where the remote came from: only prod-code's own `-r`/`--remote`, given before or after the subcommand, pins the client.
- **SourceKit Is Found Through `xcrun` Only When the Selected Toolchain Has It** (`crates/prod-code-engine-generic`, test, #1449):
  The optional native SourceKit test took an installed `xcrun` as proof that `sourcekit-lsp` exists. On a Mac whose selected developer toolchain lacks SourceKit, the test then failed while starting the engine, instead of skipping it as unavailable. The test and the gateway's installed-server check now share one probe, `xcrun --find sourcekit-lsp`, bounded to 10 seconds; the gateway's own check had no time limit before. A launcher that is missing, hangs or cannot find the server finds nothing.
- **Schema Rename Reads OpenAPI JSON by Its Root Key and GraphQL Descriptions by Their Real Delimiters** (`crates/prod-code-mcp`, #1452):
  - A JSON document is OpenAPI when its root object has an `openapi` or `swagger` key, whatever the key order or layout. A minified document such as `{"info":…,"openapi":"3.1.0",…}` used to be read as plain JSON, so renaming a property left its `required` entry stale.
  - In a GraphQL description, an escaped `\"""` no longer ends the block string, so a later mention of the field in the same description stays prose.
- **Schema Rename Leaves SQL and Protobuf Comments Alone** (`crates/prod-code-mcp`, test, #1451):
  A field mentioned in a SQL (`--`, `/* */`) or Protobuf (`//`, `/* */`) comment is now reported among the occurrences left as they are, instead of being rewritten with the schema. Comment markers inside quoted text start nothing, and a quoted name inside a code comment is prose too. The Rust-attribute test now really covers a name after `#`.
- **Schema Rename Gives Each Language Its Own Spelling and Refuses Empty Targets** (`crates/prod-code-mcp`, #1450):
  - Renaming a one-word field such as `status` to `order_status` turns the lowercase `status` into `orderStatus` in TypeScript, JavaScript, Swift and Go, and into `order_status` in SQL, Rust and Python. Before, every lowercase occurrence got the snake_case name.
  - A target with no words (`""`, `---`) is refused before anything is read or written, even with `force`. Before, it could delete the name from SQL or Protobuf files.
- **The Go Fallback Backend Normalizes gopls Diagnostic Reports Like the Managed Engine** (`crates/prod-code-gateway`, `crates/prod-code-engine-go`, #1448):
  gopls answers a diagnostic pull with an empty `kind` on a complete report. The managed Go engine already turned that into `full`, but when it failed to start and the gateway fell back to a plain gopls process, the answer reached the client unchanged and its strict parser rejected it. The fallback now applies the same rule, using the method of the request each answer belongs to. Only a complete `textDocument/diagnostic` report from a Go backend is touched; a report without items, an error, an unchanged report and other methods' answers pass as gopls sent them.
- **Roadmap Marks the Client Hot-Path Milestone Partial** (`ROADMAP.md`, `ROADMAP-AUDIT.md`, documentation, #1445):
  Milestone 1.3 claimed "zero allocations on hot paths", which is not met, so it is back to `[~]`. The required item stays visible, next to what is verified: the stack-buffered frame header, and no JSON parse for editor messages that name no mirrored file (#1433). The audit row no longer says recovery needs an editor restart: in-process reconnect with LSP state replay is implemented.
- **Rust Function Extraction No Longer Reports the Old Selection as the New Call** (`crates/prod-code-mcp`, #1441):
  When rust-analyzer's assist also changes text outside the selection, for example by making a variable `mut`, the call it put in cannot be read off the rewrite. `code_extract_function` then said that the selection "now reads" the old selected code. It now says the new call is in the diff, which still shows the extraction. Nothing else changes: the extraction still succeeds.
- **Overlay Lifecycle Tests Run Only on Linux** (`crates/prod-code-gateway`, test, #1436):
  The three `shadow_lifecycle` tests that need overlay shadows are compiled only on Linux, the one platform that has them. On macOS they panicked with "overlay unavailable". On Linux they still fail loudly when overlays are missing. The rest of the suite still runs on macOS.
- **Rust Function Extraction Keeps Other Files' Text and Picks Unshadowed Parameter Names** (`crates/prod-code-mcp`, #1439):
  - When a duplicate in another file is replaced, only the inserted call is renamed from `fun_name` to the requested name. That file's own text, such as a `"fun_name"` string, a comment, or a function of its own with that name, stays as it was.
  - In the file the helper is made in, only name tokens are renamed, so strings and comments keep the placeholder's spelling.
  - A parameter that replaces differing literals gets a name nothing in the file already uses (`value`, or `value_2`, …). Before, a local `value` in the helper's body could shadow it and change the result.
- **Rust Function Extraction Finds Duplicates by Rust's Own Tokens and Real Local Reads** (`crates/prod-code-mcp`, #1438):
  - Duplicate copies of the selection are matched on the tokens of Rust's own lexer, so code inside a `/* */` comment or a raw, byte or C string is no longer taken for a copy and rewritten.
  - A `let` binding is told by its role in the pattern, not by its case: `let Gross = …` is a binding, while the `Some`, `P` and `E::V` of `let Some(n)`, `let P { x }` and `let E::V(y)` are not. A path pattern with `::` is no longer cut as if it were a type annotation.
  - The guard against a later read of a name that the extracted call does not return counts only real reads of the local. A field or method (`o.gross`), a path segment, a struct-field label, a string, a comment, and the code after a later `let gross` no longer block a safe duplicate. That `let`'s initializer and the body of an `if let` still count.
- **Symbol Search Skips Virtual-Document Hits Instead of Failing** (`crates/prod-code-mcp`, #1437):
  A workspace-symbol hit in a valid URI of another scheme, such as a `jar:` class inside an archive, is skipped, and the file hits after it are still found. Before, an exact-name hit like that made the whole search fail as a malformed answer. Text that is not a URI at all still fails.
- **Compiler-Validation Isolation Test Checks the Gateway's Copy, Not Only the Client's** (`crates/prod-code-gateway`, test, #1435):
  The linked Rust compiler-validation regressions, cold and warm, compared only the client checkout after each validation. An overlay leak would land in the gateway's synced copy, where the compiler runs, and the next sync could hide it. They now also compare the gateway's copy of the fixture right after each validation, before another call. A control in the test changes that copy and confirms the comparison catches it. The cold case finds the copy only after its first validation, so nothing warms its first compile.
- **Agent Guidance Files a Recurrence of a Closed Issue Instead of Only Commenting on It** (`crates/prod-code-mcp`, #1434):
  The MCP initialization instructions told agents to comment on any similar issue that `code_report_issue` listed, closed ones included. A bug that came back could then be recorded only as a comment on a closed issue, with no open issue tracking it. The instructions and the similar-issue result now give the same steps:
  - an open issue that is the same problem gets a comment;
  - for a closed one, check its closing change and `prod-code --version`, and if the bug still happens on a version that has the fix, report it again with `force`, naming the closed issue.
- **Editor Traffic Skips the Mirror Parse When It Names No Mirrored File** (`crates/prod-code-client`, #1433):
  `prod-code lsp` no longer parses every message the editor sends into a JSON tree to look for paths of the node's mirrored dependency files. Each mirror directory is named after its node, such as `192.168.2.168_9400`. A message that contains none of those names, and no `\u` escape that could spell one, is passed on as it is, so a large `didOpen` or `didChange` of an ordinary checkout file is no longer parsed for this. Messages that do name a copy, written plainly or with JSON-escaped slashes, are translated as before.
- **A New Storage Directory Reports Its Disk Before the First Sync** (`crates/prod-code-gateway`, `cluster`, #1429):
  A gateway whose `--storage` directory does not exist yet reports the free share of the filesystem it will be created on: the nearest existing parent. Before, it reported "unknown" until the first sync created the directory, so the first placement onto a nearly full filesystem did not account for disk pressure.
- **Fallback Health Probes Wait for Outstanding Requests and Ignore Replayed Answers** (`crates/prod-code-gateway`, #1428):
  The fallback language server's health probe is deferred while an ordinary request is still unanswered, so a long silent request on a serial server no longer gets the server retired as unresponsive. A request left unanswered for eight probe intervals is still probed, so a stuck server is retired. An answer to a request that was already answered no longer counts as proof that the server works: each request's answer is accepted once.
- **Moving a Default Export Keeps Every Default Import Working** (`crates/prod-code-mcp`, #1431):
  `code_move` of a TypeScript or JavaScript `export default` function rewrites every default import of it, whatever its local name.
  - `import Foo, { other } from "./source"` is split: `other` stays imported from the source, and `import Foo from "./helpers"` is added.
  - `import LocalName from "./source"` now follows the move.
  - The source's own remaining calls import the moved function by default.
  Before, those cases kept a stale default import of the source, or added `import { Foo }`, a named export the destination does not have. The project failed to typecheck.
- **A Language Server That Does Not Start Fails the Load Instead of Leaving an Empty Workspace** (`crates/prod-code-gateway`, #1427):
  When an engine's configured language server (Python, TypeScript, clangd, Ada, ...) does not start and there is no managed fallback, the workspace load fails with the startup error, and the session is told why. Before, the error was dropped and a workspace with no engine was cached as ready. Every later session got that workspace and empty answers until it was unloaded, even after the server was installed or fixed. Engines that have no language server at all still load with no engine, as before.
- **Rust Validation Warm-Up Queues Files Synced During a Running Job** (`crates/prod-code-gateway`, #1426):
  When a sync asks the gateway to warm newly written Rust files while a warm-up job for that workspace is still running, the files are queued instead of dropped. The running job warms them next, and keeps going until nothing is queued. The queue holds at most the 20 latest files. Before, the request was discarded, so the files an agent had just edited stayed cold and their first validation was slow.
- **Cross-Repository Schema Renames Report Every Repository Still Indexing** (`crates/prod-code-mcp`, #1425):
  `code_schema_rename` with `repos` collects the language server's "still loading or indexing" notes from every repository it asks, the secondary ones included. Each note is listed under the result, the result is not clean, and `apply` writes nothing without `force`. Before, notes from the other repositories were stored and never shown, so a plan built from a half-indexed repository passed as complete, and the note could surface in a later, unrelated answer.
- **Nested Projects of Another Checkout Are Placed Under That Checkout** (`crates/prod-code-mcp`, `cluster`, #1424):
  A query routed into a nested project of an `also_in` checkout remembers its placement under that checkout's name, `<checkout>#<engine>`, with a worktree's origin name as the CLI uses it. Before, it used the name of the checkout the process started in, so the first route chose and recorded `A#go` for a project of checkout B, and could start a cold session for it. The checkout's own default node is still kept when it serves the engine.
- **Symlinked External Checkouts Are Asked by Their Real Paths** (`crates/prod-code-mcp`, #1423):
  A query about a file of another checkout reached through a symlink alias now opens and asks about the document by its real path. Every URI of the alias in the request becomes the real one. Before, the session opened on that checkout's real root, but `didOpen` and the request kept the alias URI, which the node cannot translate into its copy of the checkout, so the server lost the project context.
- **Symbol Search Retries an Empty Answer Right After Synced Changes** (`crates/prod-code-mcp`, #1422):
  A pooled session now records when a refresh last synced changed files. Symbol search counts its 30-second indexing grace from that moment or from the engine load, whichever is later. Before, a session whose non-gated engine was loaded more than 30 s earlier never asked again. A function in a file synced a moment before was reported as `no symbol named fresh` while the server was still indexing it.
- **Unindexed Declarations Are Reported Beside Fuzzy Matches, Not From Comments** (`crates/prod-code-mcp`, #1421):
  - `code_symbols` looks for a declaration the index omits whenever no result has exactly the asked name, and lists it under the fuzzy matches. Before, `Lost` with an indexed `LostBuilder` showed only `LostBuilder`, and the unindexed `struct Lost` went unreported.
  - The declaration scans read source with comments and string literals blanked out, so `// fn Ghost()` and `"fn Ghost()"` are no longer reported as declarations and no longer use up the three-result report.
- **Reference Discovery Keeps Searching Past Locals and Reads UTF-16 Columns** (`crates/prod-code-mcp`, #1419):
  - When `code_references` asks a dependency's declaration for its uses in the checkout, a use that means another item, such as a local `Widget`, no longer ends the search of its file. A later `dep::Widget` in that file is still asked, within the same budget.
  - A position on `$value` (TypeScript) or a Swift operator such as `<~>` is a name, and is asked of the language server instead of being refused as "on no name".
  - Positions are read and asked as LSP columns in UTF-16 code units. A use after `"😀😀"` on the same line is no longer asked two units early, so it is no longer missed.
- **Qualified Symbol Verification Checks Whole Names in UTF-16 Columns** (`crates/prod-code-mcp`, #1420):
  A qualified lookup accepts an indexed position only when the whole name starts at it, with the LSP column read in UTF-16 code units. The column it answers is in the same units. Before:
  - a stale `run` where the source now says `runner` was accepted, so a symbol-addressed rename or delete could aim at `runner`;
  - after `"😀😀"` on the same line, the answered column was two units early.
- **Metrics History Scans No Longer Block Event Recording** (`crates/prod-code-gateway`, #1416):
  A metrics request copies the in-memory events it needs and releases the ring's lock before it reads the daily history files. Every request completion records into that ring, so before, a request for a window older than the ring made recording wait for the whole file scan. The summary also runs on a blocking thread instead of an async worker.
- **Exported-Only Outlines Count Each Member's Owners** (`crates/prod-code-mcp`, #1418):
  `code_outline` with `exported_only` judges a member together with its owners.
  - A public Rust trait keeps its required methods, and a public enum its variants, without a `pub` of their own.
  - A member of an owner that is not exported is left out, such as a method of a TypeScript class that is not exported.
  - A Rust `impl` and a Swift `extension` pass their owner's visibility on.
  Before, each symbol was judged alone: `pub trait Reader { fn read(&self); }` listed `Reader` without `read`, and `class Hidden { run() {} }` listed `run`.
- **Qualified Lookups Keep Directories Named main, mod or index** (`crates/prod-code-mcp`, #1417):
  A module path read from a file path drops `lib`, `main`, `mod`, `__init__` and `index` only as the file's own name, and drops `src`, or `lib` outside `src`, as source-root directories. `src/main/foo.rs` is `main::foo`, so `crate::main::foo::record` resolves to it. Before, every component with one of those names was removed, and the lookup reported `no symbol named crate::main::foo::record`.
- **The Grep Guard Suggests Patterns prod-code grep Reads** (`scripts/hooks`, #1581):
  When the guard hook blocks a source `grep` and suggests `code_grep` / `prod-code grep` instead, it converts a basic regular expression (grep and git grep without `-E`) to ripgrep's syntax. `ctx\|cancel` becomes `ctx|cancel`, `\{2\}` becomes `{2}`, and a literal `(ms)` becomes `\(ms\)`. A `-F` string is escaped. Before, the pattern was passed on as written, so `\|` searched for a literal `|` and the suggestion silently found nothing.
- **Mixed-Language Directory Outlines Ask Each File's Own Engine** (`crates/prod-code-mcp`, #1415):
  `code_outline` on a directory that holds files of several languages opens one session per language, on that language's first file, the way a single-file outline routes it. In a Rust checkout, `scripts/a.py` and `scripts/b.ts` are outlined by the Python and the TypeScript server. Before, both went through the one session the directory chose, so `b.ts` was asked of the Python engine. A directory of one language still uses one session. The scripted test gateway now shows a session's `preferred_engine` to its script (`crates/prod-code-testkit`).
- **Linter Fix Reports Name the Files Kept for Local Edits** (`crates/prod-code-mcp`, #1414):
  `lint --fix` lists a file whose local copy changed while the fixer ran as skipped, with the reason, and its note says how many files were kept and which. The local edit stays and the node's fix of that file is not written. Before, the report said only `rewrote 1 file(s)` and did not mention the file whose fix was discarded. A failing fixer's error names kept files too.
- **Symbol Resolution Bounds Its Remote Reads** (`crates/prod-code-mcp`, #1413):
  A symbol lookup asks the gateway for at most 16 distinct dependency files, counting reads that fail, and never asks for a failed path twice. Before, only successful reads counted: an answer with 40 unreadable paths, each listed twice, made 80 sequential round trips.
- **Go Result Replacement Knows Its Boundaries** (`crates/prod-code-mcp`, #1411):
  - `func Count() (int)` is read as the one unnamed result `int`, and the replacement keeps the parentheses. Before, it was refused.
  - A caller in another module of a `go.work` refuses the replacement. Only the declaring module is compiled with the change, so before, `func Use() int { return a.Value() }` in the sibling module went unchecked when `Value` became `uint`.
  - An exported method refuses when the checkout imports `reflect`, `text/template` or `html/template`, or when its package is not `main`. In those cases code can call the method by name with no reference to it, as `reflect.ValueOf(v).MethodByName("ValueResult").Call(nil)[0].Int()` does, and that code panics after the change.
  - Go test fixtures can hold a `go.work` with modules in subdirectories (`crates/prod-code-testkit`).
- **Go Reorders Refuse Unreported Callers and Track Argument Comments** (`crates/prod-code-mcp`, #1410):
  - `code_change_signature` on a Go function refuses a reorder or removal when a file gopls did not report calls it, such as one behind `//go:build extra`. Before, the file was only compiled with the change. With `f(a, b int)` swapped, it kept `f(1, 2)`, still type-checked, and `go run -tags extra` printed `12 21` instead of `12 12`.
  - A comment inside an argument or parameter list now belongs to the element beside it: it must move with that element and goes when that element is removed. Before, the whole file's comments were compared as one list, so removing `b` from `h(1, 2 /* documents b */)` was refused although gopls removed the comment with it.
- **JS/TS Delegate Extraction Reads Initializers, Exports and Receivers from Syntax** (`crates/prod-code-mcp`, #1409):
  - Before deferring a readonly field into the helper's constructor, `code_extract_delegate` checks every instance field initializer of the class as parsed. Before, only one-line `name = value;` declarations were checked, so `metadata =` with `of / this.current();` on the next line, or with no semicolon, called `current()` before the delegate existed.
  - A modifier inside a comment is not a modifier: `// export` above a plain class no longer exports the helper and inserts it inside the comment, which broke the program.
  - A `this` owned by a nested object method, function or class, such as `{ value: 1, read() { return this.value; } }`, no longer counts as the owner's, so such an initializer is no longer refused. A `this` in an arrow function is still the owner's.
- **C-Family Parameter Extraction Lexes Continued Comments and Increments** (`crates/prod-code-mcp`, #1408):
  - In C and C++, a `//` comment ending in a backslash runs on to the next physical line, with LF or CRLF. `code_extract_parameter` with `replace_all` no longer rewrites a value on that line, and selecting it is refused before anything is written. Before, `int a=80; // keep \` followed by `80` gave three replacements instead of two.
  - `++` and `--` are single tokens in C and C++. Selecting `+value` starting at the second plus of `return ++value;` is refused. Before, it was extracted and the rewrite `return +limit;` dropped the increment, so the program printed `80 80` instead of `81 81`.
- **Signature Changes Read Each JS/TS Caller as Its Own Language** (`crates/prod-code-mcp`, #1407):
  `code_change_signature` on a TypeScript function parses a `.js` caller with JSX, and on a JavaScript function parses a `.ts` caller without it. Before, the declaration's language decided: in a `.js` caller, `<p>ready to` followed by `export {f(1, 2)}` on the next line read as a module export, so only the declaration was reordered and the program printed 21 instead of 12. A `.ts` caller with `(<number>1)` of a JavaScript function was refused.
- **Flat Outlines Hide Locals on the Function's Own Line** (`crates/prod-code-mcp`, `crates/prod-code-client`, #1403):
  `code_outline` and `prod-code outline` tell a local from a top-level item by line and character when the analyzer answers with a flat symbol list. In `fn f() { let x = 1; } static S: u8 = 0;`, `x` is hidden and counted as a hidden local, while `S` after the function stays listed. Before, a variable on the function's starting line was always shown. Nested answers are unchanged.
- **Re-Exports Never Outrank Their Definition** (`crates/prod-code-mcp`, #1402):
  - A symbol lookup drops `pub use` re-exports before it takes the best-scored hit. Before, a path hint at `lib.rs` gave only `pub use sync::scan;` the bonus, and `scan` resolved to the re-export instead of `fn scan` in `sync.rs`. Between two real definitions the hint still decides.
  - A grouped `use` ends at the `;` that closes it. Before, a `;` inside a comment such as `pub use m::{ // public API; keep sorted` ended it, so the imported names below were taken as second declarations and the lookup reported ambiguity.
- **Assist Prelude Shortening Keeps Literals and Comments** (`crates/prod-code-mcp`, #1400):
  `code_assist` drops `std::prelude::v1::` only from paths in code on the lines the assist wrote. The same text inside a string literal such as `println!("std::prelude::v1::")`, or inside a comment, keeps its bytes; before, it was cut and the program printed something else. A file that already spells the path in code keeps the analyzer's spelling, because which occurrence is new cannot be told.
- **TypeScript Safe Delete Binds Its Snapshot to the Text It Cuts** (`crates/prod-code-mcp`, #1398):
  `code_safe_delete` on a TypeScript function refuses when the target file in its project snapshot is not byte for byte the text it read first. Every later check compares against that snapshot, while the deletion is cut from the first read. An edit made between the two, such as `keep = 1` becoming `keep = 2`, passed every check and was overwritten.
- **Go Safe Delete Refuses on Git-Ignored Callers** (`crates/prod-code-mcp`, #1397):
  `code_safe_delete` on a Go function refuses, before asking gopls or the compiler, when an untracked file of the same package that Git ignores mentions the function. Such a file is never synced to the build node, so neither check there could see the caller, and the local module would break. Tracked files that match an ignore pattern are synced and unaffected, and so are checkouts outside Git.
- **Trait Parameter Removal Keeps Assigning Arguments** (`crates/prod-code-mcp`, #1396):
  Removing a trait method's parameter reads each dropped argument as a Rust expression. An assignment such as `{ counter += 1; value }` blocks the write like a call does, and so do `return`, `break` and `continue`. Before, only text with a call, macro, `?` or `.await` counted, so the increment silently disappeared. `force` still writes; pure arguments are still removed.
- **Accessor Rename Finds the Field's Own Struct** (`crates/prod-code-mcp`, #1383):
  - `code_rename` with `accessors` renames an accessor only when the analyzer resolves its `impl`'s self type to the field's own struct. Before, `a::Conn` and `b::Conn` with the same accessor name were both renamed.
  - Accessors are searched in the index of the project that declares the field, not the checkout root's.
  - A struct whose `where` clause stands on a line of its own is recognised as the field's owner.
- **Go Mocks with Three or More Grouped Names** (`crates/prod-code-mcp`, #1391):
  `code_generate_fixture` reads `Copy(dst, src, mode string)` as three `string` parameters, and `(x, y, z int)` results as three `int`s. Before, the first name became a parameter type, and the mock declared `arg0 dst`, a type that does not exist. A list of unnamed types such as `(int, string, error)` stays unnamed.
- **Generify Reads Every C++ Digit Separator** (`crates/prod-code-mcp`, #1556):
  `0xAB'CD'EF` stays one number in the namespace scan. Before, its second separator opened a character literal that ran through `}` and `namespace b {`, so the definition read as `a` and the real header in `b` was taken as unrelated. A string or character literal still open at the declaration now makes the scan uncertain, which keeps the split-header refusal, instead of proving a namespace.
- **Pruning Never Takes a Copy in Use** (`crates/prod-code-gateway`, #1524):
  - Every session and remote command holds a use on its checkout while it runs: an engine, a nested `engine_subpath` engine or a dedicated editor.
  - The worktree and main-copy pruners skip a copy that is used, or loaded at or below its root. Before, they checked only the checkout's exact engine key, so a running nested engine or editor did not protect it.
  - Each candidate is checked again just before it goes, under the lock that entering takes, and moved aside to a hidden name in the same step. The space pruner's candidate list no longer deletes a copy that a session entered while an earlier one was being deleted.
  - Copies left moved aside by an interrupted prune are deleted on the next run.
- **Sync Never Deletes Through a Directory Symlink** (`crates/prod-code-gateway`, #1526):
  Manifest reconciliation and command snapshots no longer walk a directory symlink on the node copy, the link `copy_tree` keeps as a link. Before, an alias such as `src/again -> .` made `src/again/value.rs` look absent from the client's manifest. The real `src/value.rs` was deleted through it, a link to a parent recursed, and files behind a link pointing outside the copy could be removed. The link itself stays; an ordinary unlisted file is still deleted. The special case for the shared Python stub link is covered by the same rule and was removed.
- **Generify Reads Prefixed C++ Character Literals** (`crates/prod-code-mcp`, #1552):
  The namespace scan skips `L'}'`, `u'}'`, `U'}'` and `u8'}'` as character literals. Before, such a brace closed `namespace a`, and a header that declares the function was taken as unrelated. Digit separators such as `1'000` and `0xFF'FF` are still read as numbers.
- **Rename Test Names Only in Real Tests** (`crates/prod-code-mcp`, #1550):
  `code_rename` with `comments` takes a Rust file's test functions from its syntax tree. `#[test] fn order_total()` spelled inside a string or raw string is data and keeps its bytes. Real tests under `#[test]`, `#[tokio::test]`, `*_test` and `rstest` are still renamed.
- **Process-Group Signals Stay with Their Own Group** (`crates/prod-code-gateway`, #1545):
  A remote command's exited leader is no longer reaped while its stream may still signal the group. It stays a zombie that holds its PID, and so the group's number, until the stream has finished, timed out or been cancelled and killed the group. A late kill can therefore never reach a newer group that reused the number. Descendants that outlive the leader are still killed on timeout, departure and cancellation.
- **Generify C++ Header Safety** (`crates/prod-code-mcp`, #1546, #1547):
  - A same-named prototype in a header no longer has to share the source's stem or be included directly to count. Generify refuses the split layout unless every such prototype provably sits in another namespace, so a header reached through another header or named differently is refused again instead of left out of sync with a template definition.
  - `namespace a::b` and `namespace a { namespace b` are the same namespace.
  - Comments and string or character literals no longer affect the namespace scan. Anonymous and inline namespaces, namespace macros, raw strings, conditional blocks and qualified names count as uncertain, so the refusal stays.
  - A non-ASCII character before the definition, as in `// café`, no longer panics the scan.
- **Builder Fields with Associated-Type Bindings** (`crates/prod-code-mcp`, #1390):
  `code_generate_fixture` with `builder` accepts a field such as `values: Box<dyn Iterator<Item = u8>>` and keeps its whole type. The `=` inside generic angles binds an associated type and is no longer read as a field default. A real default such as `max: Vec<u8> = Vec::new()` is still refused.
- **Rename Comments in Rust Block Comments and Raw Strings** (`crates/prod-code-mcp`, #1384):
  `code_rename` with `comments` reads Rust files with Rust's lexer. It now renames inside `/* */`, `/** */` and nested or multiline block comments, and leaves raw-string data such as `r#"<a href="//Order">"#` byte for byte. A test commented out inside a block comment keeps its name. `comments` together with `accessors` is refused before writing instead of silently dropping the comment update.
- **Descendants Outliving the Command** (`crates/prod-code-gateway`, #1541):
  When a remote command times out, its client leaves or its stream is cancelled after the command itself already exited, the descendants still in its process group are killed too. The exited command's PID is no longer signalled on its own, so a reused PID is never hit.
- **Encoded Query Keys in Running Commands** (`crates/prod-code-gateway`, #1539):
  A URL query key is percent-decoded before the secret-name check, so `?p%61ssword=…` and `?%74oken=…` read `<redacted>` in status and gossip like their plain forms.
- **Generify Bounds and C++ Headers** (`crates/prod-code-mcp`, #1382):
  The new type parameter goes after a complete `F: Fn() -> i32` bound instead of inside its return arrow. A C/C++ header whose same-named prototype sits in another namespace, or that the source neither includes nor shares a stem with, no longer blocks the conversion. A header that may declare the function is still refused.
- **Cancelled Exec Streams Kill Their Process Group** (`crates/prod-code-gateway`, #1536):
  When a running command's stream is dropped or cancelled before it finishes, without a Disconnect, its process group is killed and still reaped. It used to keep running past its deadline. Normal exit, timeout and disconnect handling are unchanged.
- **URL Query Secrets in Running Commands** (`crates/prod-code-gateway`, #1535):
  A URL argument in status and gossip keeps its keys, but a query value whose key names a secret reads `<redacted>`: `postgresql://host/db?password=…`, `access_token`, `X-Amz-Signature`, `pwd`. Userinfo is found before a `?` or `#` too.
- **Unknown Verify Modes Refused** (`crates/prod-code-mcp`, #1453):
  A `verify` value other than `compile`, such as the typo `compiler`, is refused before any planning, analyzer query or write, for MCP tools and for the CLI, which goes through the same entry point. It used to read as "no compiler gate", so an apply went through unchecked. Omitting `verify` keeps the analyzer check, and `code_generate_fixture` keeps its boolean `verify`.
- **Running Commands Without Secrets** (`crates/prod-code-gateway`, #1484):
  Status answers and cluster gossip list each running command in a redacted, bounded form. Values after `--token`/`--password`-style flags, `KEY=value` pairs whose key names a secret, `Authorization`/`Bearer` values and URL userinfo read as `<redacted>`, and the line is capped at 240 bytes. The command still runs with its real arguments.
- **Inline-Parameter Literals and Turbofish Calls** (`crates/prod-code-mcp`, #1381):
  A string argument counts as caller-independent only as one whole literal, so `"yes" if flag else "no"` is no longer copied into the callee. A Rust call with a turbofish, such as `clamp::<u32>(v, 100)`, is a call rather than a function value. Same-named constants from different caller scopes are tracked in #1508.
- **Call Tree Depth on Shorter Paths** (`crates/prod-code-mcp`, #1389):
  `code_callers`/`code_callees` expand a function again when a later path reaches it higher up than where it was first shown, so a callee within the requested depth is no longer dropped as "shown above". Cycles and repeats at the same or a deeper level are still marked.
- **Codemod Receiver Boundaries** (`crates/prod-code-mcp`, #1432):
  A placeholder before `.`, `->` or `?.` binds only the receiver: `$a.unwrap() ==>> handle($a)` turns `1 + obj.unwrap()` into `1 + handle(obj)`, not `handle(1 + obj)`. Grouped receivers, unary prefixes and turbofish arguments such as `parse::<u8, u16>(s)` still match whole.
- **Unicode in Rust Match Arms** (`crates/prod-code-mcp`, #1430):
  Replace-conditional no longer panics on a `match` arm such as `0 => f(é, 0),`: the arm scanner trims its rolling context by characters instead of cutting a multi-byte identifier.
- **Python Setter Evaluation Order** (`crates/prod-code-mcp`, #1406):
  Encapsulating a Python field keeps the staged form for an assignment whose value is an attribute read, since a property getter can rebind the receiver first; literals and bare names still become a direct setter call. C++ literals now also get the direct call.
- **Go Signature Stack Overflow** (`crates/prod-code-mcp`):
  The change-signature planner future is boxed, so a Go reorder no longer overflows a 2 MB thread stack in debug builds.
- **Test Suite Green Again** (`crates/prod-code-mcp`, tests):
  Tests whose expectations predated the audit merges follow the current contracts. These cover analyzer identity for fields, stricter diagnostics, staged compiler reports, failed fixer status, TypeScript compile refusal and syntax-tree identity for introduce-variable. Workspace formatting is clean again.
- **Compile Verification of Every Affected Rust Project** (`crates/prod-code-mcp`, #1365):
  `verify: "compile"` checks each Cargo package group that owns an edited Rust file, including crates the root workspace excludes, in the same proposed shadow; any failing group fails the verdict, and a Rust file no Cargo target owns is reported as incomplete verification instead of passing. Edits outside Rust and trees without any `Cargo.toml` keep the single root check.
- **Polyglot Extract-Delegate Identity and Initialization** (`crates/prod-code-mcp`, #1335):
  JavaScript and TypeScript sources are parsed with Oxc, so a delegate keeps its declaration initializer when the constructor assigns other fields, and only the analyzer's references are rewritten, never a same-named property of another receiver. Declaring-file references and callers before the class are rewritten, and Go keeps package qualification in nested literals.
- **Polyglot Signature Identity, Effects and Awaits** (`crates/prod-code-mcp`, #1333):
  Signature changes rewrite a C/C++ prototype only when the analyzer ties it to the changed function, treat `1+i++` and concatenations with calls as effectful arguments, and remove only the `await` that belongs to the changed call, including `await obj.f()` and parenthesized operands. Rust literals such as `0x1F_u8` and CommonJS `require` bindings keep their earlier handling.
- **Extract-Parameter Tokens and One-Line Bodies** (`crates/prod-code-mcp`, #1309):
  `replace_all` replaces the selected expression as tokens, so `80` no longer rewrites `"80px"`, comments or longer numbers; one-line Rust functions no longer panic; `f(existing_arg,)` gains the new argument without a doubled comma; C-family casts keep their comparison precedence and comments their boundaries.
- **Rust Extract-Delegate Literals, Receivers and Await** (`crates/prod-code-mcp`, #1334):
  Files that use the moved fields only in struct literals or patterns are rewritten too, `other.city` on another owner inside a moved method becomes `other.address.city` while `self` stays with the helper, and async forwarding awaits. Moved literal fields join as `address: Address { street, city }` on one line when nothing but code sits between them.
- **Constructor Replacement Values and Coverage** (`crates/prod-code-mcp`, #1345):
  Factory and builder replacements keep argument evaluation order for user-defined clone/into calls, ignore constructors inside nested block comments, keep `Self` bound to the nearest impl, keep spread arguments and Python defaults, cover C++ brace and Swift `init` construction, and generate valid Go scalars and generic arguments.
- **Rust Trait Extraction Conditions and Syntax** (`crates/prod-code-mcp`, #1359):
  Conditional method and impl attributes are carried onto the generated trait and implementation instead of refused, the widest selected visibility is used, const-expression comparisons are no longer read as generic angles, and Boolean negation in an impl header is no longer taken for a macro.
- **Rust Item and Module Moves** (`crates/prod-code-mcp`, #1354):
  Moves keep multiline attribute and doc preambles, give a public item a public new module, pick the new module's parent from the item's own crate root, and refuse targets owned by a different library/binary crate. Whole-module moves rewrite `super::` paths by syntax, do not follow links, and update every root that declares a shared file.
- **Return Wrapping Callers and C++ Prototypes** (`crates/prod-code-mcp`, #1347):
  Swift callers in the same SwiftPM target and real calls after a Python import on the same line are no longer skipped. A C++ prototype is rewritten only when its header is actually included by the definition, so an unrelated same-named prototype stays untouched, and attributes on the declaration survive.
- **Encapsulate-Field Nested Edits and Privacy** (`crates/prod-code-mcp`, #1326):
  Nested assignments such as `cfg.retries = cfg.retries + 1` compose into one setter call, accesses from other modules of the declaring file are rewritten while the actual privacy scope keeps direct access, and polyglot rewrites follow the analyzer's references instead of spelling. Plain Rust and Python assignments keep a direct setter call.
- **Convert-to-Method Promoted Parameter Uses** (`crates/prod-code-mcp`, #1317):
  Promoting a parameter to the receiver rewrites every direct use of it in TypeScript, Python, Swift and C++ bodies, including comparisons, indexing, arguments and one-line bodies, and keeps defaults, Unicode comment boundaries and signature suffixes in their scope.
- **Rust Extract-Field Boundaries** (`crates/prod-code-mcp`, #1342):
  Extract-field no longer rewrites matching text inside strings or longer numbers, keeps deeply nested patterns blocking instead of giving them an initializer, and keeps binding identity under NFC normalization and private constructors.
- **Prune Runtime Roots and Engine Scope** (`crates/prod-code-mcp`, #1376):
  Zero-reference Rust items reached through `#[no_mangle]`, `#[export_name]` (also inside `unsafe(...)`) or an unknown registration attribute stay out of the dead set, so prune no longer deletes FFI entry points. Prune refuses non-Rust and mixed scopes before any query or write instead of reporting "nothing to prune".
- **Boolean Value Inversion Blockers** (`crates/prod-code-mcp`, #1375):
  Semantic blockers now stop a compile-verified inversion before it writes, unless `force` is set. Multiline and qualified `Default`/serde derives are recognized through comments, and grouped borrows such as `&mut (f.enabled)` count as borrowed places.
- **Stale Deletion Hunks** (`crates/prod-code-mcp`, #1374):
  A patch that deletes a file is applied only when its hunks reproduce the live file exactly, carriage returns and the final newline included. A stale, incomplete or replaced preimage refuses the whole proposal before the analyzer or any write; a Git preimage no longer stands in for a missing live file.
- **Pull-Up Literal and Declaration Preservation** (`crates/prod-code-mcp`, #1327):
  Sibling cleanup no longer treats literal whitespace or modifier-looking text as interchangeable. Moved-member modifier edits stay within parsed declarations, retaining decorators and ordinary constructor/destructor boundaries. Inheritance replacement refuses opaque base coverage and excludes Swift initializers from default method forwarding.
- **Go Project Routing and Placement Evidence** (`crates/prod-code-mcp`, #1330):
  Nested Go requests retain their selected module's OS constraints, including imported local replacement dependencies within bounded inspection. Independent child modules stay separate. Plain workloads reconsider remembered Mac placements when a compatible unpressured non-Mac node returns, and OS-constrained failures retain the underlying probe error.
- **Exec Writeback Preservation** (`crates/prod-code-mcp`, `crates/prod-code-client`, #1324):
  Returned files preserve concurrent local deletions and renames, including ignored outputs and file-to-directory replacements. Unverified optional outputs stay untouched without blocking execution. Cross-platform warnings retain pre-write evidence and nested `cfg` predicates; only confirmed Rust formatter whitespace changes are exempted.
- **MCP Stdin Reload Ownership** (`crates/prod-code-mcp`, #1325):
  Hot reload waits for outstanding worker reads before replacing the process. Eligible Unix pipes and sockets use cancellation-safe readiness reads; shared socket output flags remain unchanged, while uncertain shared inputs retain conservative read ownership.
- **Report Privacy and Labels** (`crates/prod-code-mcp`, `crates/prod-code-client`, #1323):
  Public reports scrub IPv6 endpoints, including zones and compact error suffixes, before drafting or filing. Reports reject multiple distinct type labels, retain normalized duplicates and optional `roadmap`, and expose that marker in CLI and MCP guidance.
- **Complete Compiler Proposals** (`crates/prod-code-mcp`, #1256):
  Diff validation stages file deletions together with additions and replacements in every compiler shadow request. Unsupported resource operations and escaping paths refuse the whole proposal. Reports identify the configured command and unresolved command roots, without claiming broader target or feature coverage.
- **Rust Fixture Completeness** (`crates/prod-code-mcp`, #1307):
  Default Rust generation includes every record field and retains enum variant shapes to produce values. Visibility stripping preserves identifiers beginning with `pub`; balanced attributes, raw field names and function-return arrows retain their declaration boundaries.
- **Rust Supertype Header Preservation** (`crates/prod-code-mcp`, #1305):
  Restricted-visibility traits retain their supertraits, higher-ranked `Self` predicates retain their binders, and implementation lookup follows complete headers through nested const-argument items. Unrelated bodies and declarations are excluded, with one shared lexical mask per header search.
- **Type-Migration Error and Syntax Preservation** (`crates/prod-code-mcp`, #1310):
  Attribute diagnostics remain blocking during migration and conversion, and missing reference evidence refuses planning even with force. Rust return-type edits preserve generic headers, `where` constraints, trait-item boundaries, macro delimiters and Unicode identifier tokens.
- **Diagnostic Error Authority** (`crates/prod-code-mcp`, #1286):
  Genuine derive-inference and auto-trait errors remain blocking with explanatory notes. Baseline subtraction requires unambiguous occurrence, range and owner correspondence; changed literal/header identities and uncertain matches retain their errors.
- **Promise Caller Preservation** (`crates/prod-code-mcp`, #1280):
  Promise wrapping requires evidence for selected member callers, retains existing awaits around postfix results, and preserves uncertain regex/division occurrences as unmatched references. Ambiguous or unsupported callers refuse writes, including forced apply, while proven calls retain their value and evaluation order.
- **Rust Introduce-Variable Preservation** (`crates/prod-code-mcp`, #1300):
  Extraction checks executable syntax, binding and primitive-operation identity before reusing a value, retaining strings, comments and pattern literals. Fresh-name checks use Rust's NFC identifier identity, and uncertain effects, macro captures and guarded panic placement refuse writes.
- **Failure-Dossier Evidence Preservation** (`crates/prod-code-mcp`, #1306):
  Compiler suggestions retain the failing Rust test command's package and filter selection, and both verification passes share one timeout budget. Rust assertion output with duplicate operand markers is reported as ambiguous instead of truncating a value.
- **Rust Parameter-Object Preservation** (`crates/prod-code-mcp`, #1293):
  Generated structs retain module ownership and attached attributes, including raw module names, header macros and Unicode trivia. Same-line parameter uses and nested calls are rewritten using original source coordinates, and explicit lifetimes no longer stop later elided references from receiving the generated lifetime.
- **Rust Predicate Inversion Preservation** (`crates/prod-code-mcp`, #1295):
  Inversion preserves raw-literal bytes and returns owned by nested functions, closures and async blocks. Generic call arguments, qualified paths and type-macro token trees retain their boundaries while selected callers receive the compensating negation.
- **Language-Safe Convert-to-Method Verification** (`crates/prod-code-mcp`, #1301):
  Non-Rust convert-to-method requests reject `verify: compile` before planning or writing, rather than treating an unrelated Cargo check as verification. Ordinary non-Rust conversion and Rust compile verification retain their existing behavior.
- **Rust Make-Static Safety** (`crates/prod-code-mcp`, #1297):
  Owning receivers are refused before planning or writing, preserving consumption and destructor timing. Impl classification ignores comments and higher-ranked bounds while preserving trait-method refusal for raw and Unicode identifiers.

## v0.3.41 — 2026-10-24

### Tests
- **Coverage Campaign, Wave 3** (`crates/prod-code-gateway`, `crates/prod-code-mcp`, `tests/support`):
  Every file now meets the 80% per-file region gate. New gateway tests cover:
  - a real server over TCP and a Unix socket, with a token and with SIGTERM/SIGINT;
  - strict TLS;
  - Prometheus scrape and push;
  - the shadow runner, janitor sweeps and managed-backend sessions.

  Two measurement gaps are closed:
  - The LSP secret-spawn driver runs with a cleared environment and lost `LLVM_PROFILE_FILE`. Its coverage was dropped and a stray `default_*.profraw` was written into the crate directory. The fixture now passes the variable on.
  - The janitor loop was generic over its host probe, so the production copy never ran under the tests. It now takes a boxed probe and is compiled once (#1195).

### Fixed
- **Editor Backpressure and TypeScript Initialization** (`crates/prod-code-gateway`, #1289):
  Editor input waits for queue capacity within one write deadline instead of disconnecting on transient saturation, while retirement and child-exit handling remain responsive. TypeScript fallback SDK paths reach initialization without replacing explicit editor selections or preferences.
- **Rust Signature Syntax Preservation** (`crates/prod-code-mcp`, #1287):
  Added argument expressions retain their braces and literal bytes in structural rewrites. Return-type changes preserve newline-delimited `where` bounds, Unicode identifiers containing `where`, and macro token trees containing comparisons or shifts, and simultaneous visibility and async changes keep declaration offsets current while updating caller awaits.
- **Ordered Workspace Edits and File Move History** (`crates/prod-code-mcp`, #1288):
  Preflight defers state checks affected by earlier operations to transactional execution, so a binary file or moved directory can be replaced with a new text file in the same edit. Single-file moves carry their original text for reports through overwrites and sequential renames, including moves between repository roots. Failed later steps still restore the original paths, bytes and permissions.
- **Occurrence-Based Dependency Slices** (`crates/prod-code-mcp`, #1290):
  Slices resolve separate identifier occurrences before deduplicating target declarations, so equal method names on different receivers retain both dependencies. Dataflow slices preserve original query positions, and occurrence-budget exhaustion is reported as incomplete.
- **Literal CTest Selection** (`crates/prod-code-mcp`, #1284):
  Impact-generated CTest filters escape registered test names before joining them, so regex punctuation in a name neither selects unrelated tests nor hides the intended one.
- **clang-tidy Filename Boundaries** (`crates/prod-code-mcp`, #1282):
  CMake and Meson lint/fix plans pass source paths through NUL-delimited `find` and `xargs`, preserving spaces, quotes and backslashes as filename characters.
- **Plaintext Clients of a Strict-TLS Gateway Receive the 426** (`crates/prod-code-gateway`):
  The gateway wrote `426 Upgrade Required` and closed the socket while the client's request was still unread. The kernel then reset the connection, so the client usually saw `Connection reset by peer` instead of the answer. The gateway now shuts down its side and drains the request (for at most a second) before closing.
- **A Gateway Bound to Port 0 Advertises the Port It Took** (`crates/prod-code-gateway`):
  Without `--advertise`, a gateway started with `--bind …:0` advertised `<host>:0` to the cluster. It now advertises the address its listener actually bound.
- **F# and NASM Sources in Non-Git Workspaces** (`crates/prod-code-mcp`, #1275):
  Filesystem scanning and sync planning now include `.fs`, `.fsi`, `.fsx`, and `.nasm` sources, including uppercase suffixes, alongside their project manifests. The relevance watermark version is advanced so older sync state cannot retain the omissions. Unchanged warm plans and existing artifact/data exclusions are preserved.
- **A Rebalance Redirect Right After the Handshake Is No Longer Lost** (`crates/prod-code-gateway`):
  The session subscribed to rebalance redirects only once its loop started. A redirect issued between the handshake response and the first loop turn reached nobody, so `trigger_rebalance` reported 0 sessions. Under the loaded coverage run this failed `editor_session_receives_rebalance_redirect`. The subscription is now taken before the handshake response is sent. The live cluster test now retries a refused connect until its deadline, instead of failing on the first try while the second gateway starts. Under parallel load, each test passes 10 times out of 10 (#1264).

### Tests
- **Coverage Campaign, Wave 2** (all crates):
  Workspace region coverage rose from 90.30% to 90.84%, and files under the 80% gate fell from 32 to 8. New tables cover:
  - `verify` planning and parsing;
  - the MCP transport, dispatch retry and rebalance tick;
  - `fixit`;
  - polyglot inline-parameter and type migration;
  - the gateway LSP intercept, the workspace coalescer, shadow runner and probes;
  - the remaining client, engine and cluster files.

  The CMake configure probe and the Swift settings wait now take their program and time limit as parameters, so their outcomes can be tested (#1195).

### Fixed
- **Criterion Result Markers** (`crates/prod-code-mcp`, #1276):
  Benchmark names containing `time:`, such as `runtime::poll`, no longer hide Criterion's timing result. Single-line and wrapped results preserve the complete benchmark name.
- **Language-server Child Environments** (`crates/prod-code-gateway`, `crates/prod-code-engine-generic`, #1269):
  Editor proxy and generic language servers no longer receive cluster authentication or TLS environment variables, including reserved names supplied through server configuration. Ordinary environment configuration is preserved.
- **Make Project Source Extensions** (`crates/prod-code-gateway`, `crates/prod-code-mcp`, #1270):
  Root and nested Make projects now recognize uppercase C/C++ source and header suffixes, including `.C`, `.CPP` and `.HPP`, with the same manifest precedence as lowercase sources.
- **Nested Project Roots and Cargo Membership** (`crates/prod-code-mcp`, #1262):
  A language inferred from a child no longer hides that child's project root. Independent crates below empty virtual workspaces retain their own roots. Cargo membership now reads the actual TOML workspace table, honors literal path-prefix exclusions, ignores package packaging exclusions, and matches interior member globs without absorbing unrelated or deeper child projects. Implicit in-tree path-dependency members continue to share their workspace's session.
- **Bugs Found by the Coverage Campaign, Wave 2** (`crates/prod-code-mcp`, #1195):
  - Inline-parameter reported call sites one line too early.
  - It treated `export const a = f(1)` as a mere export of `f` rather than a call.
  - Type migration matched an identifier that only began with the requested name (`value_suffix` for `value`) and names inside comments and imports.

## v0.3.40 — 2026-10-24

### Tests
- **Coverage Campaign, Wave 1** (all crates):
  Region coverage of the workspace rose from 81.65% to 90.30%. The number of files under the 80% per-file gate fell from 276 to 32. The new tests are tables of cases that assert exact results: rendered text, edits, refusals and error messages. They cover the MCP tool handlers, the dependency readers for Cargo, Gradle, Maven, .NET, Python and TypeScript, polyglot extract, move, wrap-return, encapsulate and the other refactorings, the fixture generator, the client commands, packaging and update, the gateway runners and caches, the protocol messages and the language engines. Three existing tests stopped depending on parallel tests:
  - the RAM cache sweep test no longer shares a name prefix with workspaces where exec tests run;
  - the metrics HTTP test serves on the listener it bound;
  - the Unix socket test no longer probes a listener that a forked child may still hold (#1195).

### Fixed
- **Bugs Found by the Coverage Campaign** (`crates/prod-code-mcp`, `crates/prod-code-client`, `crates/prod-code-gateway`, #1195):
  - Polyglot move:
    - a TypeScript caller import was corrupted when the symbol's name contained the source module's basename;
    - Python callers were matched by module substring (`pkg.ab` was rewritten when `pkg.a` moved);
    - a symbol was merged into an import from a longer module path;
    - a merged TypeScript import lost its spacing.
  - Polyglot extract function:
    - a Go parameter of a predeclared type such as `string`, `bool` or `error` took the type of the next parameter;
    - a Python `def` at the start of the file got the wrong scope end;
    - offsets drifted on CRLF text.
  - Dependency graph:
    - a renamed Cargo path dependency was lost depending on hash order;
    - Gradle `include` statements that continued past blank or comment lines, or across an open parenthesis, were cut short;
    - Python `from a.b import` and `from . import` were matched incorrectly;
    - Rust `pub use` and `pub mod` did not count as imports;
    - TypeScript imports in single quotes were missed.
  - Fixture generator:
    - Python return annotations containing brackets or followed by a one-line body were misread;
    - a `}` before the opening `{` was taken as the closing brace;
    - Java field modifiers were taken into the type;
    - a trailing `//` comment swallowed the next field;
    - Go `<-` channel parameters were split as generics;
    - Java constructor arguments were laid out one per line;
    - the sample values for Go `byte`, `rune` and `error`, TypeScript `void`, `any`, `unknown` and `Array<T>`, Python containers and `None`, Swift optionals and Java `byte`, `short` and `float` were wrong.
  - Other refactorings:
    - make-static picked the earlier of two short methods on adjacent lines;
    - change-signature accepted a lone quote as a string argument and misread names followed by `<`;
    - impact checked only the first occurrence of a name on a line;
    - extract-function stripped unbalanced outer parentheses;
    - replace-inheritance for Python broke the indentation of a replaced `pass` block, inserted after the trailing newline, and could panic when cutting a diff inside a multibyte character;
    - extract-delegate misread TypeScript parameters with defaults or `?`;
    - replace-conditional offsets drifted on CRLF;
    - loop-to-iterator refused the Go form `acc = append(acc, x)`;
    - lexical references missed `}from'…'` imports without a space.
  - Client and gateway:
    - the divergent bench reported a gateway hover error as `hover returned null`;
    - a HEAD request for an unknown metrics path received a body.
- **Rust Import Diagnostics Respect Module Boundaries and Aliases** (`crates/prod-code-engine-rust`):
  Imports in a parent module no longer suppress unresolved-name errors in a child module, and aliased imports suppress duplicate diagnostics only for the name they actually bind. Syntax-based import scanning now also reports unused imports after another statement or an attribute, including multiple `use` items on one line (#1248).
- **Implicit Rust SSR Context Includes Receiver Methods and Item Scopes** (`crates/prod-code-engine-rust`):
  Structural replacement without an explicit position now selects a method or item scope when the context file has no free functions. Patterns can resolve receiver expressions, method-body imports, and names in nested item-only modules. Explicit caller positions and file-scoped search remain unchanged (#1254).
- **Invalid Token Files Fail Closed** (`crates/prod-code-protocol`, `crates/prod-code-gateway`, `crates/prod-code-client`):
  A configured token file that is missing, unreadable, invalid UTF-8, or has a blank first line now stops gateway startup before storage or listener initialization and makes default client connections return an error. A nonblank direct token retains precedence. The checked `try_auth_token` API distinguishes absent configuration from failure; the legacy `auth_token` API panics on invalid selected configuration instead of silently disabling authentication (#1246).
- **Acknowledged Syncs Survive Lost-Client Restore and Failed Deletions Stay Stale** (`crates/prod-code-gateway`):
  Sync and lost-client restoration now serialize disk updates for the same workspace root. Acknowledged files, including coordinated TypeScript configs, survive restoration; failed deletions remain marked for retry. Cancellation retains lock ownership through disk and metadata commit, while subsequent analyzer updates use a bounded wait (#1251).
- **Completed LSP Responses Survive Server Exit** (`crates/prod-code-engine-generic`, `crates/prod-code-engine-go`):
  If a language server exits after delivering a response but before its waiter resumes, the completed request now returns that response. New requests to a retired engine and requests whose reply channel closes without a response still fail (#1249).
- **Python Local Imports Keep Their Binding During `code_wrap_return`** (`crates/prod-code-mcp`):
  Parenthesized and explicitly continued function-local imports now shadow an imported callable just like single-line imports. The empty-reference fallback reuses the import-statement collector, keeps aliases distinct, and excludes imports inside nested functions, so it does not treat an unrelated local call as a caller of the function being wrapped (#1244).
- **`code_test` Shows What Failed the Run** (`crates/prod-code-mcp`):
  A failed Rust test run reported `1 error(s), 92 warning(s)` but listed only warnings, so the cause stayed hidden. There were two reasons. First, cargo prints a library's warnings before the error of the test target built on it, and `code_test` renders only 40 diagnostics, so an error after the 40th warning was cut off. Second, when a test binary crashed, cargo's own `error: test failed, to rerun pass …` line was counted as the compiler error, which suppressed the output tail that carries the signal. Errors now come first, with each level kept in compiler order. That cargo summary is no longer a diagnostic. A failed run that no error or failed test explains shows its output tail, with room reserved for it next to the warnings (#1230).

## v0.3.39 — 2026-10-24

### Fixed
- **`prod-code validate` Answers Within Two Minutes and Shows Progress** (`crates/prod-code-client`, `crates/prod-code-mcp`):
  A 14-file Rust batch printed nothing for several minutes, until it was interrupted. The CLI used the 600 s refactoring validator, which also discards every result once one file stays unchecked. The CLI now uses the same 120 s interactive budget as `code_validate_edits` and keeps per-file results, naming any file it could not check. Baseline diagnostics, which are optional, get at most 30 s of that budget, so the proposal is still checked. After 10 s of silence, and every 15 s after that, a progress line on stderr gives the phase and the number of files done. A fast batch prints none (#1220).
- **Qualified Lookup Finds a Dependency Type's Method** (`crates/prod-code-mcp`, `crates/prod-code-gateway`, `crates/prod-code-engine-rust`):
  `def --symbol Analysis::parent_module` in this repository exited 1 with unrelated suggestions, although the `ra_ap_ide` dependency declares that method. Qualified library queries were searched under their full text, which the index does not match, and any workspace hit hid the dependencies. When no workspace declaration matches, lookup now asks the gateway for dependency symbols by the bare member name, keeps those whose owner matches, and returns them. Workspace declarations still win. A miss suggests that type's own methods. The dependency search is bounded to 5 s, and the deadline also reaches the gateway (#1218).
- **Rust Move Apply Reads LSP Columns as UTF-16** (`crates/prod-code-mcp`):
  A struct move whose dry run succeeded refused on `--apply` with `3 reference(s) ... were not rewritten`, although nothing had changed. The analyzer's references were right, but move turned their LSP columns, which count UTF-16 code units, into byte offsets by counting `char`s, and it split lines only on `\n`. An emoji or another non-BMP character earlier on a reference's line, or CRLF line endings, moved every offset after it, and the stale-reference check refused. Move now uses the shared UTF-16 and CRLF converter. A refusal names the on-disk position of the reference (#1215).
- **Module-Qualified Rust Lookups Keep the Module** (`crates/prod-code-mcp`, `crates/prod-code-gateway`, `crates/prod-code-engine-rust`):
  `def --symbol parent::fallback::cap` returned a local `let cap` in the parent module instead of `fn cap` in the child module declared with `#[path]`. `parent::child_fn` returned eight `let` bindings and missed the function. `fixtures::state` returned 12 parameters and fields, each with its whole body, over 1800 lines. Lookup now uses the analyzer's semantic module path for each file, through the new `prodCode/modulePath` request, instead of guessing it from file names or `mod` lines. A Rust local binding or parameter never matches a qualified path, and a function's locals are no longer taken as members of a type with the same name in another case (`fn noise` against `Noise::state`). `crate::`, `self::` and `super::` prefixes resolve. A real ambiguity lists at most 10 ranked one-line previews without bodies, and a miss names the closest qualified declarations (#1214, #1216).
- **report-issue Refuses Names From the Reporter's Private Checkout** (`crates/prod-code-mcp`, `crates/prod-code-client`):
  `code_report_issue` scrubbed addresses, host names and home paths but published everything else verbatim. A problem hit in another repository therefore reached the public tracker with that repository's file paths and module, type and function names. A report filed from any checkout other than this public repository is now refused when its title or body names one of the checkout's tracked source paths, file names or stems, or an identifier declared in its sources. The refusal lists the tokens and asks for a neutral reproduction plus `private_ref`. Names made only of generic words (`src/main.rs`, `error`, `config`) do not count. The scan is lexical and bounded: 10 s, 50,000 files, 256 KiB per file, 32 MiB in total. `allow_private_names` / `--allow-private-names` keeps the old behaviour for another public repository. The tool description and the server instructions state the rule (#1219).
- **Extract-Function Names the Loop Exit It Cannot Carry** (`crates/prod-code-engine-rust`, `crates/prod-code-mcp`):
  Extracting a loop body that mixes `let ... else { break }`, `continue` and `?` failed with `rust-analyzer cannot extract a function from this selection`, followed by a list of unrelated assists. A syntax preflight now refuses such a selection before the assist runs, and names each exit with its line: mixed loop exits, a labelled `break`/`continue` aimed at an outer loop, and a `return` owned by a closure or nested function inside the selection. Selections the assist does handle, such as only `break` or only `continue`, extract as before (#1217).
- **Moving an Item Into Its Parent Updates the Parent's Re-Export** (`crates/prod-code-mcp`):
  `code_move` of `run_stdio_mcp_server` from `server/transport.rs` into `server/mod.rs` refused with `unresolved import [E0432]`. The parent still re-exported the function from the child (`pub use transport::{...}`), because references inside the destination file were skipped. The destination's import of the moved name is now dropped like any other stale import: grouped siblings stay, and an emptied `use` goes (#1201).

### Tests
- **Qualified Lookup Against Same-Named Wrappers** (`crates/prod-code-gateway`): the real-analyzer table for qualified Rust definitions gains 13 rows, among them a same-named wrapper that calls the original (`signature::offset_of` against `move_item::offset_of`), nested wrappers, `pub(crate)` originals and a method next to a free function. The behaviour itself was fixed by #1224 (#1221).

## v0.3.38 — 2026-10-22

### Fixed
- **Rust Engine Writers Revoke Snapshots Before Taking the VFS Lock** (`crates/prod-code-engine-rust`, `crates/prod-code-gateway`):
  The second deadlock between engine writes and analysis snapshots (#1190 was the first). A writer took the VFS write lock and then set Salsa inputs. Salsa waits for every snapshot to be dropped, while a snapshot query waited for the VFS lock. Every snapshot's `Analysis` is now a revocable handle (`snapshot/lifetime.rs`). Before a writer takes the VFS lock, `prepare_mutation` revokes idle handles at once and triggers cancellation for active ones, which release their handle as they unwind. A gateway query that times out or is cancelled now revokes its snapshot instead of leaving it alive in the background (`lsp/bounded_query_control.rs`), and the query preparation runs off the runtime thread (#1210).

## v0.3.37 — 2026-10-22

v0.3.36 was tagged but never published or deployed: its full test run caught the validation-budget regression fixed below (#1205). Everything listed under v0.3.36 ships in this release.

### Fixed
- **MCP Hot Reload Answers Every Call It Was Running** (`crates/prod-code-mcp`):
  When the installed client binary changed, the MCP server waited for an empty task set with no deadline, while it kept accepting new calls. Reload now stops reading new requests, delivers the calls that finish within 30 s and answers the rest with a retryable `-32001` "Server is reloading" error before it re-executes. It keeps a partially read request for the new process, and the re-executed binary inherits the same stdin and stdout, so the client connection survives (`server/reload.rs`). The `Transport closed` in #1182 came from the rollout overwriting the signed executable in place, which macOS answers by killing the process. Installers replace the binary by rename now (#1182).
- **Refactorings Validate Against a Cold Engine Again** (`crates/prod-code-mcp`):
  The batch validation budget from #1172 (5 s per file, 10 s to open a session, 90 s per batch) also applied to the check every refactoring runs before it writes. Against a cold rust-analyzer, `code_change_signature`, `code_move` and the rest refused with "was not validated", and an injected diagnostic failure read as "does not type-check". A refactoring's check (`validate_texts`) now waits up to 180 s for a session, 60 s per file and 10 minutes in all. It fails with "was not validated" instead of reporting a compile error when a file cannot be checked. Only `code_validate_edits` keeps the 120 s budget with partial results (`validate_texts_interactive`), which still answers below the MCP client's 150 s limit (#1205).
- **A Rust Engine Can No Longer Deadlock a Gateway on Its VFS Lock** (`crates/prod-code-engine-rust`):
  A gateway on a Linux node stopped answering anything for over an hour. Its accept queue was full (129 of 128), its log stopped, and every thread waited on a futex. Two stacks explained it: a priming task was in `RustEngineSnapshot::file_id_for_path` waiting for a second VFS read lock, and a `didChange` was in `RustEngine::apply_file_change` waiting for the write lock. `file_id_for_path` took the read lock and then called `is_in_view`, which took it again. `std`'s `RwLock` makes a new reader wait behind a queued writer, so the second read waited for the writer, and the writer waited for the first read. The visibility check now runs under the guard already held. `path_lookups_never_deadlock_against_a_vfs_writer` deadlocks on the old code within seconds and passes now (#1190).
- **`validate` Compares Against the Real Pre-Edit Text** (`crates/prod-code-mcp`):
  `prod-code validate FILE --from FILE --with OTHER=OTHER` passed a file with six new E0425 errors and listed them as pre-existing. When the proposed text is already on disk, the disk and the node's synced copy both hold the edit, so the baseline was the edited file itself. Now the baseline of an already-written proposal is the committed `HEAD` blob, while a separate proposal still compares against the checkout. Batch and single-file validation share this choice (`diagnostics/validate/baseline.rs`), and every baseline is opened before diagnostics are requested (#1191).

## v0.3.36 — 2026-10-22

### Fixed
- **Shadow Test Jobs Get Their Own Hypothesis Directories** (`crates/prod-code-gateway`, tests only):
  The test fixture `job()` gave every job nonce 7. Successive overlay and in-place runs in one test shared a hypothesis directory, and a late teardown removed the next run's script (`sh: cannot open .../run.sh`), which failed the coverage gate under load. Each job now takes a distinct nonce (#1183).
- **A Worktree's First Probe No Longer Times Out While Its Copy Is Seeded** (`crates/prod-code-gateway`, `crates/prod-code-mcp`):
  The first probe of a new worktree seeds its copy from the main checkout's. Four seeds at once took 67-70 s on one node, past the client's 30 s probe wait, so `exec` and `outline` failed with `pre-flight workspace sync failed: timed out waiting for sync probe response`, although the gateway finished later. Now the gateway sends a `Pong` every 5 s while it answers a probe or a sync (`session/heartbeat.rs`), and the client's wait counts from the last frame instead of from the request, up to 15 minutes in all (`wait_for_message_capped`). Older clients skip the keepalives as before (#1164, #1165, #1166, #1174).
- **The Coverage Gate Can Pass Again** (`scripts/coverage.py`):
  The gate required every Rust source to appear in the llvm-cov report. llvm-cov reports no file without a function (a `mod.rs` or `lib.rs` that only declares modules, a file of types and constants), and it instruments neither test sources nor build scripts. Since the modular split (#937, #942), about 80 such files failed every run with `no coverage data for: ...`. `omitted_by_llvm_cov` now excuses those files, and modules declared only under `#[cfg(not(unix))]`/`#[cfg(windows)]`. A file with a real function that is missing from the report still fails the gate (#636). A broken fixture in `test_coverage.py` gets its member manifest (#1188).
- **Metrics Server Test Waits for the Server to Listen** (`crates/prod-code-gateway`, tests only):
  `prometheus_http_server_serves_metrics_and_health` connected once after a fixed 50 ms sleep. On a loaded node it got ConnectionRefused and failed the coverage gate. It now retries the connection for up to 10 s (#1192).
- **Batched Lookups, Outline Waits and Missing Paths** (`crates/prod-code-mcp`):
  - `prod-code def --symbol A --symbol B --symbol C` timed out on the third symbol after the second used up the shared 60 s budget. Each remaining symbol now has its own share, a fast lookup leaves its unused time to the later ones, and a timed-out symbol is reported with what to try next while the others still answer (#1168).
  - An outline now gives up after 60 s on routing, sync, engine load or another query's lock, with an actionable message, instead of waiting minutes (#1178).
  - An outline of a checkout path that does not exist says so and suggests close paths (`did you mean src/manager/mod.rs`) before any engine is loaded, instead of `Invalid file path for URI`. Dependency sources that exist only on the node are not affected (#1163).
- **Silent Execs Stay Visible and Never Leave a Stale Running Command** (`crates/prod-code-gateway`, `crates/prod-code-mcp`):
  `prod-code exec -- cargo fmt --all` printed nothing for over five minutes and had to be killed, and afterwards `prod-code cluster` still listed `cargo fmt --all` as running on a node where no such process existed.
  - `stream_exec` (`runner/exec_stream.rs`, shared by plain and structured exec) drops the running-command record as soon as the process exits, answers the client's `Ping` during a run, and kills the process group when the client leaves.
  - The client (`exec/progress.rs`) names the pre-flight step it waits on once it has waited 10 s, so a quick exec prints nothing extra. During a silent command it pings the gateway every 10 s and prints that it is still waiting, and it stops with an actionable message when the gateway goes 30 s without any frame (#1177).
- **Batch Edit Validation Survives Writes and Slow Files** (`crates/prod-code-mcp`, `crates/prod-code-gateway`):
  `code_validate_edits` on a batch of Rust files failed whole with `textDocument/diagnostic failed: cancelled because of pending write` or `query timed out after 60s`. Now a cancelled diagnostic is retried on a fresh snapshot, until a deadline rather than three times. Each file's diagnostic query gets at most 5 s within a shared 90 s batch deadline, which the gateway honours (`prodCodeTimeoutMs`, never above its own 60 s bound). A file that cannot be checked in time is named as incomplete while the other files keep their results (`diagnostics/validate/budget.rs`, `engine.rs`) (#1172, #1175, #1176).

## v0.3.35 — 2026-10-22

### Added
- **`--path FILE` for the Navigation Commands** (`crates/prod-code-client`):
  `prod-code def`, `hover`, `refs`, `callers`, `callees`, `impls` and `supertypes` take their file positionally, while `test`, `check` and the MCP tools spell it `path`. Agents carried that spelling over and got `unexpected argument '--path'`. `--path FILE` and `--path=FILE` are now read as the file of those commands (`cli/argv.rs`). None of them has a `--path` of its own, and arguments after `--` are left alone (#1159).
- **`prod-code pull` and `code_pull` Fetch Build Artifacts From the Exec Node** (`crates/prod-code-gateway`, `crates/prod-code-mcp`, `crates/prod-code-client`, `crates/prod-code-protocol`):
  After `prod-code exec -- cargo build --release --bin NAME`, `prod-code pull target/release/NAME` failed with `... is not a readable source location on this gateway`. It could also ask a different node than the one exec ran on. Pull now uses its own `PullFileRequest`/`PullFileResponse` frames, which start no analyzer. They read from the checkout's own copy on the node, including ignored build outputs and the RAM build target recorded for that copy's last exec (`sync/artifact_pull.rs`, `sync/artifact_target.rs`). Transfers go in 4 MiB chunks and keep the executable bit. Paths outside the copy and symlinks out of it are refused. The client remembers the node of the last `exec` per checkout and pulls from it (`--remote` overrides). Exec, pull and their errors name the node and the copy. The new MCP tool `code_pull` does the same (#1145).

### Fixed
- **Workspace Sync Answers While the Rust Engine Is Busy** (`crates/prod-code-gateway`, `crates/prod-code-mcp`):
  `apply_sync`, `apply_sync_probe` and `handle_session_sync` awaited each loaded Rust engine's lock to update the synced files' base text, holding the workspace sync lock meanwhile. A query stuck on an analysis snapshot keeps that lock, so the sync was never answered and every later probe for the workspace hung behind it (`pre-flight workspace sync failed: workspace sync was not acknowledged: timed out waiting for sync response`, then `timed out waiting for sync probe response`). The files are now written and answered first; `push_base_updates` (`sync/base_updates.rs`) waits at most 5 s for the engines and otherwise lets them take the files in the background, reading each file from disk under the engine's own lock so a late update cannot apply older text. A client that still times out now names the gateway, the wait and the next step (#1143).
- **Exact Declarations in Nested Projects After Root Prefix Matches** (`crates/prod-code-mcp`):
  Since v0.3.34, `symbol_search_across_projects` (`tools/symbols/across_projects.rs`) stopped at the root project's prefix or substring matches, so `code_symbols foo` returned `foobar` but missed `func Foo()` in a nested Go module. With such matches it now asks the nested projects that declare the name in any case (`declared_at`), and spends at most 2 s finding them. It still skips projects that only mention the name, so substring queries stay fast (#1124, #1150).
- **Client CLI Tests Follow the Single-Node Engine Check** (`crates/prod-code-client/tests`):
  Nine mock-gateway tests in `tests/cli.rs` had failed since #1067 made the client refuse a single named node whose status does not list the engine. The mock's status now lists `typescript`. The `.sh` outline test expects the earlier refusal. The recording gateway answers the engine check's status connection without counting it, so the `lsp_*` sessions get the connections they script. `cargo test -p prod-code-client --test cli` passes again (112 of 112) (#1161).
- **The Whole Workspace Test Suite Passes Again** (all crates):
  `cargo test --workspace` failed 46 tests on `main`. Most had been red since releases that did not run it. Product fixes:
  - `execute_tool` keeps its dispatch state on the heap (7130a00e had inlined it, and the `code_pull` branch inlined it again). A tool future had embedded 14,624 bytes in every caller and overflowed debug test stacks; `tests/execution_stack.rs` keeps it under 1 KiB.
  - A malformed member coordinate in a type's outline aborts a qualified lookup again, instead of turning into "no symbol named" (052a967e).
  - A Rust recursive call in the declaring file is again reported as where the references came from. Only same-file Swift uses stay unannotated (#1135).
  - A Swift search whose results came only from the same-target fallback still suggests `build_index: true`.
  - `prod-code change-signature --help` has its Rust and Go long description back, lost in #937.
  - The gateway has a hidden `--no-admission` (`PROD_CODE_NO_ADMISSION`). `.cargo/config.toml` sets it for every gateway a test suite starts, so live tests no longer fail when a shared build node's memory is busy with other services.

  Test fixes: engine seed and cache tests get injected disk budgets and isolated environments. The Rust engine's library tests no longer exhaust the proc-macro farm. Stale expectations were traced to the commits that changed the behaviour on purpose, among them #911 and #1055 (#1179).
- **Rust `mut`/`ref` Bindings Resolve by Their Name** (`crates/prod-code-mcp`):
  rust-analyzer labels a local by its whole pattern (`mut items`, `ref mut items`). Position-free lookups (`code_references`, `code_definition`, `code_hover` with `symbol`) answered `no symbol named \`items\`` and suggested `mut items`, and `code_outline --locals` listed `mut items`. `binding_name` and `binding_column` (`tools/symbols/bindings.rs`) strip the binding mode from Rust variable labels and move the position to the identifier. `collect_named` and `parse_workspace_symbol_hits` use them, and `declared_at` reads `let mut NAME` as a declaration. For a Rust file whose outline has nothing, the file-local declaration fallback now applies to `let` bindings too (#1148).
- **Explicit Dependency Files Answer `--symbol` From Their Own Outline** (`crates/prod-code-mcp`):
  `prod-code def <cargo registry file> --symbol maybe_round` stayed pending and then failed with `no symbol named \`maybe_round\` in the workspace index`, while `prod-code outline` on the same file listed it. The explicit-file fast path in `symbol_search_across_projects` required the file to be readable locally, and a dependency source can exist only on the node. Now a named file that does not exist locally still has its outline asked first. If that outline is empty, `declarations_in` reads the Rust file from the node, waiting at most 2.5 s, and finds the declaration there (#1141).
- **Extract Function Respects Pattern Bindings and Refuses Ambiguous Borrowed Returns** (`crates/prod-code-mcp`):
  The extraction's capture check (`uncaptured_enclosing_locals`) knew only `let` bindings and ignored scope. A selection holding `for p in ...` was rejected with "local variable from enclosing function was not captured" for its own loop variable `p`. `local_bindings` (`extract_function/patterns.rs`) now records `let`, `for`, closure, `match`-arm, `if let` / `while let` and let-chain bindings with their lexical extent, so they no longer count as captures but do not hide a real capture outside their scope either. `check_return_borrows` (`extract_function/return_borrows.rs`) refuses, before anything is written, a helper whose return type has an elided borrow while it has several input lifetimes (`fn f(a: &RustcInvocation<'_>, b: &String, c: &mut Command) -> &Path`), and says how to change the selection (#1146).
- **Sync and Probe Waits Are Visible in the Gateway Log** (`crates/prod-code-gateway`):
  A probe or sync stuck behind its workspace's sync lock logged nothing, so a client-side `timed out waiting for sync probe response` left no trace on the node. `lock_workspace_sync` (`sync/apply.rs`) now warns every 10 s while it waits and once more when it gets the lock, naming the workspace and whether a sync or a probe is waiting. A probe that takes 20 s or more, close to the client's 30 s limit (a large seed, for instance), is logged as a warning (#1155).
- **Rust `code_move`: Bounded Planning, Integration-Test Crates, Child Modules** (`crates/prod-code-mcp`):
  - Moving a generic `std::thread::scope` helper ran for more than 135 s with no output, because every analyzer step got its own 180/300 s allowance. All planning steps now share one 60 s budget (`MoveBudget`, `move_item/budget.rs`). When it runs out, the error names the step and says that nothing was written (#1142).
  - `module_of` refused `tests/*.rs` with "only the ordinary crate layout is understood". Integration-test targets (`tests/NAME.rs`, `tests/NAME/main.rs`) and their declared submodules, `#[path]` included, now resolve as their own crate roots (`move_item/test_crate.rs`). A move into a submodule of the same test crate keeps `#[test]` and adds `use super::*;` so private helpers stay in scope. Moves between different test crates are refused (#1144).
  - A move into a child module (`a.rs` -> `a/b.rs`) copied `super::`/`self::` paths verbatim and left the item private. Now the moved item and its carried imports are rebased one level deeper (`move_item/child.rs`). Private items become `pub(super)`, `pub(super)` becomes `pub(in super::super)`, and `impl` blocks and `macro_rules!` are left alone. The new `mod b;` goes after the file's header comments and inner attributes, not at line 1 (`move_item/preamble.rs`) (#1147).

## v0.3.32 — 2026-10-17

### Fixed
- **Active-Exec Workspace Sync Protection for Newly Generated Files** (`crates/prod-code-gateway`):
  Added active-exec tracking (`begin_active_exec`, `end_active_exec`, `record_synced_during_exec`, `is_active_exec` in `workspace/stale.rs`), wired into `run_exec` and `run_remote_exec`, and updated `reconcile_manifest` (`sync/sync_fs.rs`) so files created or reformatted by an active `exec` (`gofmt`, `sqlc`, `protoc`, etc.) are not overwritten or deleted by concurrent client syncs before writeback while still respecting client deletions of client-synced files (#1104).
- **Concurrent MCP Request Dispatch & Per-Workspace Mutation Locking** (`crates/prod-code-mcp`):
  Replaced the single-flight `serve_mcp_requests` loop (`server/transport.rs`) with a concurrent `tokio::task::JoinSet` and writer channel while preserving task-originated placement failover updates, and added per-workspace `RwLock` serialization (`workspace_mutation_lock` and `is_read_only_tool` in `tools/execution.rs`) so read-only calls run concurrently while mutating/writeback tools (`code_change_signature`, `code_rename`, `code_move`, `code_exec`, `code_shadow_run` with `apply: true`, `code_check`/`code_lint` with `fix: true`, `code_sync`) hold an exclusive write guard (#1105).
- **Root-Anchored `code_impact` Symbol Attribution & Qualified Lookup** (`crates/prod-code-mcp`):
  Updated `changed_symbols` (`impact/symbols.rs`) to anchor LSP symbol queries at `workspace_root` rather than nested non-project directories, and updated `find_symbol` (`impact/signatures.rs`) to strip `impl Trait for Type` containers to `Type` and fall back from qualified names (`Type::method`) to bare symbol names (#1106).
- **Swift Pre-Build Symbol, Definition, Outline & Reference Fallbacks** (`crates/prod-code-mcp`):
  Added executable-code-masked Swift declaration and outline fallbacks (`local_declarations_in_file` in `tools/symbols/search.rs`, `local_swift_outline_symbols` in `tools/outline/file.rs`, and `scan_swift_string` in `caller_migration/mask.rs` supporting multiline `"""..."""` and raw `#"..."#` strings), positional `handle_definition` outline fallback (`tools/symbol_handlers.rs`), and per-file target/module-scoped Swift reference fallback (`tools/references/multi_target.rs`, `tools/references/lexical.rs`) when `sourcekit-lsp` returns empty results before `swift build` (#1107).
- **Single-Line JSX/HTML Tag vs Symbol Rename Detection in Pre-Tool Hook** (`scripts/hooks/prod-code-local-build-guard.py`):
  Added `HTML_INTRINSIC_TAGS` and `is_html_intrinsic_tag_token` so single-line lowercase HTML/SVG wrapper tag swaps (`<div>...</div>` -> `<details>...</details>`) are allowed in `Edit`/`Write` calls while PascalCase JSX component renames (`<OldCard />` -> `<NewCard />`) and TypeScript generic type arguments (`factory<OldType>()` -> `factory<NewType>()`) remain guarded (#1108).
- **Associated Rust Method Resolution (`Type::method`) in `code_symbols` and `code_definition`** (`crates/prod-code-mcp`, `crates/prod-code-engine-rust`):
  Added `find_inherent_method_symbols` in `crates/prod-code-engine-rust/src/snapshot/hierarchy.rs` to index inherent `impl` methods on cold Salsa snapshots, and added `search_qualified_type_members` (`tools/symbols/search.rs`) so `Type::method` queries resolve directly through the owner type's file and members (#1109).
- **Go Receiver Method Disambiguation Over Composite-Literal Field Keys** (`crates/prod-code-mcp`):
  Updated `collect_members` and `find_source_members` (`tools/symbols/type_members.rs`, `tools/symbols/matching.rs`) to match `gopls` receiver-prefixed method names (`(*Store).Method`, `(Store).Method`), skip sibling files that do not mention the owner type, bound sibling scans by engine and `read_name_scan_text`, and reject composite-literal `Field: value` lines when resolving `Type.Method` (#1110).
- **Multi-Segment Qualified Symbol Queries & Relevance Path Capping** (`crates/prod-code-mcp`):
  Updated `handle_symbols` (`tools/symbol_handlers.rs`) and `search_symbols` (`tools/symbols/search.rs`) to split multi-segment `::` queries (`crate::module::Type::method`) into leaf symbol and qualifier segments, and capped `is_relevant_to_Language` (`sync/relevance.rs`) at `MAX_RELEVANCE_DEPTH` (`8`) so deeply nested cache trees do not inflate workspace sync file counts (#1111, #1112).
- **Native TypeScript LSP (`tsgo --lsp`) Detection & Go `GOCACHE` Warmth** (`crates/prod-code-engine-generic`, `crates/prod-code-gateway`):
  Updated `native_typescript_lsp` (`config/presets/core.rs`) to check `which_bin("tsgo")` on `PATH` for `@typescript/native-preview` before falling back to `vtsls` or `typescript-language-server`, and exported `GOCACHE` alongside `GOMODCACHE` in `shared_cache_env` (`seed_cache/build_cache.rs`) so `gopls` and `go test` share warm build artifacts across workspace checkouts (#1113).

## v0.3.31 — 2026-10-14

### Fixed
- **Cross-Package Go Constant & Value Moves in `code_move`** (`crates/prod-code-mcp`):
  Added `find_go_decl_end` (`move_polyglot/decl.rs`) so single-line `const`/`var`/`type` declarations do not swallow following braced blocks, added `rewrite_go_call_sites_in_context` (`move_polyglot/go_refs.rs`) and `is_named_map_or_slice_type` (`move_polyglot/go_imports.rs`) to qualify exported value and constant references (including named and imported map/slice/array composite-literal keys) while preserving struct field keys and rejecting shadowed local/parameter bindings or unexported symbols, and scanned sibling `.go` files in the source package (`move_polyglot/callers.rs`, `move_polyglot/execute.rs`) when LSP reference discovery omits value uses (#1101).

## v0.3.30 — 2026-10-14

### Fixed
- **Pull & Remote File Read Interleaved LSP Notification Handling** (`crates/prod-code-mcp`):
  Updated `pull_remote_files` (`sync/pull.rs`) and `read_remote_file` (`remote_fs/mod.rs`) to ignore asynchronous `WireMessage::LspPayload` (`window/logMessage`) and `WireMessage::Pong` frames with fixed `timeout_at` deadlines while awaiting `HandshakeResponse` and `ReadFileResponse`, preventing `prod-code pull` from aborting with `unexpected message reading file` (#1078).
- **External Toolchain Source Path & Remote Mirror Resolution** (`crates/prod-code-mcp`):
  Fixed `resolve_file_path` (`tools/compile_gate.rs`) to preserve existing absolute external paths (such as Rust toolchain library sources under `.rustup/toolchains/.../library/core/src/error.rs`) and map remote workspace mirror paths via `workspace_relative_path`, while updating `resolve_symbol` (`tools/symbols/resolve.rs`) to match relative path hints and award the checkout ranking bonus to remote mirror paths (#1079).
- **Concurrent Placement Discovery & Resilient MCP Server Startup** (`crates/prod-code-mcp`, `crates/prod-code-client`):
  Updated `discover_nodes_with_paths` (`cluster/discover.rs`) to stop after the first responsive seed's `cluster_view` and probe unverified remembered placements concurrently via `join_all`, matched `"closed the connection"` in `is_retryable_connection_error` (`server/failover.rs`), and allowed `Commands::Mcp` (`client/src/main.rs`) to fall back to the first configured seed on transient startup discovery failures (#1080).
- **Deep Nested Swift Subproject Routing & Declaration Discovery** (`crates/prod-code-mcp`):
  Raised `MAX_SEARCH_DEPTH` from 6 to 10 in `nested_projects.rs`, routed nested subproject queries through `route_for_path` in `execute_lsp_query` (`tools/compile_gate.rs`) and `across_projects.rs`, separated the `declarations_in` query deadline from the initial Swift sync deadline, and added Swift `extension` and `typealias` keywords to `declared_at` (#1081).
- **Sticky Build-Node Sessions & Disk-Authoritative Placement Cache** (`crates/prod-code-mcp`):
  Updated `handle_cluster_rebalance_tick` (`server/rebalance.rs`) to keep active MCP sessions pinned to their current node while `status.host.pressure().is_none()` and the node fits the workspace, delayed the initial rebalance timer tick by 30 seconds (`server/transport.rs`), recorded `remember_placement` on `ExecExit` (`exec/runner.rs`), and made `load_placement` (`cluster/placement.rs`) read disk-authoritative state so external placement rewrites and removals take effect immediately (#1082).
- **Nested Git Worktree Exclusion & Component Reference Discovery** (`crates/prod-code-mcp`):
  Excluded nested git worktree directories (child directories containing `.git`, such as `.prod/worktrees/*`) from `source_files`, `workspace/symbol`, and `add_locations`; added `tools/references/lexical.rs` and extended `caller_migration/mask.rs` to resolve relative and multi-line imports, preserve template string interpolations (`${...}`), Python f-strings (`f"...{...}..."`), Vue directive expressions (`v-*`, `:`, `@`, `#`), and JS/TS regex literals (`/pattern/flags`), while filtering shadowed bindings, comments, and unverified text matches (#1071).
- **Single-File Outline Readiness & Worktree Base Placement Keys** (`crates/prod-code-protocol`, `crates/prod-code-mcp`):
  Removed `"textDocument/documentSymbol"` from `needs_index` (`readiness/types.rs`) so single-file outlines do not block on workspace-wide indexing, and normalized engine-specific placement keys in `session/pool.rs` and `session/connection.rs` to `ws_identity.base.as_deref().unwrap_or(&ws_identity.name)` (#1083).
- **Same-Type Rust Inherent Method Moves** (`crates/prod-code-mcp`):
  Added `move_inherent_method` (`move_item/item.rs`, `move_item/execute.rs`) to move a Rust inherent method into an existing, verified same-owner `impl` block in another module of the same crate while promoting private visibility to `pub(crate)` and validating caller files (#1084).
- **Rust `code_change_signature` Fallback Rewrites & Typed-Local Coercion Safety** (`crates/prod-code-mcp`):
  Added `rewrite_file_calls` and `apply_fallback_rewrites` (`signature/rewrite.rs`, called from `signature/change.rs`) with `signature/util.rs` helpers to rewrite call sites directly from analyzer reference positions when SSR cannot parse a default expression or omits associated-function calls (#1085, #1086), and recorded `typed_locals` in `signature/types.rs`, `signature/call_sites.rs`, and `signature/effects/hazards.rs` so local bindings whose explicit reference type matches a removed parameter type are classified safely (#1089).
- **Pinned `pull` Handshake Redirects & `pull`/`exec` CLI Routing** (`crates/prod-code-mcp`, `crates/prod-code-gateway`, `crates/prod-code-client`):
  Set `redirects: false` in `pull_remote_files` (`sync/pull.rs`) and skipped `check_cluster_redirects` for `client_name == "prod-code-pull"` (`crates/prod-code-gateway/src/session/handshake.rs`), and excluded `Commands::Pull` and `Commands::Exec` arguments from `command_path_tokens` (`crates/prod-code-client/src/cli/candidate.rs`) so `pull` and `exec` route to the workspace's primary placement without language-hint overrides (#1087, #1088).
- **Proven Pure-Borrow Re-Acquisition in Rust `code_extract_function`** (`crates/prod-code-mcp`):
  Added `reacquire_conflicting_borrows` and `conflicting_borrow_captures` (`extract_function/borrows.rs`, `extract_function/tokens.rs`), wired into `extract_function/execute.rs` and `extract_function/duplicates/validate.rs`, to re-acquire proven pure field borrows (`&<param>.<field>` and verified standard `Option::as_ref()` projections) inside extracted helpers when the binding is not used after the selection, or report `E0502` when it remains live (#1090).
- **Swift Subclass `override` Symbol Supplementation** (`crates/prod-code-mcp`):
  Updated `declarations_in` and `files_naming` (`tools/symbols/across_projects.rs`) to scan a bounded set of relevant `.swift` files and supplement `workspace/symbol` results with `documentSymbol` declarations omitted by `sourcekit-lsp` (#1091).
- **External-Checkout `code_search` Routing** (`crates/prod-code-mcp`):
  Updated `handle_search` (`tools/analysis_handlers.rs`) to validate explicit `path` arguments, detect canonical paths outside `workspace_root` via `other_checkout`, and route searches to the external repository's root and placement (#1092).
- **Go `code_safe_delete` Non-Go Artifact & Bundle Symlink Filtering** (`crates/prod-code-mcp`):
  Updated `refuse_linked_sources` (`safe_delete_go/directives.rs`) to skip build/framework artifact directories (`.build`, `*.framework`, `*.xcframework`) and only refuse directory symlinks whose targets contain `.go` files (#1094).
- **Manifest-Scoped Nested Engine Detection for Package Subdirectories** (`crates/prod-code-mcp`):
  Added `manifest_engine_at` (`sync/engine/detect.rs`) and updated `engine_project` and `expected_engine` (`sync/engine/project.rs`), `engine_for_path` (`cluster/routing.rs`), and `representative_source_file` (`tools/symbols/nested_projects.rs`) so package subdirectories containing embedded auxiliary files (`schema.sql`, `.proto`, `.sh`, `Dockerfile`) alongside primary language sources remain with their enclosing project's engine while preserving Vue/Svelte component detection (#1097).
- **Serialized Workspace Sync & Manifest-First Batch Ordering** (`crates/prod-code-mcp`, `crates/prod-code-gateway`):
  Added per-`(workspace, node)` client serialization via `workspace_sync_lock` (`sync/cache.rs`, `sync/push.rs`) and per-request gateway serialization via `server_workspace_sync_lock` (`crates/prod-code-gateway/src/sync/apply.rs`, `sync/sync_fs.rs`), sorted project manifests (`go.mod`, `Cargo.toml`, `Package.swift`, `pyproject.toml`, `package.json`, etc.) ahead of other files in `sync_batches` (`sync/entry.rs`), avoided creating `.prod-code-last-used` prematurely during `apply_sync_probe` on fresh unseeded workspaces with missing files, and required a full-manifest upload before accepting `workspace_was_fresh` in `push_sync_round` (#1098).

## v0.3.29 — 2026-10-11

### Fixed
- **Engine-Aware Subproject & Directory Routing** (`crates/prod-code-mcp`):
  Added `engine_for_path` checking nested subprojects, path manifests, file extensions, and directory contents for target engines. Integrated with `default_serves` check in `route_in` to route Swift test directories (`swift/Tests/...`) and root Swift projects to compatible nodes instead of immediately failing with `Transport closed` when default node is Linux (#1059).
- **Single-Node Engine Compatibility Fast-Fail** (`crates/prod-code-mcp`):
  Added engine compatibility validation in `pick_node_with` for single-node placements, returning an actionable diagnostic rather than crashing with closed transport (#1060).
- **Session Pool Engine-Placement Preservation** (`crates/prod-code-mcp`):
  Updated `pooled_query_with_budget` in `pool.rs` to look up engine-specific placement keys (`{workspace}#{engine}`) and verify `default_serves(remembered, eng)` before updating `target_remote`, preventing secondary language sessions (Swift/Python) from being retired and overwritten with incompatible root workspace nodes (#1060).
- **Interactive CLI Query Timeout Bounding** (`crates/prod-code-client`):
  Wrapped `execute_tool` calls in `run_tool` and `run_refs` (`commands/common.rs`, `commands/query/navigation.rs`) with `resolve_tool_call_timeout(&args)` and actionable error reporting upon timeout, preventing `prod-code def` and `prod-code refs` commands from stalling indefinitely on unindexed symbols or cold language servers (#1061, #1062).
- **Workspace Sync Benchmark & Dataset Code Preservation** (`crates/prod-code-mcp`):
  Updated `is_path_filtered_with_profile` in `filters.rs` to detect source files (`.rs`, `.py`, `.c`, `.cpp`, `.ts`, `.swift`, etc.) inside tracked directories named `benchmarks/benches` or `dataset/datasets`, ensuring tracked benchmark schemas and fixture code reach the remote mirror without breaking `cargo check` or `pytest` runs (#1063, #1064).
- **Tool Call Execution Timeout Propagation & Ceiling** (`crates/prod-code-mcp`):
  Updated `resolve_tool_call_timeout` in `dispatch.rs` to honor client-specified `timeout_secs` (and `timeout_millis`) with a 5-second buffer, while clamping to `MAX_TOOL_CALL_TIMEOUT` (24 hours = 86400s). Long-running executions such as `code_exec` with `timeout_secs=2400` are no longer killed by the default 150s limit (#1065).

## v0.3.28 — 2026-10-08

### Fixed
- **Rust Type Alias & Re-Export Resolution** (`crates/prod-code-mcp`):
  Added `find_type_alias_target` and `extract_alias_target` in `alias.rs` with exact right-hand identifier boundary enforcement to resolve member references through type aliases and re-exports (`type Alias = Target;`, `as Alias`) in `code_references` (#1025).
- **Enclosing Local Variable Capture Validation in Function Extraction** (`crates/prod-code-mcp`):
  Added `uncaptured_enclosing_locals` in `binding.rs` to detect and reject Rust extractions where enclosing local variables or parameters are referenced in the helper or formatted string interpolations (`{var}`) without being declared as parameters or bound internally (#958).
- **Context-Aware Prelude Macro Filtering in Test Modules** (`crates/prod-code-mcp`):
  Routed unresolved Rust prelude macros (`assert_eq!`, `vec!`, `format!`, etc.) to `auto_trait` as analyzer limitations strictly in test module contexts, while preserving genuine user errors when prelude is explicitly disabled via `#![no_implicit_prelude]` or `#![no_std]` (#950).
- **Standard Library Symbol Resolution Fallback** (`crates/prod-code-mcp`):
  Added `find_stdlib_or_usage_hits` in `resolve.rs` to discover occurrences and usages across workspace files when `workspace/symbol` index lacks `std::*`, `core::*`, or `alloc::*` symbols (#935).
- **Swift Identifier Coordinate & Call-Site Resolution** (`crates/prod-code-mcp`):
  Aligned symbol columns with `find_identifier_on_line` to resolve Swift outline/index position discrepancies (#887, #888), and permitted Swift call sites discovered via `writes_call` without requiring file-level import statements (#885).
- **Opt-In SwiftPM Index Builds** (`crates/prod-code-mcp`, `crates/prod-code-client`):
  Required explicit `build_index: true` (or `--build-index`) before launching SwiftPM package index builds for references, preventing unintended compilation runs on read-only queries (#933).
- **Workspace-Relative Path & Leading Slash Trimming** (`crates/prod-code-mcp`):
  Trimmed leading slashes when resolving outline file paths against workspace root (#913), and mapped remote server mirror URLs in `definition_body` to local checkouts with workspace identity validation (#979).
- **In-Flight Loading Workspace Preservation** (`crates/prod-code-gateway`):
  Preserved in-flight `LoadState::Loading` workspaces during `unload_under` and permitted loader leader fulfillment (#911).
- **Nested Cargo Package Discovery** (`crates/prod-code-mcp`):
  Discovered nested Cargo packages sharing the root engine in `projects_naming` and fell back to nested anchors on fuzzy matches (#889).
- **Bounded Impact BFS Traversal** (`crates/prod-code-mcp`):
  Added 30s query timeout on incoming calls and a 60s traversal budget with deadline propagation across hierarchy items (#1021).
- **Fast Sync Probe Timeout** (`crates/prod-code-gateway`, `crates/prod-code-client`):
  Reduced preflight workspace sync probe response timeout from 180s to 30s to fail fast instead of hanging on stalled syncs (#999).
- **Session Placement Under Memory Pressure & Congestion** (`crates/prod-code-gateway`, `crates/prod-code-client`):
  Evaluated node memory and load pressure during placement to migrate or route active sessions away from congested nodes to quiet nodes, avoiding capacity refusals during symbol lookups (#941).
- **JSON-RPC Error Response ID Preservation** (`crates/prod-code-gateway`, `crates/prod-code-client`):
  Ensured `id` field is always populated in JSON-RPC error frames so clients do not treat errors as protocol violations or close the transport (#948).
- **Cluster Toolchain MSRV Selection & Candidate Failover** (`crates/prod-code-gateway`, `crates/prod-code-client`):
  Added compiler MSRV rejection detection and automatic failover across cluster candidate remotes for check, lint, test, and benchmark runs when a node's default compiler is below workspace requirements (#962, #1022).
- **Contained Symlink Mirroring & Bounded Sync Reads** (`crates/prod-code-mcp`):
  Safely preserved tracked contained symlinks in remote workspace mirror with `fits_sync` checks prior to reading and bounded buffer sizes (#1003).
- **Go Move Package Declaration Initialization** (`crates/prod-code-mcp`):
  Ensured package declarations are initialized when moving Go files to new target directories (#1007).

### Added
- **Unified Gateway Metrics, Resource Snapshots & Bounded Retention** (`crates/prod-code-gateway`, `crates/prod-code-protocol`):
  Unified telemetry collection across all gateway operations (`lsp`, `exec`, `remote_exec`, `sync`, `search`, `shadow`, `read_file`, `place`, `status`) into low-cardinality `OperationMetric` records. Added periodic host and process resource snapshots (`HostSnapshot`) capturing CPU utilization, gateway RSS, host memory available/used, and workspace disk space, along with cached toolchain component inventory (`ToolchainInventory`). Disk persistence writes to daily JSONL files under bounded retention (14-day and 500 MB quota).
- **Prometheus Scrape Endpoint & Pushgateway Client** (`crates/prod-code-gateway`):
  Added native Prometheus exposition supporting both pull HTTP scrape endpoint (`GET /metrics` and `/health`) and periodic Pushgateway client with customizable job and instance labels.
- **Declarative TOML Configuration Support** (`crates/prod-code-gateway`, `gateway.example.toml`):
  Added automatic discovery and parsing of `gateway.toml` (`--config`, `/etc/prod-code/gateway.toml`, `~/.config/prod-code/gateway.toml`, `~/.prod-code/gateway.toml`, `./gateway.toml`) with strict CLI and environment variable precedence.
- **Compilation Telemetry & Workspace Storage Sizing** (`crates/prod-code-gateway`):
  Tracks compilation tool invocations (`rustc`, `go`, `clang`, `swift`, `tsc`), measuring compile durations and classifying failures. Background janitor computes non-blocking workspace directory storage.

## v0.3.26 — 2026-10-06

### Changed
- **More useful symbol navigation** (`crates/prod-code-mcp`): `code_definition` now returns the symbol body by default; pass `body: false` for a location-only response (#905).
- **Workspace-root execution and test aliases** (`crates/prod-code-client`, `crates/prod-code-mcp`): `code_exec` accepts the workspace root as its working directory, and `code_test` accepts `test` / `test_filter` and `package` / `crate` aliases. Fallback declaration lookup recognizes `var`, `let`, and `val`; the Claude Code guard resolves project transcripts by session and workspace and denies unverified bypasses (#903).
- **Faster qualified-symbol lookup** (`crates/prod-code-mcp`): member lookup tries the owning type's outline before broad workspace-symbol search and validates module qualifiers in the fallback path (#898).

### Refactored
- **Comprehensive Modular Decomposition Across All Crates**:
  Decomposed 104 monolithic source files exceeding 400 lines across `crates/prod-code-*` into cohesive, focused submodules. At the v0.3.26 release, several large production files still exceeded 400 lines, including `extract_function_polyglot/tokenize.rs` (991 lines), `extract_field/polyglot/target.rs` (504), and `dataflow/slice.rs` (409); the decomposition was substantial progress, not completion of the compact-source target.
  - `crates/prod-code-testkit`: decomposed `gopls` bridge, workspace, and mock infrastructure.
  - `crates/prod-code-protocol`: modularized codec, discovery, DNS, transport, TLS, and message serializers.
  - `crates/prod-code-engine-go`, `crates/prod-code-engine-generic`, `crates/prod-code-engine-rust`: modularized LSP lifecycle, config, and document management.
  - `crates/prod-code-gateway`: decomposed admission, backend, search, shadow, and language caches.
  - `crates/prod-code-mcp`: decomposed `parameter_object` (5,791 lines into 26 submodules), `verify` (3,575 lines), `wrap_return` (2,355 lines), `signature`, `signature_go`, `impact`, `sync`, and refactoring handlers.

### Fixed
- **Polyglot Wrap Return Semantic Reference Persistence** (`crates/prod-code-mcp`):
  Ensured `collect_and_rewrite_callers` preserves semantic filtering across the entire workspace scan when the analyzer provides references, preventing unintended fallback to lexical matching when the final reference is consumed.

## v0.3.25 — 2026-10-05

### Added
- **Cluster Rollout Automation Tool** (`scripts/cluster-rollout.py`, `scripts/cluster-rollout.sh`):
  Added automated and interactive rolling update tooling for the prod-code gateway fleet across Linux and macOS nodes. Includes live node health inspection, disk/memory pressure filtering, fast SCP staging and systemd service verification, rollback on failure, and local client updates.
- **Strict TLS Auto-Discovery and Plaintext HTTP Interception** (`crates/prod-code-gateway`, `crates/prod-code-protocol`):
  Enforces encrypted cluster communication by auto-discovering certificates in `~/.prod-code/tls/`. Intercepts incoming plaintext HTTP requests on port 9400 with a lightweight RFC-compliant request sniffer, returning `HTTP/1.1 426 Upgrade Required` to direct clients to TLS endpoints.
- **AST Refactoring Promotion & Contextual Tips** (`crates/prod-code-mcp`, `crates/prod-code-protocol`):
  Embedded actionable refactoring guidance into `code_references` output suggesting `code_rename` when multiple symbol usages are detected, and reinforced AST refactoring adoption in MCP system prompts and tool descriptions.
- **Developer ID Notarization and macOS DMG Packaging** (`scripts/packaging`):
  Integrated Apple Developer ID Application codesigning, Apple notarytool submission with keychain credentials, and ticket stapling for macOS DMG releases.

### Fixed
- **RFC 9110 Token Method Parsing & Extended URI Target Matching** (`crates/prod-code-protocol`):
  Expanded the gateway HTTP protocol sniffer to parse arbitrary RFC 9110 token methods (including SSDP/UPnP `M-SEARCH`) and full URI request targets (including bracketed IPv6 literals).
- **CLI Error Cause Chain Preservation** (`crates/prod-code-client`):
  Preserved the full anyhow error cause chain (`{e:#}`) on client command execution failures rather than discarding underlying network or handshake context.
- **Issue Reporting Scoping** (`crates/prod-code-client`, `crates/prod-code-mcp`):
  Clarified diagnostic collection and privacy sanitization scopes across MCP and CLI reporting interfaces.

## v0.3.24 — 2026-10-05

### Added
- **Cross-Worktree Build Caches and Editor LSP Supervision (Phases 3.3–3.7)** (`crates/prod-code-gateway`):
  Added dedicated editor-server health supervision and cross-worktree cache support for clangd/PCH, Python stubs, Swift modules, and TypeScript declarations. Cache seeding and pruning use bounded, symlink-aware handling.
- **Isolated Proc-Macro Worker Farm (Phase 5.4)** (`crates/prod-code-engine-rust`, `crates/prod-code-gateway`):
  Added node-wide worker permits, fair-share allocations, bounded waits, sandbox launcher validation, and fail-closed memory limits for procedural macro servers.
- **Generic LSP Diagnostics and Polyglot Outlines** (`crates/prod-code-engine-generic`, `crates/prod-code-mcp`):
  Preserve generic language-server exit details, add Protobuf outlines, improve Markdown front-matter handling, and return typed CLI errors for empty hover/definition results.
- **Protocol Framing Compatibility** (`crates/prod-code-protocol`):
  Added negotiated NUL completion markers while retaining compatibility with markerless peers.

### Fixed
- **Polyglot Refactoring Safety** (`crates/prod-code-mcp`):
  Hardened transformations across Rust, TypeScript/JavaScript, Python, Go, C/C++, and Swift to preserve control flow, argument and receiver behavior, and analyzer-resolved references. Apply operations refuse incomplete or unmatched plans before writing.
- **Bounded Symbol Search and MCP Calls (#829)** (`crates/prod-code-mcp`):
  Enforced nested-project and unindexed-source query budgets, bounded in-flight LSP operations, and clamped tool-call timeouts below the client deadline.
- **Workspace Sync, Editor Reconnect, and Remote File Safety** (`crates/prod-code-client`, `crates/prod-code-mcp`, `crates/prod-code-protocol`):
  Improved reconnect state replay and redirect validation, constrained mirrored paths, rejected symlinked sync inputs, preserved explicit executable modes, and handled stale workspace copies more safely.
- **Packaging and Remote Execution Reliability (#875, #883)** (`crates/prod-code-client`, `crates/prod-code-gateway`, `scripts/packaging`):
  Added clearer architecture-specific packaging checks, explicit working-directory errors, protections against nested worktree execution, and honored requested output-tail limits.
- **ReadFileRequest Protocol Dropping in Active Sessions & 64MiB Artifact Pulls (#820)** (`crates/prod-code-gateway`, `crates/prod-code-mcp`):
  Added `WireMessage::ReadFileRequest` dispatch to active session message loop (`on_client_message`) in the gateway, preventing requests from being dropped after session handshake. Raised maximum file pull limit for workspace files from 2 MiB to 64 MiB (`MAX_PULL_BYTES`). In `pull_remote_files`, added a 30s timeout per frame, explicit truncation and error checks, and executable detection (ELF, Mach-O, shebang) to ensure pulled binaries retain executable permissions (`0o755`).
- **Multiline Function Header Slice Panic in Expression Synthesis** (`crates/prod-code-mcp`):
  Guarded multiline parameter extraction against empty slicing ranges when the target line is a multiline function signature itself without closing parentheses, preventing panic under expression synthesis queries.

## v0.3.23 — 2026-10-02

### Added
- **Git Commit Hash & Version Telemetry in Cluster and Diagnostics Reporting** (`crates/prod-code-protocol`, `crates/prod-code-gateway`, `crates/prod-code-client`, `crates/prod-code-mcp`):
  Compiles git commit hash at build time via `crates/prod-code-protocol/build.rs` and exposes `prod_code_protocol::git_commit()`. Added `version` and `git_commit` fields to `StatusResponse` wire messages. Display gateway version and commit hash in `prod-code status` probe, `prod-code cluster` node engine listings, and embed client and remote node commit hashes into `prod-code report-issue` / `code_report_issue` environment diagnostics.

### Fixed
- **Sync Loop Stack Overflow Prevention on Fresh Workspaces (#817)** (`crates/prod-code-mcp`):
  Replaced recursive retry between `sync_workspace_with_metrics` and `ensure_workspace` with a bounded iterative loop (`MAX_RESET_RETRIES = 2`), preventing `thread main has overflowed its stack` crashes during workspace synchronization with generated or excluded build outputs.
- **False Positive Stale-Reference Filter on Moved Return-Object Properties (#818)** (`crates/prod-code-mcp`):
  Refined symbol search recursion in `validate_property_access_references` to skip nested function and method declaration blocks, preventing returned object properties moved into helper functions or switch branches from triggering spurious `prod-code::stale-reference` warnings.
- **Mixed-Language Validation Baseline Preservation on Cold Generic LSP Engines (#819)** (`crates/prod-code-engine-generic`, `crates/prod-code-mcp`):
  Fixed premature diagnostic timeout in `GenericLspEngine::current_diagnostics_for` by waiting the full timeout when document text was sent (`known || has_published`), preventing cold generic servers (such as `tsc --lsp` in mixed Go/JS projects) from failing initial baseline queries. Cached full pull diagnostics in `self.diagnostics` for subsequent unchanged pulls, and added an automatic single retry with 500ms backoff in `on_disk` queries.

## v0.3.22 — 2026-10-02

### Added
- **Single-Owner Direct-Edit Fast Path & Overlay Bypass (Phase 2.2)** (`crates/prod-code-gateway`, `crates/prod-code-engine-rust`):
  Dedicated worktrees with single-owner sessions bypass overlay crate cones and apply unsaved `didOpen` / `didChange` edits directly into base Salsa file inputs in memory. When a second session joins the same worktree, the direct-edit lease is dynamically revoked via `Arc<AtomicBool>`, and in-memory edits are atomically migrated under the engine mutex into session overlays while the clean on-disk text is restored into base Salsa DB (#808, #817).
- **Polyglot Remote Build & Test Execution Wire Protocol (Phase 6.1)** (`crates/prod-code-protocol`, `crates/prod-code-gateway`, `crates/prod-code-mcp`):
  Introduced typed wire execution messages (`RemoteExecRequest`, `RemoteExecCommand`, `RemoteExecStream`, `RemoteExecTestEvent`, `RemoteExecResult`) supporting remote build checks, unit tests, and benchmarks across Rust, Go, Python, TypeScript, C/C++, and Swift with streaming raw output chunks and structured diagnostic/test-event parsing for Rust (Cargo JSON) and Go (JSON and verbose raw).
- **Discovery Privacy, Replay-Resistant Challenge, and Cluster PKI (Phase 5.6)** (`crates/prod-code-protocol`, `crates/prod-code-gateway`, `crates/prod-code-client`):
  Added mutual TLS cluster authentication, peer certificate verification, replay-resistant challenge discovery nonces, and `prod-code cert` CLI tooling for CA root and leaf certificate provisioning.
- **Dynamic Cluster Load Balancing & Transparent Migration**:
  Added congestion scoring with conservative telemetry penalties and transparent client redirection to cluster peers based on workspace placement, memory pressure, and engine affinity.
- **Parser-Backed Validation for Markdown & SVG Proposals** (`crates/prod-code-mcp`, `crates/prod-code-client`):
  Added dedicated in-memory syntax parsers for Markdown documentation and XML/SVG vector graphics with 0 ms LSP round-trips (#778).
- **Mixed-Language Validation Routing**:
  Validation batches containing both source code and documentation/assets are automatically routed and verified in a single unified overlay (#778).

### Fixed
- **Blocking Stdio Enforcement & Stderr Panic Prevention** (`crates/prod-code-gateway`, `crates/prod-code-client`):
  Enforced blocking mode (`libc::O_NONBLOCK` cleared) on standard I/O streams across gateway and client processes, eliminating `Resource temporarily unavailable (os error 11)` panics when writing diagnostics or logs under systemd / pipe supervision (#806, #812, #814).
- **Stale Worktree & Workspace Idle Pruning** (`crates/prod-code-gateway`):
  Automatic background pruning of stale worktree directories and unreferenced workspaces upon idle timeout and low disk headroom, preventing node storage exhaustion.
- **Shadow Run Timeout & Response Handling** (`crates/prod-code-client`, `crates/prod-code-gateway`):
  Bounded `run_shadow_once` frame reads with deadlines, added full `WireMessage::ShadowRunRequest` support in scripted test gateway, and bounded stdout buffers to prevent test hangs.
- **Cluster Validation Failover Under Memory Pressure** (`crates/prod-code-gateway`):
  When a gateway node under memory pressure returns `CapacityRefused` on `validation_view()`, requests transparently fail over to alive, low-pressure cluster peers serving the requested engine, or to the generic validation session for non-C++ languages (#799).
- **Direct Connect & BSD Socket ARP Resilience** (`crates/prod-code-client`, `crates/prod-code-protocol`, `crates/prod-code-mcp`):
  Explicit `--remote` targets strictly connect to the specified node without falling back to cached loopback placements (`127.0.0.1:19400`). Added exponential backoff retry for macOS BSD socket `EHOSTUNREACH` (errno 65) during cold ARP lookup, raised probe timeout from 400ms to 1500ms, and annotated placement views (#798).
- **JavaScript Function Attribution & Polyglot Impact Analysis** (`crates/prod-code-gateway`, `crates/prod-code-client`):
  Impact analysis correctly attributes edits inside JavaScript and TypeScript arrow functions, variable assignments (`const fn = () => {}`), and nested closures in mixed-language checkouts (e.g. Go backend + JS extension), avoiding unnecessary full-test suite runs when edits are within functions (#800, #802).
- **Shadow-Run Compiler Error Retention** (`crates/prod-code-gateway`, `crates/prod-code-client`):
  Retain compiler errors in `shadow-run` outputs even when compiler warning logs exceed the tail byte cap (#794, #797).
- **Cross-Target Reference Discovery in Shared Trees** (`crates/prod-code-mcp`):
  `code_references` discovers symbol usages across all configured targets and crates sharing source trees (#783, #795).
- **Transport Closure & Definition Capacity Failover** (`crates/prod-code-mcp`):
  Added connection recovery and cluster gateway capacity failover for `code_definition` queries (#792, #780, #784, #793).
- **Unindexed Struct & Class Field Resolution** (`crates/prod-code-mcp`):
  `code_symbols` resolves unindexed struct and class fields by bare symbol name without requiring explicit enclosing type path (#791).
- **Remote Dependency Source Fetching in Slice** (`crates/prod-code-mcp`):
  `code_slice` automatically fetches remote dependency source text from the gateway when missing from local checkout cache (#789).
- **Rust-Analyzer Worktree Overlay Pinning** (`crates/prod-code-engine-rust`):
  Pinned `rust-analyzer` (ra_ap_ide) to verified revision `3abd2c9` with worktree overlay support, filtering `file_id_for_path` by worktree view (#787).

## v0.3.21 — 2026-10-01

### Added
- **Cluster UDP Multicast Discovery & HMAC-SHA-256 Authentication** (`crates/prod-code-gateway`, `crates/prod-code-client`, `crates/prod-code-mcp`): Nodes announce loaded workspaces, host capacity, and supported engines over UDP multicast (`PROD_CODE_DISCOVERY_ADDR` / `239.255.42.99:9401`). Announcements are mutually authenticated using constant-time HMAC-SHA-256 derived from `cluster_auth_token`, filtering forged/spoofed packets and detecting node eviction.
- **MCP Hot-Failover & Dead Peer Eviction** (`crates/prod-code-mcp`, `crates/prod-code-client`): MCP client automatically tracks live gateway nodes via gossip and discovery, detects node failures and connection drops, evicts unreachable peers, safely retries idempotent calls, and routes requests to cluster nodes matching required language engines (#779).
- **Shared In-Memory RustEngine for Git Worktrees** (`crates/prod-code-gateway`, `crates/prod-code-engine-rust`): Eliminates duplicate multi-gigabyte engine memory footprint (saving ~94 GB on cluster nodes). Secondary git worktrees attach isolated VFS overlays onto the shared base `RustEngine`, avoiding duplicate analysis databases and memory exhaustion (#777).

### Fixed
- **Worktree Refactoring Context Resolution**: Refactoring results, assists, and renames initiated in a worktree resolve file paths to the requesting worktree root instead of the shared base checkout.
- **Worktree Detection for Git Worktrees**: Added support for standard Git worktrees containing `.git` gitdir pointers alongside server worktree naming (`--wt-<hash>`).
- **Base Eviction Protection**: Active worktrees increment the base workspace's attached worktree count, preventing the shared engine from being evicted as idle while worktrees are in use.
- **Deferred Overlay Detachment**: Overlay detachment and base unpinning are deferred until all active leases and sessions on the worktree retire.
- **Validation Engine Attachment Refcounting**: Balanced reference counts across validation engines, transactional commit on successful overlay mount with automatic rollback on partial failures, and prevention of duplicate attachments on memory-pressure validation fallback.


## v0.3.19 — 2026-09-29

### Added
- **Expanded polyglot language matrix from 50 to 60 languages** (`crates/prod-code-gateway`, `crates/prod-code-mcp`, `crates/prod-code-protocol`): added PowerShell, Starlark/Bazel, HCL/Terragrunt, Typst, WebAssembly Text/WAT, SystemVerilog, VHDL, Ballerina, Jsonnet, and Cue, plus F# LSP mapping, with manifest/marker detection, disambiguation, priority ordering, and verification via `test_universal_language_detection_matrix_all_60_languages` (#732).
- **Windows Named Pipe client transport** (`crates/prod-code-protocol`): added `AnyStream::NamedPipe`, `connect_named_pipe`, and `connect_named_pipe_with` for Windows local execution (#731).
- **Master Epic #516 closure and Phase 7.1 delivery**: completed the full catalog of 33 polyglot refactorings across TypeScript/JavaScript, Python, Go, C/C++, Swift, and Rust; expanded MCP server to 63 tools with constructor-to-factory/builder, inheritance-to-delegation, conditional-to-polymorphism, and interface extraction (#731).

### Fixed
- Rust multi-file validation snapshots symbols before edits and compares them after the complete
  proposal is open. A rust-analyzer unused-import warning on a public re-export is filtered only
  when an unchanged checked caller references the same definition (#616).
- `prod-code validate --compile` now reports when its remote compiler check is running (#620).
- Incomplete wire frames no longer reserve memory for payload bytes that have not arrived (#621).
- Whole-repository coverage checks fail when an existing Rust source file has no measurement (#622).
- Schema rename no longer edits arbitrary prose and converts the requested column to UTF-16
  before calling the language server (#623).
- Go test and benchmark verification retain package compile failures and emit them as
  diagnostics even when the compiler produces no test events (#624).
- MCP code_sync now uses canonical worktree identity and the manifest-based synchronization
  flow without reading the same checkout twice (#625).
- code_definition now renders a single-object LocationLink as well as array responses (#626).
- Call and type hierarchy tools now return errors for malformed LSP replies instead of
  presenting them as empty results (#627).
- Gateway sync rejects absolute, parent-traversal, and symlink paths that escape the
  selected workspace (#628).
- The remote mac node build now preserves cargo's failure status through its log-filtering
  pipeline (#629).
- Gateway execution state restoration now reports restored kept files in stale paths so client
  watermarks re-sync them (#646).
- Swift verification and compile checks now report structured, actionable diagnostics for
  SwiftPM dependency fetch failures and GitShellError (#645).
- Impact analysis stops traversing upwards from test nodes, bounds high fan-in dispatchers
  with a fan-in gap, and falls back to full workspace test commands when test selections
  exceed the selective threshold (#592).

### Added
- In-flight cold engine loads are bounded by a configurable semaphore (`--max-concurrent-engine-loads`
  and `PROD_CODE_MAX_CONCURRENT_ENGINE_LOADS`, defaulting to 8 on >=32 cores, 4 on >=8 cores, 2 otherwise)
  so that parallel worktree connections do not exhaust CPU and disk I/O caches and can answer initial
  queries within their response budget (#408).
- Fallback language-server workers also probe initialized idle dispatch (#600). Private
  probe IDs cannot swallow an ordinary JSON-RPC reply; three consecutive idle timeouts
  retire the owned worker. Controlled failures and a real gopls run verify replies before
  and after scheduled probes. Dedicated editor-server probing and loaded-project stress
  policy remain open.
- TypeScript safe-delete supports ordinary private ASCII-named top-level functions in
  contained ES-module projects (#594). Exact native declaration/reference evidence and
  remote TypeScript compilation precede transactional application. Exports, uncertain or
  dynamic references (including disguised direct eval), linked/generated sources and broader
  configuration graphs refuse even with force. The `preserve` module-emission mode is refused
  because a type-only export cannot prove runtime module privacy (#603). JavaScript, global
  scripts and broader TypeScript declarations remain unsupported.
- Go and generic adapters periodically probe initialized idle language servers (#590).
  Valid late replies reset the failure streak; private probe state stays bounded. Three
  consecutive idle response timeouts retire the generation, while ordinary work defers
  probing. Native Go/Python/C++/TypeScript checks observe matching replies. The Swift 5.10
  SourceKit-LSP proof also checks hover before and after two replies (#599), with bounded
  private compiler setup and exact process-group retirement.
- Go safe-delete also supports named value/pointer receiver methods (#567). Interface
  obligations, promoted methods and embeddings through nested structures or aliases refuse
  without writing, including with force. Constant-array receiver types remain supported;
  named array fields remain distinct from instantiated alias embeddings.
- Go safe-delete supports ordinary unexported, ASCII-named, non-generic top-level functions
  with bodies (#550). Exact gopls declaration/reference evidence and remote package/test
  compilation precede the write. Used functions, generated or linked sources, uncertain
  evidence and unsupported declarations refuse without changes, including with force.
  Verification uses the active build flags; imports are not removed automatically.
- Session handshakes negotiate a mutually supported protocol version (#537). Legacy peers
  remain compatible; empty or incompatible offers refuse before session creation, and clients
  reject unsupported selections before initializing a language server.
- Go signature changes can replace one unnamed primitive result on ordinary non-generic,
  non-variadic free functions (#529) and named value/pointer receiver methods (#542). The named
  parameter list must stay unchanged; complete
  direct-call evidence and remote compilation of packages and test callers are mandatory for
  preview and apply, including no-ops. Shadowed primitive names, linked package sources and
  unsupported result shapes refuse before writing, even with force. Receiver methods additionally
  require empty interface-implementation evidence and direct selector calls; method values and
  expressions refuse. Broader Go signatures
  remain open (#448).
- Generic Rust inherent implementations can now use `code_extract_trait` (#505). Ordinary
  lifetime, type and const parameters, inline bounds and `where` clauses are carried to the
  generated trait and trait implementation, while retained methods keep the original inherent
  constraints. Method-level generics, associated-type bounds, concrete `Self` returns, comments
  and external caller imports are preserved. Attributed impls, conditional methods, `Self`-dependent
  impl bounds, macro-shaped, specialized and trait impls remain explicit refusals; caller
  type-annotation migration and other languages remain open.
- Rust trait extraction handles Unicode impl-header positions without slicing through characters
  (#509). Opaque `impl Trait` return types are refused before edits, even with force, because
  extraction can change lifetime capture and break valid callers (#510). Macro arguments are
  refused for all three delimiter forms, including force (#512).
- Typed Rust builder previews through `prod-code fixture --builder` and
  `code_generate_fixture` with `builder: true` (#459, #497). Named structs, including
  ordinary lifetime, type and const parameters with inline bounds, defaults and `where`
  clauses, get setters with their actual field types and a fallible `build` requiring every field.
  Verification checks name collisions and the generated code; unavailable or malformed
  evidence cannot report success. Previews write no files, and `--no-verify` explicitly
  returns an unverified draft. `Self`-dependent bounds, macro-expanded generic syntax,
  nontrivial const expressions, factory-call rewriting, mocks and other languages remain open.
- Failure dossiers expose printed assertion operands in JSON and text (#445): Rust
  `assert_eq!`/`assert_ne!` retain left/right order, and supported Node assertion formats
  retain actual/expected roles. Each failure uses its own output; multiline values and
  raw evidence are preserved, while ambiguous or incomplete blocks get no structured values.
- Go signature changes through gopls (#448): reorder named parameters and remove provably
  unused ones from functions and methods, including grouped parameters and variadic tails.
  CLI and MCP preview or apply only fully reconciled, type-checked edits; dropped arguments
  must be literals or simple variables. `--remove-all` (MCP `params: []`) explicitly requests
  an empty list. Typed literal additions to ordinary non-generic functions (#502) and named
  value/pointer receiver methods (#513) preserve receiver and old argument evaluation and require
  remote compiler verification, including test callers and inherited build flags. Method
  values/expressions, local or imported interface obligations (#520), generic receivers and
  receiver-name capture refuse; failed or malformed required evidence cannot bypass those guards.
  Broader additions, modifier changes, generic removals, indirect calls, incomplete
  references and uncertain argument effects remain refused.
- Parameter objects in JavaScript and JSX (#428): pass a plain object and update body uses,
  imported aliases and supported calls. Unsupported call/default shapes are refused before
  writing; validation reports syntax diagnostics, not a JavaScript type-check guarantee.

### Changed
- Cold Rust analysis loads share a process-wide CPU budget, including validation engines
  (#408). Half the detected CPUs (at least one) are assigned to concurrent loads, with at most eight
  Cargo jobs per load; the permit is returned on failure as well as success. Queries and
  explicitly requested builds keep their existing behavior. The divergent-worktree benchmark
  now syncs and loads its origin first and names it as the seed for isolated copies.

### Fixed
- Python proposals now route to basedpyright before the file or its parent directory exists (#559).
  New-file routing stays inside the checkout and refuses symlink parents, `..` escapes and
  directories named with a source suffix.
- Rust impact analysis now requires a recognized source test attribute before a Rust caller is
  selected as runnable, even when the analyzer flags it as test context (#612). Helpers in test
  files and functions marked only with `#[cfg(test)]` no longer enter Cargo test filters. A
  conditional `cfg_attr` test marker whose active Cargo configuration cannot be proved triggers a
  whole-suite fallback.
- Rust `validate --compile` recovers from missing cached dependency `.rmeta` files in
  fresh worktrees: a normal workspace `cargo check` warms metadata, then the proposed
  text is checked again in the shadow. Other compiler errors still reject the proposal
  (#607).
- Shared gateway sessions bound output queues and socket writes (#598). A connected peer
  that stops reading cannot hold its session handler indefinitely; cancellation retires
  the exact writer, and other sessions continue answering.
- Shadow hypotheses use a storage-specific namespace with exclusive gateway ownership (#591).
  Startup cleanup cannot sweep another running gateway's hypotheses. Canonical aliases,
  including absent-path `..` traversal and symlinks after it, contend on one stable lock
  (#602), held until the last accepted session releases its state.
- Gateway handshake and shared-session resources retire on errors and cancellation (#583).
  Workspace and worktree ownership remains held until the canceled session's overlays are
  restored, preserving independent sessions and subsequent single-owner behavior.
- Dedicated editor sessions bound server-input and editor-output writes and teardown (#574).
  A stalled peer cannot retain its child process group; watched-file fanout retires only
  lagging sessions. Cleanup also retires descendants after their parent exits, and final
  frames still reach a reading editor within the teardown budget.
- Dedicated editor servers share the bounded LSP frame reader (#573). Duplicate or
  malformed headers, oversized or truncated frames and invalid UTF-8 end the owned
  session without forwarding corrupted text.
- Go and generic language-server adapters reject unsuccessful or malformed initialization
  replies (#572). Failure, timeout or cancellation retires the owned server generation,
  clears pending requests and capabilities, and ends generic document acceptance; ordinary
  request timeouts keep their existing behavior.
- MCP multi-file validation accepts each advertised input form in its schema (#585):
  whole-file `edits`, `diff` or `workspace_edit`. At least one remains required, and
  the existing input precedence and field types are preserved.
- Diff-validation help names the current on-disk files as the patch base (#578).
  Submit a proposed patch before applying it; validate already-written edits using
  their complete current contents.
- The prod-code source command reads checkout-local workspace files from the active
  checkout, retains gateway reads for external SDK and dependency sources, and refuses
  paths that escape the checkout. Local reads stop at 2 MiB (#619).
- Editor pre-save synchronization reads the decoded top-level JSON-RPC method (#579).
  Reordered fields and escaped method keys/values still push the checkout before save or
  watched-file notifications; nested `method` fields do not trigger an unrelated sync.
- Editor stdin frames use the shared bounded LSP reader (#577). Duplicate or malformed
  lengths, oversized headers/bodies, truncated frames and invalid UTF-8 fail with context
  instead of reaching the node as altered text or ending the session successfully.
- Rust impact analysis follows callers through ordinary helpers in test files instead of
  treating every function there as a runnable test (#560). Attributes belong to their exact
  enclosing declaration; nested helpers do not inherit an outer test marker. Ambiguous
  same-name declarations on one line force the full suite. Multiple selected Rust tests run
  with one Cargo filter per test, preserving each failure status.
- The editor's remote-file mirror preserves source, edits and documentation while translating
  LSP locations (#568). Encoded cache paths round-trip correctly, sibling directory prefixes
  stay unchanged, and workspace-edit URI keys use the same mapping as location values.
- Symbol resolution refuses missing, malformed and overflowing LSP response coordinates (#564)
  instead of defaulting or wrapping them to another source position. Matching workspace hits,
  qualified members and fallback declarations propagate the error before any refactoring write,
  including forced application.
- Delayed search embeddings carry a file-entry generation, so a same-size edit or an index
  recreation cannot attach an older vector to a current declaration (#556). Search indexing
  skips descendant file and directory symlinks, removes invalidated linked sources, and ignores
  absolute or parent-traversal invalidations (#557). Checkout-root aliases remain supported.
- Search scopes match an exact file or directory descendants, excluding sibling string prefixes
  (#555). Root and dot components are normalized consistently; absolute and parent-traversal
  scopes return an error. Lexical and dense ranking use the same scope.
- Fallback LSP requests already answered by the gateway stay out of editor sessions (#554).
  Unknown server requests still reach the editor, while notifications and client responses
  preserve their routing even when server and client request IDs collide.
- Fallback LSP writes have one deadline covering writer contention, frame bytes and flush
  (#548). Interrupted partial frames retire the owned child before queued writers resume;
  caller-side timeouts before writing leave a healthy worker usable. A failed automatic reply
  retires the backend so a server waiting for that response is not reused.
- Go and fallback configuration replies return one default setting per requested item, including
  empty lists (#553). Invalid parameters produce a contextual JSON-RPC error and preserve the
  request ID; subsequent valid requests remain usable.
- Go, generic and fallback language servers accept complete LSP header blocks, including
  Content-Type after Content-Length (#544). Header and body sizes are bounded; duplicate,
  oversized or truncated frames terminate the stream and release pending requests.
- Go and generic LSP notifications and automatic replies have complete write deadlines (#543).
  Canceled or incomplete frames retire the owned child before releasing the writer; interrupted
  document updates invalidate their generation, while a queued update that never changed state
  leaves a healthy server available. Multi-document cleanup shares one deadline.
- Fallback language servers require a valid initialization response before becoming available
  (#538). Initialization failures and timeouts remain errors, server requests cannot masquerade
  as the response, and an exited fallback makes its cached workspace eligible for reload.
- Go and generic LSP request deadlines cover waiting for the writer, writing the frame and
  receiving the response (#533). Cancellation removes pending entries; an interrupted partial
  frame retires only its owned server before another writer can use the connection.
- Late older numbered diagnostics no longer replace the current report (#536). Closing and
  reopening a document still accepts its reset version instead of keeping a previous generation.
- Build metadata consistently requires Rust 1.95, matching the locked analyzer dependencies;
  the documented minimum is checked with its own compiler and Clippy (#532).
- Native Rust queries and refactorings reject invalid UTF-16 source positions instead of
  selecting byte zero (#523). Gateway requests reject malformed or overflowing coordinates
  before conversion, including explicit assist ends and hierarchy positions (#526); navigation
  errors remain errors instead of becoming empty answers. Valid editor clamping is unchanged.
- MCP queries reject JSON-RPC responses that omit `result` instead of treating missing evidence
  as an empty answer (#525). Explicit `null` remains valid, and failures name the queried method.
- Divergent-worktree benchmarks cap each initial workspace sync and complete session setup at
  300 seconds (#519). Silent peers produce contextual errors and their connections close; the
  separate 30-second hover budget and query error accounting are unchanged.
- Rust trait extraction resolves method-body cursors to the innermost enclosing impl item
  (#514), skipping opaque `impl Trait` type syntax, raw identifiers and closed nested impls.
  Local impls inside parenthesized and array block expressions remain supported; macro and
  existing-trait implementations and positions outside every impl still refuse before writes.
- Qualified symbol lookup requires the requested owner path before using path hints (#489).
  Missing members cannot select another type or module, including methods reported as functions.
  Outline fallback preserves unindexed members; crate aliases come from parsed Cargo metadata.
  Qualified index and outline candidates must also name the current readable source; stale
  positions are refused rather than selected as the only remaining candidate.
- Python validation keeps one pyright document identity across baseline and proposal sessions
  (#466). Closing a validation restores the current disk text through a monotonic document
  change; later proposals no longer split `builtins.str` from `str`. Generic validation uses a
  separate, session-serialized server, so proposals cannot leak into ordinary or parallel
  clients, interrupted sessions are restored, and genuine diagnostics remain errors. Retained
  documents follow later disk notifications while active overlays remain owned by their client;
  bounded generations are replaced whole, and a private-server admission or startup failure is
  reported instead of falling back to the ordinary engine. Incremental UTF-16 edits are composed
  against their owner's full text before a session is restored; invalid ranges leave it unchanged.
- All MCP tools with source positions reject zero, overflowing or malformed coordinates
  before dispatch (#500). Selection endpoints must be supplied together and cannot precede
  the start; CLI assist application shares the same guard. Discovery schemas advertise bounds.
- Linux overlay shadow commands drop mount-time namespace capabilities before running (#491),
  so read-only files are refused as in ordinary execution. Overlay support now requires
  `setpriv` on the gateway PATH; the capability probe reports when it is unavailable.
- Rename refuses zero and overflowing one-based coordinates before asking the analyzer or
  writing files (#495), including forced, comment-aware and accessor renames.
- Rust refactoring planners and the native Rust engine now convert UTF-16 source columns
  consistently (#456), including supplementary characters and CRLF. Invalid planner positions
  are refused before edits; the editor bridge clamps positions to valid character and line
  boundaries. A final bare carriage return remains source text.
- Multi-file Rust validation checks candidate definitions before warning about a removed
  name (#483). Relocated declarations and names that resolve in the complete proposal no
  longer count as stale callers; unresolved uses and malformed definition evidence still warn.
- Go signature changes reject zero one-based coordinates, CRLF terminator interiors and
  UTF-16 surrogate interiors before planning or applying native edits (#486).
- Relative source paths passed to library diagnostics and validation select their engine
  relative to the supplied checkout, so nested Python files in Rust projects reach Python
  instead of failing in the Rust VFS (#488). Absolute paths retain the same behavior.
- Missing or stale diagnostic publications return an explicit unavailable error through
  diagnostics, edit validation and code actions (#471). Empty reports remain valid when
  published for the current text. Explicit versions must match exactly, so a delayed higher
  version from a closed document cannot check its reopened text. Unversioned servers retain
  arrival-order evidence only; a timestamp does not establish a document version.
- Generic LSP diagnostic pulls accept only complete `full` reports with array items and no
  error envelope (#479). Uncached `unchanged`, unknown or malformed reports no longer become
  clean results when their kind is stripped; an available publication must supply the fallback.
- Removed Rust helper names in comments and strings no longer produce stale-reference
  warnings or false notes (#484). Line, doc and nested block comments, ordinary, raw and
  byte strings, character literals and lifetime parameters are ignored, while actual remaining
  code references, qualified paths, raw identifiers and same-line uses after comments still
  warn at their UTF-16 code columns.
- The Go adapter normalizes gopls v0.23.0 complete diagnostic reports with an empty `kind`
  to `full` (#475). Strict validation can accept valid Go source and retain real type errors;
  missing/malformed items, error responses and uncached unchanged reports stay refused.
- Rust signature reconciliation accounts for ordinary block comments that the analyzer moves
  immediately after a rewritten call (#472), including nested and repeated comments. Lost,
  changed or invented comments, moved line/doc comments, unchanged calls and unrelated edits
  still prevent the write. Whitespace inside literals remains significant.
- Edit validation treats Rust `unlinked-file` reports as missing semantic evidence (#467),
  including when the same hint existed before the edit. Read-only diagnostics retain the hint.
  Missing, malformed or uncached `unchanged` diagnostic reports fail validation (#470), and
  out-of-range coordinates no longer overflow. Unsupported manifest, lockfile and documentation
  proposals are refused before querying a language server, with an explicit shadow-check path
  instead of a VFS error (#465).
- Slices report failed, malformed or unreadable dependency evidence as `INCOMPLETE` (#457),
  without claiming a reduction result is complete. Depth, byte and declaration bounds are
  labelled `BOUNDED`; invalid file URIs and positions outside the actual UTF-16 source
  are refused or named as gaps, including CRLF and split-surrogate positions.
- Custom refactoring planners refuse failed, malformed, unreadable or unmatched required
  references before writing (#446), including with `force` and compiler verification.
  Signature changes reconcile individual occurrences, so a rewritten call cannot hide an
  unhandled function value on the same line. Raw `r#use` identifiers are not imports, and
  spaces inside argument string literals remain significant. Missing indexed files stop
  parameter-object rewrites in every supported language.
- Fixture generation requires one distinct type location (#461). Declarations in `lib.rs`,
  `main.rs` and `mod.rs` are no longer discarded by filename. Ambiguous names list every
  location and require a declaring-file hint, including separately indexed re-exports.
- Rust parameter objects preserve destruction order (#441), including types named like
  primitives. Analyzer evidence distinguishes builtin scalars from shadowing types with `Drop`.
  Interleaved owned values and async or older-edition capture cases are refused when bundling
  would change destruction order.
- Parameter-object refactoring preserves argument evaluation order and refuses uncertain
  reordering (#436), including JavaScript global getters and Swift/C++ conversions. JavaScript
  and TypeScript selected constant defaults move into object literals; computed defaults and
  potentially undefined runtime values are refused before rewriting.
- Parameter-object rewrites preserve keyword/field evaluation order and refuse calls whose
  argument reordering cannot be shown safe (#436). Failed reference requests and stale
  JavaScript positions stop the rewrite; omitted fields use `void 0`, and `__proto__` becomes
  an own computed property.
- Rust signature changes refuse reordered or removed argument effects and changes to
  parameter destruction order, including user `Deref` calls and types that shadow built-ins
  (#442). Missing, malformed or unreadable references stop planning, even with `force`.
  Confirmed scalar arguments can still be reordered; unproven reference coercions are refused.
- Path translation rewrites URI and path fields without changing source, edit text or
  documentation (#438). File URIs encode spaces, Unicode, `#` and `%` consistently,
  compare complete path components, and decode a URI exactly once.
- New language engines reserve memory before loading (#433), including on a node already
  holding the checkout. Concurrent loads count against an 85% host-memory limit; idle
  engines may be reclaimed, while existing sessions remain available. A canceled
  handshake keeps its load and reservation alive, and a failed load answers its waiters.
  An invalidated load cannot publish over a newer load of the same workspace.
  The reservation defaults to 4 GiB for Rust and 1 GiB for other language servers and
  can be adjusted with `--engine-reserve-mib`. This bounds admission, not later engine growth.
- Shadow hypotheses reject control characters, duplicate or nested paths and symlinked
  parents before staging (#440). In-place runs take turns per workspace across requests,
  restore failed writes, preserve file modes and report restoration failures. Rollback
  checks symlink containment again before restoring files or removing created directories.
  Internal overlay variables cannot be overridden by request environment values.
- Overlay shadow builds keep sccache compilation in the hypothesis namespace (#426).
  Cold compiler-validation coverage also verifies missing dependency metadata (#482), without
  a preliminary build that would hide the failure.
  They enable client-side compilation and refuse settings that disable it or send work
  to a distributed compiler. Config checks follow the proposed files, deletions and
  symlink aliases, so a hypothesis cannot silently be checked against the base source.
- Issue reports accept the documented `roadmap` label (#447). Unsupported labels are
  described as a reporting policy restriction instead of falsely claiming they do not
  exist in the repository.
- Multi-file refactors apply LSP edits in order, interpret columns as UTF-16 and roll back
  earlier writes and resource operations when a later step fails (#425). Paths are checked
  again after directory moves, and deleted files do not survive as hidden backup files.
  Rust module refactors send text changes before file moves, matching LSP ordering.
- Impact-based CI falls back to the full suite when deleted files, unreadable diffs,
  unattributed changes, failed hierarchy queries or a depth limit leave the selection
  incomplete (#434). It includes directly changed tests, all call-hierarchy items and
  Git-quoted filenames. Dead-code scans mark failed or malformed analyzer answers as
  unverified; pruning keeps those symbols and files (#435).
- MCP sessions serialize queries per workspace instead of holding one process-wide lock
  across network operations (#430). Opening a session has a three-minute deadline; pooled
  queries include queueing, sync and loading in their total budget (three minutes normally,
  five for diagnostics, fifteen for structural replacement). Timed-out or cancelled queries
  discard their connection, and a timeout is not immediately replayed.
- The coverage gate rejects empty or malformed reports, invalid percentages and requested
  files missing from the measurements (#427). A source file is reported as having no code
  only when the report explicitly records zero regions. Files outside the repository,
  including those reached through a symlink, are excluded.

## v0.3.18 — 2026-09-26

### Changed
- **The dev and test profiles keep line tables only, in the objects** (#421). With
  `split-debuginfo = "unpacked"` and incremental builds, rustc wrote a `.dwo` file per codegen
  unit into `target/debug/deps`, and nothing ever removed the old ones: 565,488 files (26.2 GB)
  within a week on the node that holds this repository's copy. `debug = "line-tables-only"`
  without split debug info keeps file and line in backtraces and leaves nothing behind.
  Measured on a Linux x86_64 build node, a cold `cargo test --workspace --no-run` took 50.7 s
  against 49.1 s. Three incremental rebuilds after a one-line change to `tools.rs` took 4.04,
  3.92 and 3.98 s against 4.57, 3.78 and 3.86 s. The target was 6.3 GB either way, with 0
  `.dwo` files against 20,139 after those four builds.

### Fixed
- **Seeding a worktree copy leaves a fifth of the disk free** (#419). A seed copied the main
  copy's whole `target/debug` whenever twice its size was free. For this repository on a build
  node that was 48 GB in 571,304 files per worktree: three short-lived worktrees took 140 GB in
  an hour and left the disk at 16% free. A seed now copies the build cache, `node_modules` and
  virtual environments only when at least 20% of the filesystem stays free afterwards. That is
  above the janitor's 15% prune line, so seeding never forces pruning or the disk pressure
  placement avoids. A skipped copy is logged with its size and the free space, so a slow first
  build in a worktree can be told apart from a bug.

## v0.3.17 — 2026-09-26

### Added
- **A new worktree's copy takes the main copy's `node_modules`** (#412). Before, a copy seeded
  from the main checkout's copy got the sources and Rust's `target/debug` (#278), but no
  `node_modules`. Every import from a dependency then resolved to nothing in the new worktree:
  hover said `any` and diagnostics reported TS2307 until something installed the packages again.
  The seed now also copies the `node_modules` trees at the root and in workspace packages with
  `cp -a`, so the symlinks of `.bin` and of pnpm's layout stay symlinks. It happens only when the
  disk has room for twice their size. The trees are the worktree's own afterwards.

### Fixed
- **`prod-code check` works in a new worktree of a CMake project** (#416). The worktree's seeded
  copy took the main copy's `build/`. `cmake` then refused the old `CMakeCache.txt`, and the
  `compile_commands.json` there, which names the main copy's sources, kept the configure step
  that would have written a correct one from running, so clangd in the worktree took its flags
  and headers from the other copy. A seeded copy now takes the sources and no per-node cache:
  `build/`, `.build`, `.cache` (clangd's index, keyed by the old paths), `dist`,
  `__pycache__`, `DerivedData`, and the rest. The caches that are safe to move keep being seeded
  on purpose: `target/debug`, `node_modules`, virtual environments.
- **Seeding a worktree copy no longer doubles a Python virtual environment** (#414). The copy
  resolved every symlink. `.venv/lib64 -> lib` became a second full copy of site-packages,
  `bin/python` a copy of the interpreter, and the venv's scripts still started the main copy's
  interpreter. A virtual environment (a directory with `pyvenv.cfg`) is now seeded like
  `node_modules` (#412), with `cp -a`, and the scripts in its `bin` that name the main copy's venv,
  console-script shebangs and `activate`, are rewritten to name the new one. A directory symlink
  in the tree is copied as a symlink, where it used to be walked (and one pointing at a parent
  recursed until the path was too long).

## v0.3.16 — 2026-09-26

### Added
- **`prod-code divergent-bench --worktrees N`** (#406). N is a multiple of 4, and the bench forks
  every mutation kind (untouched, signature change, manifest change, untracked file) N/4 times.
  Each copy gets its own directory, branch and server workspace (`<repo>--wt-signature-2`, ...),
  and the workers spread over all of them. It had always been four worktrees, so the ROADMAP 2.2
  target of 15 concurrent agent worktrees could not be measured.
- **An optional cluster token that every connection must open with** (#402). With
  `PROD_CODE_AUTH_TOKEN` set, or `PROD_CODE_AUTH_TOKEN_FILE` naming a file that holds it,
  clients and peer gateways send the token as the first frame of every connection. A gateway
  started with one closes a connection that does not open with it before serving anything,
  even the status, and tells it which variables to set. The comparison does not stop at the
  first differing byte. A gateway without a token ignores one that is sent, so clients can get it
  first. It is off by default: a cluster without a token takes every connection, as before. The
  gateway removes both variables from the environment of the commands it runs, so a build or a
  test on a node never sees the secret. The token never appears in a log line or in a message's
  `Debug`.
- **Go is linted with golangci-lint** (#400). `lint` and `code_lint` on a Go project run
  `golangci-lint run` when the node has it, which reads the project's `.golangci.yml`. Otherwise
  they run `go vet` as before, with a line on stderr saying golangci-lint is missing. `--path`
  still narrows the run to one package tree. `lint --fix` runs `golangci-lint run --fix` when the
  project has a golangci config. The findings, `file.go:line:col: message (linter)`, are parsed
  like go vet's; golangci-lint's quoted source lines and summary are skipped.
- **`prod-code status --json` and `prod-code cluster --json`** (#398). `status --json` prints the
  gateway's status as one JSON object, with the fields as the gateway sent them: engines, sessions,
  running commands, host memory and disk. It adds `remote`, `rtt_ms`, `healthy` and `pressure`.
  `cluster --json` prints the gossip view, each configured node's snapshot (or `up: false` and
  the error it gave), and the checkout's `home` and `placed_on` nodes. A script no longer parses
  aligned text to learn whether a node is running a build.
- **Placement keeps new workspaces off a node short of memory or disk** (#396). A gateway's status
  and gossip carry what its host has left: memory available and total (`MemAvailable` on Linux,
  the kernel's free percentage on macOS) and the free share of the workspaces filesystem. A node
  past 85% of its memory or under 10% of its disk gets no new workspace while a capable node with
  room is alive, gives up an idle one it holds, and keeps one in use; when every node is short,
  the quietest still takes it and the placement reason says so. The client's own choice, made
  when no gateway answers a placement, passes such a node over the same way. The janitor logs
  when the node becomes short and when it has room again, and while memory is short it unloads
  engines idle for 5 minutes, even when `--idle-evict-secs` is off. `prod-code status`,
  `prod-code cluster` and `code_status` show the figures and `SHORT (why)` in place of
  `HEALTHY`.

### Fixed
- **A C project built with Make and an XcodeGen project are detected** (#404). A `Makefile` next to
  C or C++ sources (at the root or in `src/`) now marks a C/C++ project for the client and the
  gateway alike. It ranks below every other manifest, because Go, Python and JavaScript
  repositories keep a Makefile of tasks too, and a Makefile with no C sources is not C. `check`
  and `test` then run `make` / `make test` instead of answering "no project manifest". A
  `project.yml` with top-level `targets:` (an XcodeGen spec) marks a Swift project, placed on a
  macOS node, like `Package.swift`.
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
  one module tree into several crates behind feature flags need `features = "all"`, otherwise
  those modules resolve to nothing.
- Sync ships `rustc-wrapper` scripts and `*.sh`, and the gateway keeps the executable bit, so
  `cargo metadata` works on a workspace whose `.cargo/config.toml` sets `build.rustc-wrapper`.
  Verified on a private 1,200-file Rust repository: 109 implementations of its central trait,
  235 references, callers with call sites, where before only syntax-level queries answered.

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
  have and asks only for missing files. A fresh worktree of a 1,200-file Rust repository: 0.4 s
  instead of ~7 s.

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
  Isolated mode passes on a private Rust repository and a private Go repository with zero
  errors.

### Known limitations
- Shared (coalesced) workspaces are diagnostic only; production isolates worktrees.
- A one-shot CLI query costs ~80 ms, of which ~70 ms are two `git` subprocesses on the
  client; long-lived agents avoid this through the MCP server's change watcher.
- Cluster features (Phase 5), C/C++, TypeScript, Python and Swift engines (Phase 3.4–3.7) are
  not implemented yet; see ROADMAP.md.
