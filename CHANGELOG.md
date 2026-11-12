# Changelog

All notable changes to `protowire-rust` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The version number is kept aligned with the rest of the `protowire-*`
stack — releases bump in lockstep across language ports when the wire
format changes.

## [Unreleased]

### Added

- **`protowire-pb` binds `pxf.BigInt`, `pxf.Decimal` and `pxf.BigFloat` fields** ([#142](https://github.com/trendvidia/protowire-rust/issues/142), decided on [#89](https://github.com/trendvidia/protowire-rust/issues/89)). A struct can now hold a big number and write the bytes protowire-go writes for a `*big.Int`, `*big.Rat` or `*big.Float`. The new `protowire_pb::big` module has byte-backed `BigInt`, `Decimal` and `BigFloat` types, following protowire-cpp's `pb_big.h`.
  - **Writing:** a Decimal goes out at its smallest scale >= 0 (`1.00` as `1`). A BigFloat goes out as a prec-bit mantissa, rounded half to even. A zero singular field is left out (`write_big_int`, `write_decimal`, `write_big_float`), while a list element or map value is always written, a null one as the empty message.
  - **Refusals:** a value a reader would refuse is refused on write too, with protowire-go's text.
  - **Reading:** `Decimal.scale`, a BigFloat's prec and its binary exponent are checked under the reader's `Limits`, which gain `max_numeric_literal_digits` and `max_big_float_exponent`.
  - **Tests:** the struct-binding table's 15 big-number rows now have twins, so all 23 rows match Go's bytes. `check-decode` reads `adversarial.v1.BigNumHolder` through these types, so the spec's corpus now holds the library to the bounds, not the harness.
  - **Breaking:** `Limits` has two more fields, `Error` has a `BigNumber` variant, and `Message` has a `finish` method (it has a default). `Writer::refuse` lets an impl refuse a value it cannot write.
- **`check-decode --format pb-text`** ([#113](https://github.com/trendvidia/protowire-rust/issues/113), protowire#503). It reads PB bytes with the stock runtime and writes the message's PXF on stdout, for the spec's text gate (`cross_text_check.sh`), which compares the text byte for byte with what protowire-go writes.
- **`check-decode --format pxf-pb`** ([#89](https://github.com/trendvidia/protowire-rust/issues/89), protowire#509). It decodes a PXF document with `unmarshal_full` and writes the message's PB bytes on stdout, map entries in key order, for the spec's write gate (`cross_write_check.sh`), which compares them with protowire-go's.
- **A golden test of the spec's struct-binding table** ([#89](https://github.com/trendvidia/protowire-rust/issues/89), protowire#509). `protowire-pb`'s `tests/values_table.rs` writes a `Message` twin of each row of `testdata/values/values.tsv` that holds no big number and compares the bytes; CI checks the vendored table against the spec's. The 15 rows holding a `*big.Int`, `*big.Rat` or `*big.Float` have no twin: `protowire-pb` has no binding for a big number.

### Changed

- **`pxf::marshal` and `pb::marshal` refuse a message nested past `MaxNestingDepth`** ([#86](https://github.com/trendvidia/protowire-rust/issues/86), protowire#502, draft -01 § Recursion). They wrote a message of any depth, and a reader at the default limit then refused the document. Depth is counted as each decoder counts it: in PXF every `{` or `[` is one descent, in PB every submessage or map entry. `MarshalOptions::max_nesting_depth` (both crates) raises the limit for a writer paired with a raised reader. An Any whose body would be too deep is written as `type_url` and `value` instead of being refused. **Breaking:** `pb::marshal` now returns `Result<Vec<u8>>`; `pb::marshal_with` takes the options, and `Writer::finish_checked` reports a refused value for a hand-driven `Writer`.

- **A named keyed entry can't undo its entry name** ([#82](https://github.com/trendvidia/protowire-rust/issues/82), protowire#480). The entry name binds the key field, so inside a named entry `null` on the key field, and any binding of another member of the key field's oneof, are now decode errors. The sibling case used to replace the key without a word. The anonymous form and an agreeing `name = "alpha"` stay legal.

- **`(pxf.default)` and `(pxf.required)` apply in every message body** ([#80](https://github.com/trendvidia/protowire-rust/issues/80), protowire-go#161, protowire#352). `unmarshal_full` used to stop at repeated and map fields, so annotations inside a list element, keyed entry or map value did nothing. They apply there now, at any depth, and in `@dataset` cells through `bind_row`. A document with a required field missing from an element, which used to decode, is rejected with the element's path (`required field "kr[0].name" is absent`), and an unset field in an element takes its default.

### Fixed

- **Iterating a `DatasetReader` ends after a row error** ([#132](https://github.com/trendvidia/protowire-rust/issues/132)). The iterator forwarded to `next_row`, which returns a row error again on every call, as protowire-go's `Next` does. So `for row in reader` yielded the same error forever, and a loop that logged it and carried on never ended. The iterator now yields the error once and ends. `next_row` is unchanged, and its doc no longer says it returns `None` after an error.
- **A wrong token for a message field gets protowire-go's error** ([#147](https://github.com/trendvidia/protowire-rust/issues/147)). `ts_field = 5` used to say only `expected '{' for message field "ts_field"`. It now names the forms the field takes and the token it got, word for word as protowire-go: `expected a timestamp literal or '{' for message field "ts_field", got integer ("5")`.
  - **Forms named:** a duration literal for a Duration, an integer literal for a BigInt, a numeric literal for a Decimal or BigFloat, a string literal for a Secret, and `'{'` alone otherwise.
  - **List elements and map values:** these say what they got too.
  - **Directive token:** now named `@directive` in errors, as in Go, not `@<directive>`.
  - **Tests:** `testdata/conformance/message-forms-oracle.tsv` holds 27 such errors from protowire-go v1.17.0.
- **A timestamp literal is read as protowire-go reads it** ([#118](https://github.com/trendvidia/protowire-rust/issues/118)). It ends at `)`, as protowire-go's lexer ends one, so a timestamp can be the last cell of a `@dataset` row. `(1, 2026-01-01T00:00:00Z)` used to fail with `invalid timestamp: 2026-01-01T00:00:00Z)`. Which literals are valid, and the value each reads as, now follow Go's `time.Parse` for RFC 3339, which protowire-go uses.
  - **Now refused:** lowercase `t` or `z`, and second 60.
  - **Now accepted:** a one-digit hour (`T3:04:05Z`), and zone offsets up to `+24:00`, `+24:01` and `+23:60`.
  - **Unchanged:** a fraction longer than nine digits is cut to nanoseconds, as before.
  - **Tests:** `testdata/conformance/timestamp-oracle.tsv` records what protowire-go v1.17.0 did with each of 66 literals, and the conformance test holds the port to its verdict, error text and bytes.
- **`pxf::marshal` writes an unset annotated field so that it reads back** ([#116](https://github.com/trendvidia/protowire-rust/issues/116), protowire-go#259). It left out every unset field, so a reader applied the `(pxf.default)` (a zero `n` with default `5` read back as `5`) or refused the document for a missing `(pxf.required)` field. Now a field without presence is written as its zero value, `[]` or `{}`, and a message-typed field with a default is written `null`, in every message body, as protowire-go writes them. An unset `optional` scalar with a default stays out and reads back as its default, as the reference chose (protowire-go#267). An unset `(pxf.required)` field with presence has no text a reader accepts, so `marshal` refuses the message (`pxf: unsetann.v1.ReqOpt.o is unset, and the field is (pxf.required)`), unless `emit_defaults` is set; an Any whose body would hold one is written as `type_url` and `value`. **Behaviour change:** text from `marshal` gains these lines, and a message with such a required field is now an error.
- **`pxf::marshal` writes fields in the order the schema declares them** ([#115](https://github.com/trendvidia/protowire-rust/issues/115), the spec's text case `Annotated.set`). It wrote them in field-number order, so a field declared after a higher-numbered one moved: a oneof numbered past the fields that follow it came out last. protowire-go writes declaration order; now so does this port. PB bytes stay in number order.
- **`pxf::marshal` writes a float or double as protowire-go does** ([#114](https://github.com/trendvidia/protowire-rust/issues/114), protowire#503). It used Rust's `{}`, which never writes an exponent, so `1e21` came out as `1000000000000000000000` and `1.5e-7` as `0.00000015`. It now follows Go's `strconv.FormatFloat(v, 'g', -1, bits)`: the same shortest digits, in exponent form when the decimal exponent is below -4 or at least 6 (`1e+21`, `1.5e-07`, `1.234567e+06`), plainly otherwise (`100000`, `0.0001`). The text reads back to the same value either way; the spec's text gate compares the bytes.
- **`pxf::marshal` writes a float or double negative zero** ([#114](https://github.com/trendvidia/protowire-rust/issues/114), protowire#503). prost-reflect reports a proto3 `-0.0` unset, because it compares with the default by `==`, so `f = -0` was left out and read back as `+0`. protobuf-go counts it as set, and protowire-go writes `f = -0`; now so does this port. The binary side, where prost-reflect's encoder drops it, stays open as #98.
- **`pxf::format` keeps every directive** ([#105](https://github.com/trendvidia/protowire-rust/issues/105), protowire#558, draft -01 § Directives). It wrote only `@type`, comments and entries, so a `@dataset` document formatted to nothing and `@entry`, `@proto` and named directives such as `@header` were dropped. It now writes them in document order, each after its comments: `@type` first, then the others as they appear in the source, since an anonymous `@proto` binds the typed directive after it. A dataset is written as its header and one row per line, an absent cell left empty and `null` kept; a named directive's body and a `@proto` body are written as they were read. A document whose only directive is `@type` formats as before. The output is byte for byte what protowire-go's `FormatDocument` writes.
- **An Any's packed value keeps a float or double `-0`** ([#98](https://github.com/trendvidia/protowire-rust/issues/98)). The decoder packs an Any body with this port's own writer, which went through prost-reflect for each field. prost-reflect takes a proto3 `-0.0` for the default and leaves it out (andrewhickman/prost-reflect#208), so `d = -0` in an Any body packed to nothing. A map value `-0` packed as `+0`. The writer now writes `-0` as protobuf-go does. A `DynamicMessage` encoded directly with prost-reflect still drops it, which is why the spec's write gate keeps `rust Scalars.float-negzero` excluded until prost-reflect fixes it.
- **An error with no source position prints protowire-go's text, not `0:0: …`** ([#138](https://github.com/trendvidia/protowire-rust/issues/138)). `marshal`, the schema check in `unmarshal_full` and `DatasetReader` built their errors at `Position::default()`, which printed as `0:0: ` before the message: `0:0: pxf: no @dataset directive in stream`. Lines and columns count from 1, so 0:0 is no position, and `Display` now prints the message alone for it, as protowire-go's plain errors print (`pxf: no @dataset directive in stream`). `PxfError::has_position` says which kind an error is. The nesting-depth and unset-required errors now read as protowire-go's do: `pxf: nesting depth exceeds MaxNestingDepth=N`, `pxf: unsetann.v1.ReqOpt.o is unset, and the field is (pxf.required)`. `check-decode` prints a rejection as protowire-go's harness does (`reject: 1:1: unknown field …`), without its extra `pxf: `.
- **`xml_to_proto` imports a `char` in a non-UTF-8 encoding as `bytes`** ([#94](https://github.com/trendvidia/protowire-rust/issues/94), protowire#494, draft -01 § Character Encoding). It ignored `characterEncoding`, so a char declared as, say, `ISO-8859-1` became a `string`, and the first byte outside UTF-8 then failed the whole message. A char is now a `string` only when its encoding is absent (FIX SBE's default, US-ASCII), a US-ASCII name or a UTF-8 name, compared ignoring case and punctuation; otherwise it is `bytes` with the same `(sbe.length)` and a comment naming the encoding. `XmlType` gains `character_encoding`, so a struct literal of it needs the new field.
- **A Duration past about 292 years is written as a block, and a longer literal is refused** ([#93](https://github.com/trendvidia/protowire-rust/issues/93), draft -01 § Durations, protowire-go#226). A duration literal holds a signed 64-bit count of nanoseconds. `marshal` wrote a longer one as a longer literal (`2638888h53m20s` for 9,500,000,000 seconds), which protowire-go refuses to read; it now writes `{ seconds = 9500000000 }`, in a field, a list element or a map value. The decoder now refuses a literal past the range (`out of range`) where it used to accept it, and an enormous one (`99999999999999999999999999999h`) no longer overflows: it panicked a debug build and read as a wrapped value in a release build.
- **A `pxf.Secret` can be written as a string literal** ([#97](https://github.com/trendvidia/protowire-rust/issues/97), draft -01 § pxf.Secret). `pw = "x"` sets `value` alone, in a field, a list element or a map value; it used to be rejected (`expected '{' for message field "pw"`). `marshal` now writes a Secret that has no `hint` or `fingerprint` that way, and one that has either as a block, as protowire-go does.
- **Float and double literals follow the draft** ([#96](https://github.com/trendvidia/protowire-rust/issues/96), draft -01 § Scalar Values). The decoder now reads the identifiers `inf`, `+inf`, `-inf` and `nan`, which `marshal` already wrote, so a document with a non-finite value now round-trips. `nan` is stored as the canonical quiet NaN (0x7FF8000000000000, 0x7FC00000). A literal that rounds to an infinity, such as `1e400` for a double or `3.4028236e38` for a float, is now rejected (`invalid double: 1e400`) where it used to become infinity; that includes a `(pxf.default)`. A `+` is valid only in `+inf`.
- **`pxf::marshal` writes an Any as a body only when the body gives back its bytes** ([#88](https://github.com/trendvidia/protowire-rust/issues/88), protowire#501, draft -01 § Any Blocks). With a `type_resolver`, an Any used to get `@type` and a body whenever its value parsed. A reader compares an Any by its bytes, so a body that lost or reordered them changed the message with no error. Now the body is decoded as `unmarshal_full` would read it, at the depth it lands, and the Any is written as `type_url` and `value` unless that gives back exactly the stored bytes. That covers unknown fields at any level, field or map order, an explicitly written zero, a Timestamp no literal holds, and an unset `@default` or `@required` field.
- **The decoder packs an Any body's map entries in key order** ([#88](https://github.com/trendvidia/protowire-rust/issues/88)). They were packed in the map's hash order, so the same document could read to different `value` bytes from one run to the next. They now come out as protobuf-go's deterministic writer and protoc write them.
- **`check-decode --format pb` bounds a `pxf.BigFloat`'s prec** ([#87](https://github.com/trendvidia/protowire-rust/issues/87), protowire#499). A prec whose literal needs more than `MaxNumericLiteralDigits` digits, above 13603 at the default, is rejected before anything is built for the value, as a precision suffix already is; `--limit MaxNumericLiteralDigits=N` moves the bound. The library's paths that compute the value, `format_big_float` and the `@default` carrier, already refused such a prec.
- **`sbe::marshal` cuts an overlong string on a character boundary** ([#85](https://github.com/trendvidia/protowire-rust/issues/85), protowire#495, draft -01 § SBE Annotations). A string longer than its `(sbe.length)` was cut at the byte limit, which can split a multi-byte character, and every strict decoder then refused the whole message. `ABCDEFGé` at length 8 was written `41 42 43 44 45 46 47 C3`; it is now cut at the last whole character and NUL-padded, `41 42 43 44 45 46 47 00`. A limit shorter than the first character leaves the field empty. A `bytes` field is still cut at the limit.
- **`Presence` reports a keyed entry's name as setting the key field** (protowire-go#214). `kd { alpha {} }` now reports `kd[0].name` as set, nested entries too (`kn[0].inner[0].name`).
- **Each list element and map value has its own presence path**, as in the reference (protowire-go#156): `items[0].label`, `by_id["a"].label`, `by_num[7].label`. A field set inside an element used to be recorded under the parent, so `is_set("items[0].label")` was false and `is_set("label")` could be true. That recording satisfied the parent's `(pxf.required)` and kept its `(pxf.default)` off when an element's field had the same name, so such a document now takes the default, or is rejected if the required field is really absent. A `null` inside an element now belongs to that element: `marshal` with `null_fields` used to write it on every element of the list, and the root's `_null` mask no longer gets it (protowire-go#170).

## [1.2.0] — 2026-11-01

This release includes everything listed under 1.1.0, which was tagged but never reached crates.io: its publish failed authenticating to the registry. The last version on crates.io before this one is 1.0.0.

Documents that used to decode can now be rejected, and `marshal`'s text for every `pxf.BigFloat` changes. See Changed and Fixed below.

### Added

- **`@dataset` value cells** ([#52](https://github.com/trendvidia/protowire-rust/issues/52), protowire#341). A cell holds any value, list and block included, and binds as the body entry `c = <value>` would. `{}` is a present empty message; an empty cell stays absent.
- **`@dataset` dotted column paths** ([#54](https://github.com/trendvidia/protowire-rust/issues/54), protowire#342). A column may be a path into a sub-message, and a row's cells sharing a prefix land in one block. A header that names a path twice (a plain column too, so `(a, a)` is now rejected), names a field beside a path inside it, or has an empty segment is rejected by `parse`; a path the row type can't follow is rejected when a row binds.
- `Entry::TypeEntry`, the AST node for an Any block's type entry. A `match` on `Entry` needs the new arm.
- `proto/pxf/secret.proto`, vendored from protowire, for the dataset corpus.

### Changed

- **`pxf.BigFloat`'s text form carries its precision** ([#59](https://github.com/trendvidia/protowire-rust/issues/59), protowire#406, #410). `marshal` writes N = (decimal digits of 2^prec) + 1 significant digits followed by `p<prec>`, so a value reads back exactly: `0.10000000000000001p53`, `0p64`, and a bare `0` only at prec 0. A suffixed literal is read at the suffix's precision and an unsuffixed one at 256 bits. A suffix on any other field, `p0` and a precision past `MaxNumericLiteralDigits` are rejected, and `MaxNumericLiteralDigits` now counts mantissa digits only. An unsuffixed zero is stored with prec 0 and exponent 0, where it used to be prec 256 and exponent -256. Text written by earlier releases still reads; its bytes are the same, except that value.
- `Violation` gains `surface`, and `annotations` gains `AnnotationSurface`. `IntVal`, `FloatVal` and `Token` gain `prec`, and `parse_big_float` takes the suffix and the exponent bound.
- **`Violation.name` holds the `(pxf.default)` literal for `(pxf.required)` with `(pxf.default)` on one field** (protowire-go#211). It was empty.

### Fixed

- **`cargo doc` builds with warnings denied, and CI now requires it** ([#76](https://github.com/trendvidia/protowire-rust/issues/76)). Ten broken or private intra-doc links are fixed.
- **The decoder reads what it does not bind, instead of counting braces** ([#60](https://github.com/trendvidia/protowire-rust/issues/60), protowire#312). A field `discard_unknown` drops and a directive's body are now held to the grammar, as `parse` already held them: a `,` or `;` between the entries of such a block is rejected, as is an unterminated one. The `testdata/blocks/` corpus is vendored and runs through `parse` and the decoder.
- **`null` binds only to a field with explicit presence** ([#55](https://github.com/trendvidia/protowire-rust/issues/55), protowire#339, #379, #380). On a plain proto3 scalar, a repeated or map field, or a proto2 `required` field it is now an error naming the field, in a `@dataset` cell too. A schema that bound `null` to a plain scalar needs `optional` or a wrapper. Two editions fixtures cannot run: `prost-reflect` 0.16.5, its latest release, refuses `syntax = "editions"`, and a test pins that.
- **A simple string holds no raw line feed** (#55, protowire#350), through `parse`, the decoder and every `@dataset` path. `bind_row` now re-escapes a cell's string, so an escaped `\n` in a cell still binds.
- **A singular field bound twice in one message body is rejected** ([#56](https://github.com/trendvidia/protowire-rust/issues/56), protowire#347), whatever the values, and two blocks for one message field no longer replace each other. Each body is its own scope, and repeated fields still concatenate.
- **A map key bound twice in one message body is rejected, and two bindings of a map with distinct keys merge** ([#58](https://github.com/trendvidia/protowire-rust/issues/58), protowire#396). Keys compare as values of the key type, so `1:` and `"1":` are one key of an integer-keyed map. The second binding used to replace the whole map.
- **The type entry of an Any block is accepted wherever a body is written** ([#53](https://github.com/trendvidia/protowire-rust/issues/53), protowire#354). `@type = "<url>"` opens a singular Any, a list element or a map value, through `parse`, the decoder and `format`. A type entry that isn't first, a second one, and one on a block that isn't an Any are rejected. `unmarshal_full` now applies `(pxf.required)` and `(pxf.default)` inside an Any body before packing it.
- **`DatasetReader` ends the rows only at end of input or a directive** (protowire#336). Anything else where a row could start, and a row cut short by end of input, used to end the sequence silently; both are now errors.
- **`bind_row` applies `(pxf.required)` and `(pxf.default)`, as its documentation always said.** It runs the row through `unmarshal_full`, and an error now names the column it fell in and is reported at the row.
- **`pxf.BigFloat`'s binary exponent is bounded by `MaxBigFloatExponent` = 16384** ([#57](https://github.com/trendvidia/protowire-rust/issues/57), protowire#281). A literal past it is rejected after rounding to its read precision, `marshal` fails on one, and `Limits::max_big_float_exponent` lowers it per call. Rendering such a value used to take seconds to minutes. `check-decode` applies the bound to the PB rows.
- **`@required` and `@default(v)` are read** ([#65](https://github.com/trendvidia/protowire-rust/issues/65), RFC-001 §8.1). A schema using the annotation spelling used to bind with neither enforced. `is_required`, `get_default`, the decoder and every bind-time check now read the 1327 carrier as well as the bracket options, whether the pool knows that extension or not. Two spellings of a default that disagree are a violation, and an `@default` that reduces to no single literal is too. A diagnostic quotes the spelling the author wrote.

## [1.1.0] — 2026-10-31

Tagged, but not published to crates.io. Its changes first shipped in 1.2.0.

### Fixed

- **`marshal` refuses a `pxf.Decimal` past the bounds, and no longer hangs** (protowire#448). A scale near `i32::MAX` padded one digit at a time, and a negative one wrote up to 2^31 zeros. A scale past `MaxNumericLiteralDigits` on either sign now fails before any work, as does a `BigInt` or `Decimal` literal past the digit cap.
- **A `pxf.BigFloat` with no literal fails instead of falling back to the block form**, as the reference does.

### Changed

- **`marshal` returns `Result<String, PxfError>`,** and `format_big_int` and `format_decimal` return `Result<String, String>`, so they can refuse a value.

### Fixed

- **`(pxf.required)` with `(pxf.default)` on one field is a bind-time
  violation** (draft `-01` § Required and Default;
  [#62](https://github.com/trendvidia/protowire-rust/issues/62)). Such a
  schema used to bind, with a meaning that could differ between engines.
  It now fails at bind time as `ViolationKind::RequiredOption`, over the
  import closure like the other bind-time checks, whatever the document
  contains.
  `(pxf.required) = false` beside a default stays accepted.
  
- **A `metadata` map entry on `AppError` counts toward `MaxNestingDepth`**
  ([protowire#449](https://github.com/trendvidia/protowire/issues/449)).
  HARDENING counts every PB map entry as a descent, whatever its value's
  type, and the entry was read by hand with no depth check. It now goes
  through `read_message`, the same as any nested message, which also
  checks its length: a length of 2^64 - 1 used to overflow an add (a panic
  in a debug build), and an entry whose fields ran past its length was
  accepted. Both are clean errors now. `check-decode`'s `Tree` gains
  `branches` and `tags` for the corpus's map rows; PXF already counted a
  map's `{`, and a test now pins that.

- **A dotted string map key is written bare** (draft `-01` § Entries and
  Keys, the Rust leg of
  [protowire#313](https://github.com/trendvidia/protowire/issues/313)).
  `is_valid_ident` (the marshaller) and `needs_quoting` (`format`)
  stopped at `[A-Za-z0-9_]`, while the grammar's *ident-part* — and
  `ident_safe_entry_name`, the test keyed entry names use — admits `.`;
  so a key `a.b` was marshalled `"a.b":` and a quoted `"a.b"` kept its
  quotes through `fmt`, where the text says bare. The decoder always read
  `a.b:` as one identifier token, so only the writers move: both now
  delegate to `ident_safe_entry_name`, one identifier-safe rule for the
  document. `".e"` and `"1.5"` still fail *ident-start* and stay quoted.
  The spec's third fmt pair, `fmt-dotted-keys`, is vendored and pinned.
  A canonical-text change for dotted string keys; no binding or wire
  change.

### Added

- **Keyed repeated fields** (draft `-01` §3.13; [#20](https://github.com/trendvidia/protowire-rust/issues/20),
  [protowire#116](https://github.com/trendvidia/protowire/issues/116)).
  A `repeated <Message>` field carrying `(pxf.key)` may be written as a
  block of named blocks — entry name = key-field value, entry order = list
  order — and the anonymous list form stays valid for the same field. The
  grammar accepts a quoted entry name anywhere (`field_entry = (identifier
  | string) …`); `Assignment` and `Block` record whether it was quoted, and
  the schema-less formatter reproduces it. The decoder reads the keyed
  block in both spellings (`name { }` and `name = { }`), rejects duplicate
  entry names (compared unquoted), the empty key in either form, and a
  disagreeing explicit key-field assignment, and rejects a quoted entry
  name outside a keyed block; the encoder emits the keyed form whenever
  every key is present, non-empty and distinct, unquoted iff
  identifier-safe, and the anonymous form otherwise. `canonicalize_keyed`
  is the schema-aware half of `pxf fmt`: anonymous → keyed where eligible,
  `name = { }` → `name { }`, identifier-safe quoted names unquoted,
  redundant key assignments dropped. The spec repo's `testdata/keyed/`
  corpus is vendored and driven by `tests/keyed.rs`. `Assignment` and
  `Block` gain public fields (breaking for struct-literal construction).
- **A repeated field bound more than once now concatenates** in document
  order, as draft `-01` § Entries and Keys says and the reference does; a
  second `tags = [...]` used to replace the first.

- **The v1.11 bind-time placement checks, over the import closure**
  ([#25](https://github.com/trendvidia/protowire-rust/issues/25)).
  `validate_descriptor` / `validate_file` now enforce, besides the
  reserved-name rule, `(pxf.key)` placement (draft `-01` §3.13.1: a
  repeated message-typed field whose value names a singular string field
  of the element message), `(pxf.default)` placement ("Default
  Placement": never a `repeated`, `map` or group field, nor a message
  type outside `Timestamp`, `Duration`, the nine `*Value` wrappers,
  `pxf.BigInt`, `pxf.Decimal`, `pxf.BigFloat`), and the two oneof rules
  ("Oneof Members": at most one `(pxf.default)` per oneof, and no
  `(pxf.required)` on a member). All four families cover the bound file
  **and its transitive imports** ("Scope of Bind-Time Checks"): a
  violation in an imported `.proto` is reported, attributed to the file
  that declares it, a diamond is checked once, and results are memoized
  per file descriptor at two grains (closure and file) keyed on
  descriptor identity, so the wider check costs less per decode than the
  single-file walk it replaces. `ViolationKind` gains `KeyOption`,
  `DefaultOption`, `RequiredOption`; `Violation` gains `detail`; the
  decode error header is `PXF schema violations:`. The vendored
  `pxf/annotations.proto` gains `key = 1316` and `annotations::get_key`
  reads it. **This narrows accepted schema input**: a schema carrying any
  of these placements bound before and is rejected now, over the whole
  closure — every rejected placement is one no implementation could
  honor, and the diagnostic names the field by fully-qualified name (the
  spec repo's STABILITY.md records the narrowing under v1.11).

- **`pxf.BigFloat` takes its literal form**
  ([#39](https://github.com/trendvidia/protowire-rust/issues/39)), closing
  the last gap in the arbitrary-precision types. A numeric literal on a
  `pxf.BigFloat` field decodes to the reference's bytes and the message
  marshals back to the reference's text, in every position and as a
  `(pxf.default)`. The reference parses with `math/big` at 256 bits and
  that conversion is not correctly rounded — it multiplies or divides by
  a power of five rounded to 320 bits and built by square-and-multiply at
  384 — so `src/bigfloat.rs` mirrors those steps on a hand-rolled big
  integer rather than computing the nearest value, and renders with the
  reference's `%g` at 78 digits. `testdata/bigfloat-oracle.tsv`, produced
  from protowire-go v1.6.0, pins 31 literals byte for byte, including the
  three range errors. No new dependency.
- **Every HARDENING § Mandatory limit is enforced, and configurable per
  call** ([#33](https://github.com/trendvidia/protowire-rust/issues/33)).
  `MaxMessageSize` (64 MiB, the total input to one decode or parse),
  `MaxBytesLiteralLength` and `MaxRepeatedCount` were not enforced at all
  — a 65 MiB document decoded — and `MaxNestingDepth` and
  `MaxNumericLiteralDigits` were fixed constants. New: `protowire_pxf::Limits`
  on `UnmarshalOptions::limits`, `parse_with_limits`,
  `DatasetReader::with_limits` (which also caps the bytes held while
  looking for a row boundary); `protowire_pb::Limits` with
  `unmarshal_with`, `Reader::with_limits`, and the element helpers
  `Reader::push_element` / `check_repeated` / `packed` a `Message` impl
  appends through so a repeated field is refused before the element past
  the bound is added; `protowire_sbe::Limits` with
  `Codec::from_files_with_limits`, refusing a group's `numInGroup` before
  any entry is allocated. The defaults are the constants, exported from
  each crate. Rejections name the limit (`input of N bytes exceeds
  MaxMessageSize=M`), and the PXF decoder now surfaces the lexer's own
  diagnostic at a value position (`invalid duration: 5seconds`, the bytes
  cap) instead of `expected bytes for field`. `check-decode` accepts
  `--limit NAME=VALUE` for the corpus rows that prove a limit with a small
  fixture (protowire#299), links `adversarial.v1.ListHolder`, and
  **decodes SBE for real** — its SBE leg returned "not implemented", which
  the harness reads as a rejection, so every SBE corpus row passed without
  decoding a byte.

- **`pxf.BigInt` and `pxf.Decimal` take their literal forms**
  ([#34](https://github.com/trendvidia/protowire-rust/issues/34)). A bare
  integer on a `pxf.BigInt` field and an integer or decimal literal on a
  `pxf.Decimal` field decode to the wire messages of `pxf/bignum.proto`
  (big-endian magnitude, sign flag, and for `Decimal` the scale exactly as
  written: `1.00` has scale 2), in singular, repeated and map-value
  positions and as a `(pxf.default)`; the encoder writes them back as
  literals. The conversions are hand-rolled in `src/bignum.rs` — no
  bignum dependency — and **`MaxNumericLiteralDigits` (4096) is enforced
  before they run**: a literal of exactly 4096 digits decodes, one more
  is rejected naming the limit. Before this a `pxf.BigInt` field rejected
  every literal with `expected '{'`, which passed the corpus's reject
  rows by accident and failed the accept row protowire#279 added.
  `pxf.BigFloat`'s literal form is still open
  ([#39](https://github.com/trendvidia/protowire-rust/issues/39);
  its block form is unchanged). `check-decode` links `adversarial.v1.BigNumHolder` and
  bounds `pxf.Decimal.scale` on the PB wire on both signs.

### Changed

- **The vendored shared schemas are re-synced with [trendvidia/protos](https://github.com/trendvidia/protos) v0.1.2** ([trendvidia/protowire#447](https://github.com/trendvidia/protowire/issues/447)). They gain protos' file options, and `envelope.proto`'s `go_package` now names a published package. Declarations and field numbers are unchanged, and so is this port's public API.

- **Accepted input narrows at the HARDENING defaults**: a document, PB
  message or SBE buffer over 64 MiB is rejected where it decoded before,
  and the SBE decoder and `View` reject a wire `block_length` below the
  template's (HARDENING § SBE step 2), a zero entry `block_length` with a
  non-zero count (step 4), and a group whose `count × block_length` runs
  past the buffer — `View::group` used to accept the header and let
  `entry()` index out of bounds. STABILITY.md promise 1 calls the size cap
  a narrowing; the spec repo's v1.13 section records why the family takes
  it. The nesting-depth message is now `MaxNestingDepth=N` in every crate.
- **The encoder writes message shorthand for map values** — a
  `Timestamp`, `Duration` or `*Value` wrapper as a map value was written
  as a block (`at: { seconds = … }`) where singular and repeated positions
  and every other port write the literal (`at: 2026-01-01T00:00:00Z`).
  Both forms still read; marshal output for such maps changes.
- **A `pb` map entry always carries both its key and its value, zero-valued
  or not** ([#32](https://github.com/trendvidia/protowire-rust/issues/32);
  decided family-wide in
  [protowire#295](https://github.com/trendvidia/protowire/issues/295)).
  The envelope codec wrote a `metadata` entry the way it writes any
  message, with proto3 zero-skipping inside it, so `"" → ""` went out as
  `22 02 2a 00` where protobuf-go, protoc and C++ protobuf write
  `22 06 2a 04 0a 00 12 00`. Every reader in the family accepted all three
  layouts measured, so the divergence was lossless and invisible until the
  spec repo added a golden-checked wire vector. This **changes pb bytes**
  for entries with an empty key or value (STABILITY.md promise 2; the spec
  repo's v1.13 section records why they move); the reader still takes the
  old omission. `protowire_pb::write_map_entry` and the `MapEntryField`
  trait are new, so a `Message` impl no longer hand-rolls the entry — every
  hand-rolled one had got it wrong the same way. `dump-envelope --vector
  NAME` prints a named vector for the gate, and exits 3 with
  `not-implemented: NAME` for one this port has not built.
- **MSRV raised from 1.82 to 1.85.** `prost` 0.14.4 raised its own MSRV to
  rustc 1.85, and holding the workspace at 1.82 would have meant freezing
  a core decoding dependency off its upstream fix stream. Rust 1.85 shipped
  in February 2025. `[workspace.package].rust-version`, the CI gate, the
  README and CONTRIBUTING all move together.

  This is a compatibility change on five published 1.0.0 crates —
  `protowire`, `protowire-pb`, `protowire-envelope`, `protowire-pxf`,
  `protowire-sbe` — and by the usual Rust convention wants a minor
  release rather than a patch.

- Dependency bumps taken with it: `prost` 0.14.3 → 0.14.4, `prost-types`
  0.14.3 → 0.14.4, `prost-reflect` 0.16.4 → 0.16.5, `bytes` 1.11.1 →
  1.12.1, `thiserror` 2.0.18 → 2.0.20. No wire-format or API change.

### Fixed

- **`format` keeps the spelling of a map key where changing it would
  change what the key denotes**
  ([protowire#306](https://github.com/trendvidia/protowire/issues/306),
  option 2). Since #36 a bare `true` / `false` is a bool key, so
  `"true": "v"` on a `map<string, V>` and `true: "v"` are different
  documents — and the formatter wrote the second for the first, turning a
  document that binds into one that does not (`"null"` → `null`, no key at
  all, likewise). `ast::MapEntry` gains `quoted`, the parser records it,
  and the formatter reproduces it: a bare key stays bare; a quoted key is
  unquoted only when it is identifier-safe and not a value keyword. One
  existing output moves: a bare integer key was written quoted
  (`404:` → `"404":`) and stays bare now, as the marshaller has always
  written it. The spec repo's `fmt-keyword-keys` and `fmt-bare-keys`
  pairs are vendored under `testdata/map-keys/` and pinned as fixed
  points. `MapEntry` gains a public field, which is breaking for
  struct-literal construction.
- **The lexer reads fractional and `µs` duration literals** (`1.5ms`,
  `312.5µs`, `1h30m0.5s`, `2µs`)
  ([#26](https://github.com/trendvidia/protowire-rust/issues/26)). Draft
  `-01` §3.3 admits `duration-segment = 1*DIGIT [ "." 1*DIGIT ] time-unit`
  with `µs` (U+00B5) among the units, and the encoder writes exactly those
  forms for any `google.protobuf.Duration` that is not a whole multiple of
  its largest unit — so this port could not read its own output for a
  measured latency. Two defects inherited from the Go reference lexer
  (fixed there in protowire-go#76): the number path decided *float* on
  seeing `.` before it looked for a unit, and the duration scan was
  ASCII-only. The lexer now consumes an optional fraction first and takes
  the duration branch when a unit (or the `C2 B5` micro sign) follows;
  `1.5` stays a float, `1.5e3ms` stays a float plus an identifier, `1.ms`
  stays `1.` plus an identifier, and U+03BC GREEK SMALL LETTER MU is not a
  unit. The 47-case token table from protowire-go and a marshal → read-back
  property test over every `Duration.String()` branch pin it. An illegal
  non-ASCII character is now reported as one token naming the character,
  not one per UTF-8 byte.
  
- **Bool map keys accept exactly the spellings the grammar admits**
  ([#31](https://github.com/trendvidia/protowire-rust/issues/31)). A
  `map<bool, V>` key was matched against the text `true` / `false` only, so
  the bare integers `0` / `1` — the "bool encoded as 0/1" spelling draft
  `-01` § Entries and Keys names, which Go and Java bind — were a syntax
  error here. A bool key is now the keyword `true` / `false` bare (decided
  in [protowire#284](https://github.com/trendvidia/protowire/issues/284)),
  the bare integers `0` / `1`, or the quoted literals `"true"` / `"false"`;
  every other spelling (`t`, `TRUE`, `yes`, `"1"`, `"0"`, `"TRUE"`, …) is an
  error naming the key. The keyword bare on a `map<string, V>` is rejected
  too — the string is spelled quoted — and the AST parser admits a bool key
  with the `:` tail so `format` and `validate` see the same documents the
  decoder does. The spec repo's `testdata/map-keys/` corpus is vendored
  under `crates/protowire-pxf/testdata/map-keys/` and driven by
  `tests/map_keys.rs`.
  
- **`(pxf.default)` on a oneof member no longer destroys the arm the
  document chose** ([#24](https://github.com/trendvidia/protowire-rust/issues/24)).
  `post_decode` tested presence per field, so a member's default was
  applied over a chosen sibling — and setting a oneof member clears the
  rest, so `a = "written"` decoded as `b = "bbb"` with `a` gone. The
  default now applies only when no member of the oneof is present in the
  document (a member bound to `null` counts as present), per draft `-01`
  §annotation-extensions "Oneof Members". A proto3 `optional` field's
  synthetic oneof is excluded, so its default keeps applying.
  
- **`(pxf.default)` on a `repeated` or `map` field is an error, not a
  one-element list** ([#23](https://github.com/trendvidia/protowire-rust/issues/23)).
  `apply_default` dispatched on the element kind and handed `set_field` a
  scalar for a list field, which prost-reflect accepts and encodes as
  `tags = ["ignored"]` — pb bytes no other port emits. Draft `-01`
  "Default Placement" forbids inventing that semantics; the decode now
  fails with `default values not supported for repeated field "tags"`
  (the message protowire-typescript and protowire-go already produce),
  and the map case names the placement rather than the synthetic
  `…Entry` type. The bind-time half of the rule is
  [#25](https://github.com/trendvidia/protowire-rust/issues/25).
  
- README and CONTRIBUTING both claimed an MSRV of **1.74**, which had not
  matched `Cargo.toml` since the pin moved to 1.82. Both now state 1.85,
  and CONTRIBUTING no longer describes the workspace as depending on
  `prost 0.13`.

## [1.0.0] — 2026-05-13

First major-version cut. Implements the three one-time spec changes
from the [protowire v1.0 freeze line](https://github.com/trendvidia/protowire/releases/tag/v1.0.0)
in lockstep with the other v1.0 ports. **Breaking** — there is no
alias period; v1.0 is itself the major bump.

### v1.0 spec changes

- **`@table` → `@dataset` rename** (draft §3.4.4). Public API
  rename: `ast::TableDirective` → `ast::DatasetDirective`,
  `ast::TableRow` → `ast::DatasetRow`, `TableReader` →
  `DatasetReader`, `Presence::tables()` → `Presence::datasets()`,
  `Presence::add_table()` → `Presence::add_dataset()`,
  `TokenKind::AtTable` → `TokenKind::AtDataset`. Source files
  `src/table_reader.rs` → `src/dataset_reader.rs`;
  `tests/table_reader.rs` → `tests/dataset_reader.rs`. Hard cutover.

- **`@proto` directive added** (draft §3.4.5). New `ast::ProtoDirective`
  + `ast::ProtoShape` enum (`Anonymous`, `Named`, `Source`,
  `Descriptor`). Four body shapes lexically distinguished.
  Exposed via `Document::protos` and `Presence::protos()`.
  Descriptor form is the MUST-support shape; this port supports all
  four.

- **Reserved directive names** expanded from 5 to 13 (draft §3.4.6).
  `schema::is_future_reserved_directive(name)` exported. Parser +
  fast decoder reject `@table`, `@datasource`, `@view`,
  `@procedure`, `@function`, `@permissions` as spec-reserved.

`@dataset`'s row message type is now optional in the AST — binding
to an anonymous `@proto` per draft §3.4.4 Anonymous binding.
`Lexer::reposition_to(target)` and `Lexer::input_bytes()` added so
the parser can skip past an `@proto` brace-body whose interior is
protobuf source rather than PXF.

### Build

- Workspace version `0.75.0` → `1.0.0` (Cargo.toml + inter-crate
  path-dep version pins).

### Tests

- New `crates/protowire-pxf/tests/proto_directive.rs` with 13 cases
  covering all four `@proto` body shapes, anonymous binding,
  multi-`@proto`, nested-brace bodies, three error paths, parametric
  reserved-name rejection, and `ProtoShape::name()` lookup.
- `cargo test --workspace`: all tests pass.

## [0.75.0] — 2026-05-12

First release after the v0.70.0 baseline that closes the v0.72–v0.75
gap with the rest of the `protowire-*` stack (Go, Java, cpp, python,
TypeScript). All four PXF v0.72-series features are now available in
the Rust port, in lockstep with what the sibling ports shipped over
their v0.72 → v0.74 → v0.75 cuts. The Rust port skips intermediate
version numbers and lands the bundled feature set directly on v0.75.0
to match the active wire revision.

### Added

- **`TableReader` streaming `@table` consumption + `bind_row`
  per-row binding** (draft §3.4.4). `unmarshal_full` materializes
  every row of an `@table` directive into `Presence::tables`; that
  works for small datasets and breaks for the CSV-replacement
  workload `@table` was designed for. New
  `protowire_pxf::table_reader` module exposes:
  - `TableReader<R: Read>::new(R)` — consumes leading directives
    and the `@table TYPE ( cols )` header from any `io::Read`
    source. Header capped at 64 KiB (`DEFAULT_HEADER_MAX_BYTES`)
    to fail-fast on misuse.
  - `type_name()` / `columns()` / `directives()` / `done()` accessors.
  - Implements [`Iterator`] (`Item = Result<TableRow, PxfError>`)
    so `for row in reader` just works; `next_row()` is the
    non-iterator entry point. Per-row arity and v1 cell-grammar
    checks happen at consume time. Errors are sticky.
  - `scan_one(desc, options)` — `next_row` + `bind_row` in one call;
    returns `Ok(Some(msg))` or `Ok(None)` at EOF. Named
    `scan_one` because `Iterator::scan` would shadow `scan`.
  - `tail()` — returns an `impl Read` that yields the buffered + the
    remaining underlying bytes, so callers can chain a second
    `TableReader` for multi-`@table` documents.
  - `bind_row(desc, columns, row, options)` — exported helper for
    callers iterating `Presence::tables()[i].rows` from the
    materializing path. Strategy is format-and-reparse — render
    cells as a synthetic PXF body and run through `unmarshal`,
    reusing every branch of the existing decoder. Callers in a
    tight scan loop typically set `options.skip_validate = true`.

- **`Presence::directives()` and `Presence::tables()` accessors.** The
  direct decoder now populates the document-root directive list and
  `@table` directive list on `Presence` during `unmarshal_full`, so
  consumers can read them after a decode call.
  - `Presence::directives()` returns the generic
    `@<name> *(prefix) [{ ... }]` blocks in source order, with raw
    body bytes (`Vec<u8>`) preserved verbatim for downstream re-
    parsing (chameleon's `@header T { ... }` reader, etc.). A single
    prefix populates the back-compat `type` field; two or more leave
    it empty and consumers read `prefixes` directly.
  - `Presence::tables()` returns the `@table` directives with full
    column metadata and parsed cell values per row, faithful to the
    three-state cell grammar (absent / present-but-null /
    present-with-value, draft §3.4.4). Cells are
    `Vec<Option<Value>>` — `None` for absent, `Some(Value::Null)`
    for present-but-null.
  - `unmarshal` (vs `unmarshal_full`) still passes no `Presence` and
    walks directives without allocating directive AST nodes — the
    direct path retains its zero-allocation prelude on the hot path.

- **PXF schema reserved-name validator (draft §3.13).** Rejects
  protobuf schemas that declare a message field, oneof, or enum value
  whose name is case-sensitively equal to a PXF value keyword
  (`null` / `true` / `false`) — such a name lexes as the keyword and
  the declared element is unreachable from PXF surface syntax. New
  `protowire_pxf::schema` module exposes:
  - `validate_descriptor(&MessageDescriptor)` /
    `validate_file(&FileDescriptor)` return a sorted
    `Vec<Violation { file, element, name, kind }>`.
  - `ViolationKind::{Field, Oneof, EnumValue}` and a `Display` impl
    that renders one-line human-readable text.
  - `UnmarshalOptions` gains `skip_validate: bool` for consumers that
    validate once at registry-load time and don't want the per-call
    recheck cost.
  - `unmarshal` and `unmarshal_full` invoke the validator before
    decode; violations come back as a `PxfError` with a multi-line
    message (one `Violation::to_string()` line per offender).
  - Synthetic oneofs from proto3 `optional` fields are filtered
    automatically — prost-reflect's `OneofDescriptor::is_synthetic()`
    matches the Go reference's `IsSynthetic()` filter.

- **PXF parser-side `@<name>` / `@entry` / `@table` directive grammar**
  (draft §3.4.2 – §3.4.4). The AST `Document` now carries `directives`
  (generic `@<name> *(prefix) [{ ... }]` entries) and `tables`
  (`@table <type> ( cols ) row*` entries) alongside `type_url` and
  `entries`. `Directive::body` preserves the raw bytes between `{`
  and `}`; `Directive::type` keeps the legacy single-prefix shape
  for v0.72.0-era consumers. `Document::body_offset` marks the byte
  right after the last directive (used by chameleon for hashing the
  schema-typed payload).

  Both the AST parser and the direct decoder consume the new forms;
  runtime semantics (`Presence` accessors, `TableReader` streaming,
  per-row `bind_row`) follow in subsequent PRs of the v0.72-v0.75
  catch-up. The decoder discards directive contents for now and
  enforces the standalone constraint (draft §3.4.4): a document
  containing any `@table` directive MUST NOT also carry `@type` or
  top-level field entries.

  `Position` gains an `offset` field (byte offset into the lexer's
  input) so directive body extraction can slice raw bytes; existing
  callers that read only line / column are unaffected.

## [0.70.0]

Initial public release. The version number aligns this port with the rest
of the `protowire-*` stack, which targets the 0.70.x series for the first
coordinated public release.

### Added

- **crates.io distribution** for the public crates: `protowire`
  (umbrella), `protowire-pb`, `protowire-pxf`, `protowire-sbe`,
  `protowire-envelope`. The `bench-pxf`, `bench-sbe`,
  `dump-envelope`, and `check-decode` workspace members are internal
  test harnesses and stay unpublished.
- **HARDENING.md decoder safety** (M8): bounded recursion depth and
  PB length-prefix overflow rejection in `protowire-pxf` and
  `protowire-pb`. Verified by the `check-decode` adversarial corpus
  reference under `crates/check-decode/`.
- **Comprehensive CI matrix**: build + test on stable/beta/MSRV across
  Linux/macOS/Windows, plus `cargo fmt --check`, `cargo clippy
  --all-targets --all-features -- -D warnings`, and `cargo miri test`
  on the codec crates. Weekly CodeQL SAST.
- **Governance scaffolding**: `LICENSE` (MIT), `CONTRIBUTING.md`,
  `SECURITY.md` (security@trendvidia.com), `GOVERNANCE.md`,
  `CODE_OF_CONDUCT.md`, `.github/CODEOWNERS`, issue + PR templates,
  Dependabot for cargo + GitHub Actions.

### Changed (breaking)

- **PXF parser stricter on key forms**, mirroring the upstream grammar
  tightening in
  [`trendvidia/protowire@8262bbb`](https://github.com/trendvidia/protowire/commit/8262bbb)
  (`docs/grammar.ebnf`, `docs/draft-trendvidia-protowire-00.txt`):
  - `=` (field assignment) and `{ … }` (submessage) now require an
    identifier key. Inputs like `123 = 234` or `child { 123 = 123 }`
    are now parse errors with
    `"field assignment with '=' requires an identifier key, got integer
    (\"123\"); use ':' for map entries"`.
  - `:` (map entry) is rejected at document top level — the document
    represents a proto message, never a `map<K,V>`. Use `=` for
    top-level field assignments. Map literals (`field = { 1: "x" }`)
    still work because `:` remains valid inside `{ … }` blocks.
