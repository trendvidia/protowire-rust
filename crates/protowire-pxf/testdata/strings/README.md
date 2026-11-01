# Simple-string newline fixtures

Cross-port conformance fixtures for the rule that **a simple string holds
no raw line feed** ([issue #350](https://github.com/trendvidia/protowire/issues/350);
IETF draft `-01` § String Literals: "Within a simple-string, U+000A (LF)
MUST NOT appear unescaped ... Decoders MUST reject a simple-string
containing a literal LF").

Every fixture binds against [`strings.proto`](strings.proto) (package
`strings.v1`, `Holder` with string fields `s` and `t`). Each file's leading
comment states the expected behavior, and `cmd/pxf/strings_test.go` runs
them through the reference implementation three ways: `Parse` (no
schema), a decoder bound to `strings.v1.Holder`, and `pxf validate`. The
same rule inside a `@dataset` cell is pinned by
[`../dataset/string-newline-in-cell.pxf`](../dataset/string-newline-in-cell.pxf).

| File | Verdict | Asserts |
|---|---|---|
| `err-raw-newline.pxf` | reject | A raw LF inside `"..."` is an error at the string. |
| `escaped-newline.pxf` | accept | The escape `\n` is how a simple string holds a line break: `s` = "x", LF, "y". |
| `triple-quoted.pxf` | accept | A triple-quoted string may span lines; after the leading LF and the common indent are stripped, `s` = "x", LF, "y". |
| `newline-after-string.pxf` | accept | A newline after a closed string ends the entry: `s` = "x", `t` = "y". |
