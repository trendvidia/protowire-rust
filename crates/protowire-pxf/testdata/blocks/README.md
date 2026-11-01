# Block-entry separator fixtures

Cross-port conformance fixtures for the rule that **the entries of a block
are separated by whitespace and nothing else**
([issue #312](https://github.com/trendvidia/protowire/issues/312); IETF draft
`-01` § Blocks and Map Tails: "A "," or ";" between two entries of a block
is not whitespace, and a decoder MUST reject it"). The grammar's
`block-tail = "{" [ any-type-entry ] *entry "}"` has no separator token;
the comma separates list elements only.

Every fixture binds against [`blocks.proto`](blocks.proto) (package
`blocks.v1`, `Doc` with a message field `address`, a map `headers` and a
repeated message `children`). Each file's leading comment states the
expected behavior, and `cmd/pxf/blocks_test.go` runs them through the
reference implementation three ways: `Parse` (no schema), a decoder bound
to `blocks.v1.Doc`, and `pxf validate`.

| File | Verdict | Asserts |
|---|---|---|
| `err-comma-in-block.pxf` | reject | A `,` between the entries of a message block. |
| `err-semicolon-in-block.pxf` | reject | A `;` between the entries of a message block. |
| `err-comma-in-map.pxf` | reject | A `,` between the entries of a map block. |
| `err-comma-in-list-element.pxf` | reject | A `,` between the entries of a block that is a list element. |
| `space-in-block.pxf` | accept | Entries on one line, separated by spaces: `address.city` = "Berlin", `address.zip` = "10115". |
| `newline-in-block.pxf` | accept | Entries on their own lines, in the bare-block form: the same values. |
| `comma-between-elements.pxf` | accept | Commas between list elements whose blocks use spaces: `children` ids "greeting", "title". |
