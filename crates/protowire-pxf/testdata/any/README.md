# Any block fixtures

Cross-port conformance fixtures for the **type entry of a
`google.protobuf.Any` block** ([issue #354](https://github.com/trendvidia/protowire/issues/354);
IETF draft `-01` "Any Blocks"): `@type = "<type-url>"` as the first entry
of a block bound to `google.protobuf.Any`, after which the block is a body
of the type the URL names.

Every fixture binds against [`any.proto`](any.proto) (package `any.v1`,
row type `Holder`) with a type resolver that maps a URL to the message
named after its last `/`. Each file's leading comment states the expected
behavior, and `cmd/pxf/any_test.go` runs them through the reference
implementation: through `Parse`, which has no schema, and through a
decoder bound to `any.v1.Holder`.

## Accept fixtures

| File | Asserts |
|---|---|
| `block.pxf` | `payload { @type = ... }` decodes; the body packs as `any.v1.Leaf`. |
| `assignment.pxf` | The `payload = { @type = ... }` spelling decodes the same way. |
| `url-verbatim.pxf` | `type_url` holds the URL as written, `type.googleapis.com/` prefix included. |
| `list-element.pxf` | Each element of a repeated Any may open with a type entry. |
| `map-value.pxf` | A map value of type Any may open with a type entry. |
| `plain-fields.pxf` | Without a type entry, an Any block is its own fields, `type_url` and `value`. |

## Reject fixtures

| File | Parse | Decoder |
|---|---|---|
| `err-not-first.pxf` | rejects: the type entry is not first | rejects (the reference fails at `tag`, a field `Any` does not have) |
| `err-twice.pxf` | rejects: a second type entry is not first | rejects |
| `err-not-any.pxf` | accepts: a parser cannot tell which blocks are Any | rejects: `leaf` is not an Any |
| `err-unresolved.pxf` | accepts | rejects: no type `any.v1.Nope` |
| `err-unquoted-url.pxf` | rejects: the URL must be a string | rejects |
