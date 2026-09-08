# Keyed repeated field fixtures

Cross-port conformance fixtures for **keyed repeated fields**
([issue #116](https://github.com/trendvidia/protowire/issues/116);
IETF draft `-01` §3.13 "Keyed Repeated Fields"; `(pxf.key) = 1316` in
[`proto/pxf/annotations.proto`](../../proto/pxf/annotations.proto)).

Every port implementing the keyed grammar + semantics (reference:
[protowire-go#50](https://github.com/trendvidia/protowire-go/issues/50);
cpp, rust, java, typescript, csharp, swift, dart tracked from #116)
must satisfy these fixtures. Inheriting ports (kotlin via java, python
via cpp) verify against the same files. As with
[`testdata/dataset/`](../dataset/), the conformance harness wiring is
deferred until the reference implementation lands; each file's leading
comment states the expected behavior.

All fixtures bind against [`keyed.proto`](keyed.proto) (package
`keyed.v1`); each file's `@type` directive names its message.

## Accept fixtures

| File | Asserts |
|---|---|
| `roundtrip-keyed.pxf` | Canonical keyed-block form round-trips: decode → encode reproduces the body. |
| `roundtrip-quoted.pxf` | Non-identifier-safe keys round-trip in quoted entry-name form (quoting preserved). |
| `anonymous-equivalence.pxf` | Anonymous list form decodes to exactly the same message as `roundtrip-keyed.pxf`; fmt canonicalizes it to the keyed form. |
| `redundant-key-ok.pxf` | An agreeing explicit key-field assignment inside a named entry is legal; fmt drops it. |
| `anonymous-duplicate-ok.pxf` | Duplicate keys decode fine in anonymous form and MUST stay anonymous on encode/fmt. |

## fmt canonicalization pairs

Each `<name>.pxf` input must format to exactly `<name>.expected.pxf`.
The pairs are deliberately comment-free (apart from `@type`) so the
byte-level expectation doesn't pin comment-placement behavior. The
expected files use the reference formatter's style (2-space indent,
expanded blocks); structure — quoting, key placement, form selection —
is the normative content.

| Pair | Asserts |
|---|---|
| `fmt-unquote` | Quoted identifier-safe entry name → unquoted; redundant agreeing key assignment dropped; already-canonical entries untouched. |
| `fmt-anonymous-to-keyed` | Eligible anonymous form → keyed form; per-key quoting decision (`"us-east-1"` stays quoted, `primary` emitted bare). |

The expected files are also fmt fixed points: formatting an
`.expected.pxf` reproduces it byte-for-byte.

## Reject fixtures

| File | Expected error |
|---|---|
| `err-duplicate-key.pxf` | Duplicate entry names in one keyed block (decode error; diagnostic in tolerant mode). |
| `err-duplicate-key-spelling.pxf` | `greeting` vs `"greeting"` — duplicate detection compares unquoted values. |
| `err-key-conflict.pxf` | Explicit key-field assignment disagrees with the entry name. |
| `err-empty-key.pxf` | `""` as a quoted entry name — the empty string is never a valid key. |
| `err-empty-key-anonymous.pxf` | Explicit empty-string key-field value in anonymous form. |
| `err-quoted-name-unkeyed.pxf` | Quoted entry name outside a keyed repeated field's block — parses, but the schema layer rejects it. |

## Adding fixtures

Extend `keyed.proto` additively only (new fields / messages); renaming
or renumbering breaks recorded expectations across every port. Keep new
fixtures single-purpose with the expected verdict in the leading
comment, and update the tables above.
