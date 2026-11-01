# `@dataset` adversarial fixtures

Reference fixtures for the `@dataset` directive (draft §3.4.4). Each
file's leading comment states the violation and the expected
behavior (accept or reject) of a decoder that parses the document and
binds each row to its type. Most verdicts are decided by the parse;
`block-on-scalar` is decided by binding, so a harness that only parses
cannot meet it.

The PXF parser itself lives in the downstream port repos
(`protowire-go`, `protowire-rust`, …); these fixtures are the
cross-port contract every port's `@dataset` implementation must
satisfy. `cmd/pxf`'s `dataset_test.go` holds the reference
implementation to every fixture three ways: through `pxf validate`,
which parses the document and binds each row to its type; through the
streaming `DatasetReader`, whose rows must match the materializing
parse; and, for accept fixtures, through `pxf query`. The test's verdict
table must list every file here. The other ports are held to the
fixtures through their own issues; no cross-port harness runs them
yet.

All fixtures bind against `test.v1.AllTypes` in `testdata/test.proto`
unless otherwise noted. The `dotted-*` fixtures bind against
`dataset.v1.Event` in [`paths.proto`](paths.proto), which imports
`pxf/annotations.proto` and `pxf/bignum.proto` from `proto/`. The
exception is `dotted-column`, which binds against `test.v1.AllTypes`,
as `duplicate-column` does. Each fixture's `@dataset` header names its
row type.

The `truncated-row-*`, `junk-between-rows` and `rows-end-at-directive`
fixtures pin the "Appending" rule (issue
[#336](https://github.com/trendvidia/protowire/issues/336)). The row
sequence ends only at end of input or at the next directive. A
streaming reader must reject the first three, as the materializing
parser does, rather than end the sequence early.

`list-cell`, `block-cell`, `nested-cells`, `cell-framing` (accept) and
`block-on-scalar` (reject) pin value cells (issue
[#341](https://github.com/trendvidia/protowire/issues/341), v1.13). A
cell holds any value, and a non-empty cell in column `c` binds as
`c = <value>` would in a body. `list-cell` and `block-cell` were reject
fixtures before v1.13.

`string-newline-in-cell` (reject) carries the string rule into a cell
(issue [#350](https://github.com/trendvidia/protowire/issues/350)): a raw
LF inside a simple string is an error in a cell as it is anywhere, for
the materializing parser and the streaming reader alike. Its general
fixtures are in [`../strings/`](../strings/).

`null-on-scalar-cell` (reject) carries the null rule into a cell (issue
[#339](https://github.com/trendvidia/protowire/issues/339)): a cell
applies the singular-field null rules, so `null` in a plain `int32`
column is an error. `null-on-optional-cell` (accept) is the other side:
`null` in an `optional int32` column leaves the field unset and
present-but-null. Its general fixtures are in [`../nulls/`](../nulls/).

The `dotted-*` fixtures and `duplicate-column` pin column paths (issue
[#342](https://github.com/trendvidia/protowire/issues/342), v1.13). A
column may be a dotted path into a sub-message, and a row binds as the
body that groups its non-empty cells by path prefix, one block per
prefix.

| Fixture | Verdict | Rule |
|---|---|---|
| `dotted-column` | accept | a path binds into its sub-message (was reject before v1.13) |
| `dotted-grouping` | accept | one block per prefix; all-empty cells leave the sub-message absent |
| `dotted-null-leaf` | accept | `null` applies to the leaf; the parent is present |
| `dotted-oneof` | accept | columns may reach different oneof members, and two may fill one |
| `dotted-required` | accept | `(pxf.required)` / `(pxf.default)` apply under a present parent only |
| `dotted-with-value-cells` | accept | dotted columns and value cells together |
| `dotted-oneof-two-members` | reject | a row fills two oneof members |
| `dotted-required-missing` | reject | a present parent is missing its required field |
| `dotted-through-repeated` | reject (header) | a path through a repeated field |
| `dotted-through-map` | reject (header) | a path through a map field |
| `dotted-into-timestamp` | reject (header) | a path into a type written as a literal |
| `dotted-into-wrapper` | reject (header) | the same, for a wrapper |
| `dotted-into-duration` | reject (header) | the same, for a Duration |
| `dotted-into-bignum` | reject (header) | the same, for a `pxf.BigInt` |
| `dotted-through-scalar` | reject (header) | a path through a scalar field |
| `dotted-unknown-segment` | reject (header) | a segment names no field |
| `dotted-bad-header-empty-row` | reject (header) | a header rule holds for a row of empty cells |
| `dotted-bad-header-no-rows` | accept | the same header with no rows: the rules are checked per row (#349) |
| `dotted-into-secret` | accept | a path may go into a `pxf.Secret`, whose block form is written (#349) |
| `dotted-into-any` | accept | a path into an `Any` names its own fields, `type_url` and `value` (#349) |
| `dotted-overlap` | reject (header) | a field and a path inside it are both columns |
| `dotted-overlap-deep` | reject (header) | the same, below the top level |
| `dotted-empty-segment` | reject (header) | a path with an empty segment (`a..b`) |
| `dotted-trailing-dot` | reject (header) | a path ending in an empty segment (`a.`) |
| `duplicate-column` | reject (header) | a column named twice, plain names included |

`dotted-overlap`, `dotted-overlap-deep`, `dotted-empty-segment`,
`dotted-trailing-dot` and `duplicate-column` are decided by the header
alone, so a parser that does not bind rows must reject them too. The
other header rejections need the row type, and a decoder checks them
when it binds a row. So a table with no rows passes them, as
`dotted-bad-header-no-rows` pins.
