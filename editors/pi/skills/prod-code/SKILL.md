---
name: prod-code
description: Navigate source identifiers, trace calls and inspect analyzer diagnostics in a prod-code checkout.
---
Use `code_definition`, `code_references`, `code_callers`, and `code_hover` instead of grep/rg for identifiers. Supply `symbol` (e.g. `Type::method`) or `file`, `line`, and `col` (both 1-based). Use `code_symbols` to discover names; use a position to disambiguate a match.

Use `code_outline` for file structure and `code_diagnostics` for analyzer errors and warnings. These operate on the whole file, even when given a position or a symbol. `code_symbols` given a file lists its declarations. All three also accept `file` alone.

After `edit` or `write` on a source file, inspect the appended diagnostics before continuing; other files get none. “diagnostics unavailable” is not a clean bill of health; check `prod-code status` if the file should be supported.
