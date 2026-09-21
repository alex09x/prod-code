# polyglot-order

One schema field, four languages, on purpose.

`order_id` is declared in `schema/order.proto` and in `db/schema.sql`, and every language that
reads it spells it its own way:

| where | spelling |
|---|---|
| `schema/order.proto`, `db/schema.sql` | `order_id` |
| `core/` (Rust) | `order_id`, and `order_id` inside a SQL string |
| `backend/` (Go) | `OrderID`, with a `json:"order_id"` tag and `order_id` in a query string |
| `frontend/` (TypeScript) | `orderId` |

That is the test bed for `code_schema_rename`: no analyzer knows that the Go field and the
TypeScript property are the same thing, so renaming the field has to be both a semantic rename
in each sub-project and a text edit in the places no analyzer owns.

Each directory is its own project (`go.mod`, `Cargo.toml`, `tsconfig.json`) with no manifest at
the root, which is what makes the gateway treat them as three separate workspaces.

```sh
cd fixtures/polyglot-order
prod-code schema-rename order_id --to trade_id          # report only
prod-code schema-rename order_id --to trade_id --apply  # write it
git checkout -- .                                       # put it back
```
