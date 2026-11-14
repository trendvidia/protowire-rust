# Changelog

All notable changes to `protowire-rust` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The version number is kept aligned with the rest of the `protowire-*`
stack — releases bump in lockstep across language ports when the wire
format changes.

## [Unreleased]

### Added

- **`Rewriter`: format-preserving edits** ([#131](https://github.com/trendvidia/protowire-rust/issues/131), protowire-go's `Rewriter`). A tool can now change a field in a document that a person also edits by hand, and keep everything else byte for byte: comments, blank lines, key order, indentation, and how values were spelled.
  - **Operations, as protowire-go's:** `set` (by dotted path, creating missing blocks), `remove`, `replace_value` (the exact node held from `document()`), `set_span` (raw bytes), `append_entry`, and `bytes`, which applies the staged edits. `bytes` refuses edits that overlap, and parses the result again, refusing one that is not valid PXF. Inserted text is indented like its siblings, using the document's own indent step.
  - **Keyed collections** (draft -01 §3.13), as protowire-go's: `set_keyed`, `remove_keyed`, `insert_keyed_element`, `remove_keyed_element`, `rename_keyed_element` and `move_keyed_element` take the element's key as one atom, so a key with dots or spaces is addressed whole. They search every binding of a field bound more than once. They edit the anonymous list form (`f = [ { id = "x" } ]`) once `keyed_by_field` names its key field, and keep the comments on the other elements and on the moved one.
  - **API** (the shape decided on #131): `document()` returns an `Arc<Document>`. A caller holds it and passes its nodes to `replace_value` and `append_entry`, which take `&mut self`.
  - **Tests:** `tests/rewrite_oracle.rs` holds each operation's verdict, its error text and the rewritten document to protowire-go v1.17.0 (`scripts/gen-rewrite-oracle.sh`). The cases are protowire-go's rewriter tests as data, `testdata/conformance/rewrite-cases.txt`, vendored from protowire-java, which replays the same rows. Four cases are this port's own, for what no protowire-go test reaches: the overlap check at its one-byte boundary, the reparse refusal, and `set_keyed` creating an element in the last of two bindings. All 412 cases run. Six operations pass Go's `nil` or a negative offset, which Rust's types rule out, and are skipped; protowire-go refuses each without staging an edit, so the rest of their cases still run.
- **Node spans: an `end` on every entry and value, and `entry_span` / `value_span`** ([#131](https://github.com/trendvidia/protowire-rust/issues/131), protowire-go's `End`, `EntrySpan` and `ValueSpan`). A tool can now find the exact source text of any part of a parsed document, which is what an in-place rewriter edits.
  - **Where a node ends, as protowire-go's:** an entry ends just past its value, and a block past its `}`. A scalar ends past its last byte, a precision suffix included; a list or block value past its `]` or `}`. Leading comments and the whitespace around a node are not in its span. A placeholder from `parse_tolerant` spans nothing. A block or list that `parse_tolerant` closes early ends where it stopped, and one skipped past the depth limit ends past its own close.
  - **Tests:** `tests/tolerant_oracle.rs` now holds every entry's and value's span to protowire-go v1.17.0 on its 751 documents, and `tests/spans.rs` ports protowire-go's span tests.
  - **Breaking:** `Assignment`, `MapEntry`, `Block`, `TypeEntry` and every value struct have a new public field, `end`. A node built by hand outside this crate needs `end: Position::default()`, which is protowire-go's zero `End`.
- **Message streams in pb, sbe and envelope** ([#128](https://github.com/trendvidia/protowire-rust/issues/128), protowire-go's stream `Encoder`s and `Decoder`s). A Rust program can now read the streams a Go program writes, and write ones Go reads.
  - **Framing, as protowire-go's:** pb and envelope frames are a varint length then the body (protobuf-delimited). An SBE frame has the 6-byte Simple Open Framing Header, total length and encoding type 0xEB50, before the message. `DEFAULT_MAX_FRAME_SIZE` is 16 MiB, and `Decoder::set_max_frame_size` changes it.
  - **API** (the shape decided on #128): `Encoder::encode(&msg)`, and `Decoder::decode(&mut msg) -> Result<bool, StreamError>`, which is true when a frame was read and false at a clean end of stream. pb: `Encoder::new`/`with_options` and `Decoder::new`/`with_options`, whose `UnmarshalOptions` give each frame's limits and validator. sbe: `Codec::new_encoder` and `Codec::new_decoder`, which use the codec's limits and validator. envelope: `Encoder` and `Decoder` over pb's.
  - **Errors:** each crate's new `StreamError` is `FramingCorrupt`, `UnexpectedEof`, `Io` or `Decode`, with `is_framing_corrupt()`. Its texts are protowire-go's (`pb: stream framing corrupt: frame size 1073741824 exceeds max 1048576`). The first three leave the stream unreadable and are returned again on every later call. A frame whose body fails to decode, or that the validator refuses, is a `Decode` error, and the next call reads the next frame. The existing `pb::Error` and `SbeError` are unchanged.
  - **Tests:** `tests/stream.rs` in each crate ports protowire-go's `stream_test.go`, and sbe's also ports `TestStreamDecoderRunsValidator`. Each test reads the stream protowire-go v1.17.0's encoder wrote (`scripts/gen-stream-oracle.sh`) as Go's messages, and writes it byte for byte.
- **`parse_tolerant` and `parse_tolerant_with_limits`** ([#130](https://github.com/trendvidia/protowire-rust/issues/130), protowire-go's `ParseTolerant`). For editor tooling: the tree of what a broken document holds, with every syntax error in source order, where `parse` stops at the first. A document `parse` reads gives the same tree and no error.
  - **Recoveries, as protowire-go's:** a missing or malformed value is a placeholder, the new `Value::Bad(BadVal)`, so its entry stays in the tree. An unclosed block or list is closed at the end of input, or at the `)` of the `@dataset` row it is a cell of. An unterminated string ends at its newline; a triple-quoted string, block comment or bytes literal at the end of input. A malformed directive is skipped to the next directive or entry.
  - **Formatting:** `format` writes nothing for a placeholder, as protowire-go does, so a formatted tolerant tree that holds one may not parse again.
  - **Tests:** `tests/tolerant_oracle.rs` holds the errors, their positions and the whole tree to protowire-go v1.17.0 on 753 documents (`scripts/gen-tolerant-oracle.sh`). They are every document of its `tolerant_test.go`, one for each error path of its parser and lexer, every prefix of three documents (`testdata/tolerant/`), and every vendored `.pxf` fixture. Each case also holds `parse` to protowire-go's `Parse`.
  - **Known difference:** a string whose escapes give bytes that are not UTF-8 (`"\xff"`) is refused by this port's lexer, where protowire-go keeps the bytes ([#191](https://github.com/trendvidia/protowire-rust/issues/191)). `parse_tolerant` records that error and a placeholder; two cases pin it.
  - **Breaking:** `ast::Value` has a new variant, `Bad`, so a `match` over every variant outside this crate needs a new arm.
- **Typed keyed errors and `keyed_diagnostics`** ([#130](https://github.com/trendvidia/protowire-rust/issues/130), protowire-go's `KeyedError`, `KeyedErrorKind` and `KeyedDiagnostics`). These are the errors of a keyed repeated field (draft -01 §3.13): a duplicate key, an empty key, a key conflict, or a quoted entry name outside a keyed block.
  - **Decoding:** a decode refused for one fails as before, with the same position and text. `PxfError::keyed_error()` now gives the `KeyedError`, with its kind, field, key, position and message, so a caller can tell the kinds apart without reading text.
  - **Diagnostics:** `keyed_diagnostics(&doc, &desc)` gives every keyed error in a parsed document, in source order. That way an editor can show them all instead of failing on the first. As in protowire-go, it skips entries the schema does not resolve, `Any` bodies and `@dataset` rows.
  - **Tests:** `tests/keyed_oracle.rs` holds both to protowire-go v1.17.0 on 91 cases (`scripts/gen-keyed-oracle.sh`). They are every fixture of `testdata/keyed/`, every document of its keyed tests, and Any bodies, map values, nesting and `@dataset` cells. New fixture: `testdata/keyed-undo-test.proto`.
  - **Known difference:** a keyed error in a `@dataset` cell keeps this port's text, at the row and naming the column. protowire-go reports it at its place in the decoder's synthetic body ([#178](https://github.com/trendvidia/protowire-rust/issues/178)). Its `keyed_error()` is protowire-go's exactly.
- **`PxfError::is_no_dataset`** ([#130](https://github.com/trendvidia/protowire-rust/issues/130), protowire-go's `ErrNoDataset`). A caller can tell an input with no `@dataset` directive from any other `DatasetReader::new` failure without comparing text. The error's `source()` is the new `NoDataset`, so it is also found in an error chain. Its text and missing position are as before. A malformed input still fails with `parse`'s error, which is not one.
- **A `@dataset` writer: `append_dataset_header` and `append_dataset_row`** ([#129](https://github.com/trendvidia/protowire-rust/issues/129), protowire-go's `AppendDatasetHeader`, `AppendDatasetRow`). A table can now be written as well as read. The header names the row type and its columns; each row is one message, on one line.
  - **Cells:** each is the text `marshal` writes for the field. A list, map or block is one cell on one line. A column may be a dotted path into a sub-message (`inner.x`). An unset field with `(pxf.default)` or `(pxf.required)` is written so that it reads back as it was, as protowire-go writes it: its zero, `[]`, `{}` or `null`, or an empty cell.
  - **Refusals:** a column list or a row the reader would refuse is refused, with protowire-go's text, and `dst` is left as it was. That covers a column that is not a field, a path through a list or into a literal, a required field left out, and a schema the reader refuses. A path deeper than `MAX_NESTING_DEPTH` is refused too, since this port's reader refuses it.
  - **Appending:** a row depends only on its message and the columns, so rows can be appended to a file that already holds a table.
  - **Tests:** `tests/dataset_write.rs` holds the writer to protowire-go v1.17.0 on 308 rows (`scripts/gen-dataset-write-oracle.sh`). They are every case of its `dataset_write_test.go` and `dataset_write_required_list_test.go`, and 248 rows from those tests' random generators. Each row's header, text or error must be protowire-go's. The table must read back, through `scan_one` and `bind_row`, as protowire-go's reader read it. One row's `_null` paths come back in another order ([#174](https://github.com/trendvidia/protowire-rust/issues/174)), and that case is pinned.
- **A validator hook for pxf, pb and sbe decoding** ([#126](https://github.com/trendvidia/protowire-rust/issues/126), protowire-go's `check` package). A caller plugs a rule engine, such as a protovalidate adapter, into decoding, and a decoded message that breaks its rules is refused.
  - **The new crate:** `protowire-check` holds the `Validator` trait, `Report`, `Violation`, `Error` and `validate`. It has no dependencies; pb, pxf and sbe depend on it, and `protowire::check` re-exports it. `publish.yml` publishes it first.
  - **Where it runs:** after a successful decode, and after `unmarshal_full`'s post-decode pass. Options: pxf `UnmarshalOptions::validator`, which also covers `@dataset` rows bound by `scan_one` and `bind_row`. pb: the new `UnmarshalOptions { validator, limits }` and its `unmarshal`. sbe: `Codec::with_validator`, which `View` ignores. The validator receives the decoded value as `&dyn Any`.
  - **How a refusal surfaces:** with the validator's text, `check: port: port out of range (test.rule)`, and no position. The `check::Error` holding the report, or the validator's own error, is the error's `source()`; `PxfError::check_error` and `SbeError::check_error` downcast it. pb returns `Error::Check` or `Error::Validator`. A clean report is `Presence::report`. Unlike protowire-go, a refused `unmarshal_full` does not also return the message and its presence.
  - **Tests:** protowire-go's `check_test.go`, the validator tests of `encoding/pxf`, `encoding/pb` and `encoding/sbe`, and all of `dataset_scan_options_test.go`. SBE's stream-decoder case waits for the stream decoder (#128).
  - **Breaking:** `PxfError` and `SbeError` have a private field, so a struct literal of either no longer compiles (use `new`). Two such errors are equal when their positions and texts are, as before. pb's `Error` has two more variants.
- **`UnmarshalOptions::on_secret_field`** ([#127](https://github.com/trendvidia/protowire-rust/issues/127), protowire-go's `OnSecretField`). A hook that receives each `pxf.Secret` value with its field's dotted path, so the secret never lands in the decoded message.
  - **Where it fires:** for every form a Secret takes. That covers `pw = "x"`, list elements, map values and the block form `pw { value = "x" hint = "h" }`, wherever the Secret sits, `Any` bodies included.
  - **Paths:** they read as protowire-go's do: `db.password`, `servers[0].password`, `by_host["a"]`, `by_id[7]`.
  - **What is stored:** the Secret's `value` is left unset, though presence reports it set. `hint` and `fingerprint` are stored as before.
  - **Errors:** a hook error aborts the decode as `pxf.Secret hook for "<path>": <error>`.
  - **Shape:** a Secret of the wrong shape (#120) is an ordinary message, and the hook never fires for it.
  - **Tests:** `tests/api_oracle.rs` and `tests/wkt_shape_oracle.rs` hold it to protowire-go on 178 rows. Rows with invalid UTF-8 in a Secret match protowire-go's text since [#170](https://github.com/trendvidia/protowire-rust/issues/170).
- **`format_value` and `append_value`** ([#127](https://github.com/trendvidia/protowire-rust/issues/127), protowire-go's `FormatValue`, `AppendValue`). They give one value's PXF source text, as `format` writes it inside a document. A tool that splices a rendered value into a buffer no longer has to render it itself. `tests/api_oracle.rs` holds them to protowire-go on 10 documents.
- **The pxf API for layered configuration** ([#127](https://github.com/trendvidia/protowire-rust/issues/127), protowire-go's `SkipPostDecode`, `ApplyDefault`, `Result.PresentFields`). A layered-config caller merges several documents, and the defaults and required checks belong to the merged result, not to one layer. These pieces let it run them there:
  - `UnmarshalOptions::skip_post_decode` decodes without applying `(pxf.default)` or checking `(pxf.required)`. This holds at the root, in list elements and map values, and in `Any` bodies. Presence and the root's `_null` mask are still recorded.
  - `Presence::present_fields` gives every path the document set, to a value or to `null`.
  - `apply_default` applies one default literal to a field, with the post-decode pass's rules.
  - `crates/protowire-pxf/tests/api_oracle.rs` holds them, and `is_required` and `get_default`, to protowire-go on its `layered_config_test.go` cases and more (`scripts/gen-api-oracle.sh`).
  - **Breaking:** `UnmarshalOptions` has a new public field, so a struct literal without `..Default::default()` no longer compiles.
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

- **`parse` documents that it refuses a string that is not UTF-8** ([#191](https://github.com/trendvidia/protowire-rust/issues/191), decided by the owner on 2026-11-14). A quoted string whose escapes give bytes that are not UTF-8, such as `"\xff"`, is refused by `parse` and is an error and a placeholder in `parse_tolerant`. protowire-go's parser keeps the bytes, but this port's AST holds `String`, which cannot. The difference is now documented on both functions and is permanent. A decode sees none: a quoted string binds only where text is read, since a `bytes` field takes `b"…"`, and there both ports refuse it with protowire-go's error (#170). Only parse-only and format-only use differs, and a `@dataset` row kept as parsed values. No behaviour changes. Tests: the pins in `tests/tolerant_oracle.rs` and `tests/api_oracle.rs` now record a decided difference. The decision rests on protowire-go refusing a quoted string on a `bytes` field, and the spec's wording on that is trendvidia/protowire#613.
- **A required field rejects `null`, as it rejects absence** ([#106](https://github.com/trendvidia/protowire-rust/issues/106), protowire-go#268, trendvidia/protowire#569). A field with `(pxf.required)` or `@required` holds a value, so `r = null` on an `optional int32 r [(pxf.required) = true]` now fails to decode, where it used to leave the field unset. The rule holds whatever the field's type and presence, in a body, a nested body, a `@dataset` cell and a dotted cell, as decided by the owner on 2026-11-04. The error is protowire-go's, `null is not allowed on required field "r": a required field holds a value`. A zero value still satisfies the field. This narrows what decodes.
  - **One rule:** the decoder and the writers now ask the same `nullable`, so the `_null` mask and the `@dataset` writer follow it too. The writers already did; their private copy of the rule is gone.
  - **Tests:** `tests/required_null.rs` ports protowire-go's `TestRequiredRejectsNull` over a new reference-compiled schema, `testdata/carrier/required_null.proto`, with `@required` both known and unknown. `testdata/nulls/` and `testdata/dataset/` are vendored again from the spec at e653718: `required-optional.pxf` becomes `err-required-optional.pxf`, and `err-required-wrapper.pxf` and `dataset/null-on-required-cell.pxf` are new. The presence oracle is regenerated at protowire-go v1.17.0, and its one changed row is this rule.
- **SBE finds a template by schema id and template id together** ([#121](https://github.com/trendvidia/protowire-rust/issues/121), protowire-go#253, protowire-go#257). A template id is unique only within a schema (draft -01 § SBE Annotations), but the codec keyed templates by template id alone. Two messages of one schema with the same id are now refused when the codec is built, naming both; before, the second silently replaced the first. `Codec::view` looks a header up by its schema id and template id, so two schemas in one codec can each use an id. `unmarshal` refuses a header whose schema id is not the target message's. The texts are protowire-go's. **Breaking:** `Codec::template_by_id` takes `(schema_id, template_id)`, and `Codec::by_id` is keyed by that pair.

- **`pxf::marshal` and `pb::marshal` refuse a message nested past `MaxNestingDepth`** ([#86](https://github.com/trendvidia/protowire-rust/issues/86), protowire#502, draft -01 § Recursion). They wrote a message of any depth, and a reader at the default limit then refused the document. Depth is counted as each decoder counts it: in PXF every `{` or `[` is one descent, in PB every submessage or map entry. `MarshalOptions::max_nesting_depth` (both crates) raises the limit for a writer paired with a raised reader. An Any whose body would be too deep is written as `type_url` and `value` instead of being refused. **Breaking:** `pb::marshal` now returns `Result<Vec<u8>>`; `pb::marshal_with` takes the options, and `Writer::finish_checked` reports a refused value for a hand-driven `Writer`.

- **A named keyed entry can't undo its entry name** ([#82](https://github.com/trendvidia/protowire-rust/issues/82), protowire#480). The entry name binds the key field, so inside a named entry `null` on the key field, and any binding of another member of the key field's oneof, are now decode errors. The sibling case used to replace the key without a word. The anonymous form and an agreeing `name = "alpha"` stay legal.

- **`(pxf.default)` and `(pxf.required)` apply in every message body** ([#80](https://github.com/trendvidia/protowire-rust/issues/80), protowire-go#161, protowire#352). `unmarshal_full` used to stop at repeated and map fields, so annotations inside a list element, keyed entry or map value did nothing. They apply there now, at any depth, and in `@dataset` cells through `bind_row`. A document with a required field missing from an element, which used to decode, is rejected with the element's path (`required field "kr[0].name" is absent`), and an unset field in an element takes its default.

### Fixed

- **`@type` takes a type name, not a string** ([#193](https://github.com/trendvidia/protowire-rust/issues/193)). `unmarshal` and `unmarshal_full` accepted a quoted name in the header (`@type "acme.v1.Config"`), which the grammar does not (`type-directive = %s"@type" 1*WSP identifier`, draft -01 § The "@type" Directive), `parse` refuses, and protowire-go refuses. It is now refused as protowire-go refuses it: `expected type name after @type, got string ("acme.v1.Config")`. Tests: the row of `tests/api_oracle.rs` pinned to #193 is now an exact match.
- **A string that is not UTF-8 is refused where it stands, with protowire-go's text** ([#170](https://github.com/trendvidia/protowire-rust/issues/170)). A `\xHH` escape can write any byte, so `"\xff"` is a string literal holding bytes that are not UTF-8. This port's lexer refused every such string with `string contains invalid UTF-8`, wherever it stood. The decoder now says what the string was for, as protowire-go does: `invalid UTF-8 in string field "name" (use b"…" for raw bytes)`, `invalid UTF-8 in string map key`, `invalid UTF-8 in entry name for keyed field "kn"`, `invalid UTF-8 in pxf.Secret value for field "db_password"`. A string where a message or number belongs is quoted as protowire-go quotes it (`got string ("\xff")`). Where protowire-go never reads the string, it is now taken: in a value `discard_unknown` drops, a directive's body, and an `@proto` source body. `parse` and `parse_tolerant` still refuse such a string, since a parsed string holds UTF-8 ([#191](https://github.com/trendvidia/protowire-rust/issues/191)). No public type changes. Tests: 68 rows of `tests/api_oracle.rs` against protowire-go v1.17.0, including a `discard` op; the eight Secret rows that compared only their position now compare in full. With the old lexer and decoder, 68 rows differ.
- **A type error in a value names the field the document wrote** (protowire-go#85). A well-known wrapper's shorthand is typed by the wrapper's inner `value` field, and a map value by the map entry's. So `nullable_int = "x"` and `string_map = { "k": 5 }` were refused as `expected integer for field "value"`, a name the document never wrote. Each now names the field written (`"nullable_int"`, `"string_map"`), as protowire-go does; the block form `nullable_int { value = "x" }` still names `value`. A list element or map value of message type now lists the literal forms its type takes, as a singular field does: `expected a timestamp literal or '{' for map message value`. Tests: 21 rows of `tests/api_oracle.rs`; with the old decoder 11 differ.
- **A document at the depth limit no longer overflows a debug build's stack** ([#179](https://github.com/trendvidia/protowire-rust/issues/179)). Without optimization, the decoder and parser needed up to 2368 KiB of stack for a document nested to `MAX_NESTING_DEPTH`, against the 2 MiB Rust gives a thread. So `payload = { @type = … }` nested 100 deep, which the reader accepts, aborted the process. The worst case was an Any assigned with `=`, and `unmarshal`, `bind_row` and the `@dataset` reader all hit it. The cause was the frame of each recursive function, which in a debug build holds every error message and temporary the function can build. Each recursive reader now keeps only its recursion, and its checks run in helpers that have returned before it descends. The worst document needs 512 KiB in debug (macOS aarch64), and 296 KiB in release, down from 392. In an optimized build the helpers are inlined back. Measured with criterion, `unmarshal` is about 1% slower than before. Tests: `tests/stack_margin.rs` reads and writes the deepest document of every nesting shape, by every reader and writer, each on a thread with 1 MiB of stack. Against the old decoder or the old parser it aborts.
- **`format` writes a map entry built in code so that it parses.** A `MapEntry` whose `quoted` is false had its key written bare, whatever it was, so an empty key or `my key` gave a document that does not parse, and `null` or `1.5` one that reads as another key. As protowire-go does, such a key is now written bare only when it lexes as one identifier, integer or bool, and quoted otherwise. A parsed document formats as before. Tests: protowire-go's `TestLexesAsBareMapKey` and `TestMapEntryBuiltInCodeSpelling`.
- **The `_null` mask lists its paths in document order, as protowire-go's** ([#174](https://github.com/trendvidia/protowire-rust/issues/174)). `unmarshal_full`, `scan_one` and `bind_row` sorted the paths of the root's `_null` FieldMask, where protowire-go keeps them in the order the document holds the nulls. The nulls were the same, but the message's pb bytes differed from protowire-go's whenever two nulls were out of alphabetical order. `Presence::null_paths` now gives the paths in that order too. Tests: two presence-oracle documents with nulls out of order, top-level and nested, and the `@dataset` read-back case of `tests/dataset_write.rs`, whose allowance for this order is removed.
- **`marshal` writes a `_null`-masked field that cannot hold `null` as its value** (protowire-go#151, found on [#129](https://github.com/trendvidia/protowire-rust/issues/129)). A path in the message's `_null` mask was written `null` whatever its field. On a scalar without presence, a repeated or map field, or a `(pxf.required)` field, that is text a reader refuses. Such a field is now written as if the mask did not name it, as protowire-go writes it. An unset required field is still refused.
- **`marshal` no longer drops a list element's or map value's field numbered like the root's `_null` mask** ([#173](https://github.com/trendvidia/protowire-rust/issues/173)). With the root's mask unset, a field of an element or value body whose number was the `_null` field's was left out, so the text read back as a different message. Only the root's own `_null` field is skipped now, as protowire-go skips it. `tests/api_oracle.rs` holds a list element, a map value, a sub-message and two `Any` bodies to protowire-go's text.
- **A schema violation from a `@dataset` row keeps no row position** ([#126](https://github.com/trendvidia/protowire-rust/issues/126)). `scan_one` and `bind_row` printed `2:1: PXF schema violations: …`; protowire-go prints it without the position, as it does every error that is not a decode error. A validator's refusal now passes through the same way.
- **A string map key in a presence path is quoted as Go quotes it** ([#127](https://github.com/trendvidia/protowire-rust/issues/127)). `Presence` paths and map-key errors spelled the key with Rust's `{:?}`, which differs from Go's `strconv.Quote` for control and non-printable characters; they now use the same quoting as protowire-go.
- **Strings are quoted as protowire-go quotes them** ([#127](https://github.com/trendvidia/protowire-rust/issues/127)). protowire-go's formatter writes every string, key, entry name and type URL with Go's `%q`, and its marshal quotes a map key that is not an identifier with `strconv.Quote`. This port escaped only `\\`, `"`, `\n`, `\r` and `\t` in the formatter, and used the byte-wise value writer for map keys. So `\a`, `\b`, `\f`, `\v`, DEL, a no-break space or a zero-width space came out differently. Both now follow `strconv.Quote`. Marshal's string values are unchanged; they already match protowire-go's own writer.
- **A default literal and a bytes literal are read by protowire-go's parsers** ([#167](https://github.com/trendvidia/protowire-rust/issues/167)). protowire-go reads a `(pxf.default)`, and an `apply_default` literal, with Go's `strconv`, `encoding/base64` and `time`, and a document's `b"…"` with the same base64 decoder. Its errors wrap theirs. This port used readers of its own, which accepted different literals, and stopped its errors at the field name. The new `go_strconv` module ports those parsers, following protowire-cpp's `detail/go_strconv` and protowire-java's `Base64Literal`.
  - **Accepted now:**
    - a bytes literal in the URL-safe alphabet (`b"aGk-"`), in a document or a default;
    - a hex float (`0x1p-2`) or one with underscores (`1_000.5`) as a float default;
    - a one-digit hour or a comma fraction in a timestamp default.
  - **Refused now:** in a default, `+5` for an unsigned field, and over-padded base64 (`aGVsbG8==`, `=`).
  - **Errors:** each now ends with Go's own text, for example `…: strconv.ParseInt: parsing "1_000": invalid syntax`, `…: illegal base64 data at input byte 2`, or `…: parsing time "2026-05-09T24:00:00Z": hour out of range`. A `@proto b"…"` body that does not decode now says where, as protowire-go does.
  - **Tests:** `tests/api_oracle.rs` gains 96 cases, all held to protowire-go. One row, a float default of `-0`, waits on [#98](https://github.com/trendvidia/protowire-rust/issues/98), the same prost-reflect divergence as the spec's `Scalars.float-negzero`.
- **A default literal is read as protowire-go reads it** ([#127](https://github.com/trendvidia/protowire-rust/issues/127)). These came to light once `apply_default` could be held to protowire-go's `ApplyDefault`.
  - A bool default other than `true` or `false` was read as `false`; it is now an error, with protowire-go's text.
  - A wrapper's default error names the field holding the wrapper, not its inner `value`.
  - A big-number default error now names its type: `invalid default big integer "x" for field "bi": …`.
  - A Timestamp, Duration, wrapper, big-number or Secret literal on a singular field now marks its inner fields present (`t.seconds`, `bi.abs`), as protowire-go's `markInnerPresent` does.
- **`protowire-pb` writes a NaN as the canonical quiet NaN** ([#125](https://github.com/trendvidia/protowire-rust/issues/125), protowire-go#241, draft -01 § Numeric Literals). `Writer::double` and `Writer::float` wrote a NaN's bits as given, so a struct holding Go's `math.NaN()` (`0x7FF8000000000001`), a negative NaN or a signalling one went out with that payload. protowire-go writes every NaN in a struct field as `0x7FF8000000000000` (double) or `0x7FC00000` (float), and so does this port now, in singular, packed and map-value positions. `Writer::fixed64` and `Writer::fixed32` still write bits as given. `tests/nan.rs` holds the bytes to protowire-go's, for four NaNs.
- **A descriptor compiled before protowire v1.12.0 is diagnosed, not bound silently** ([#133](https://github.com/trendvidia/protowire-rust/issues/133), STABILITY.md promise 3, draft -01). Such a descriptor carries its options at the extension numbers retired in v1.12.0 (50000 and up). This port reads only the registered numbers, so the schema bound without a word, and its `(pxf.required)` and `(pxf.default)` did nothing.
  - **pxf:** bind-time validation now reports each retired number as `ViolationKind::RetiredNumber`, with protowire-go's text: what the number was, what it is now, and which file to recompile against. As in protowire-go, a number counts only on the options kind it was allocated on, and only in a file that imports one of protowire's annotation files.
  - **SBE:** `Codec::from_files` and `proto_to_xml` report a `(sbe.schema_id)` at 50100, or a `(sbe.template_id)` at 50200, as predating the registered block. Before, they said the option was missing, from a `.proto` that declares it.
  - **Tests:** `crates/protowire/tests/retired_oracle.rs` holds 16 descriptors to what protowire-go does with each (`scripts/gen-retired-oracle.sh`). Before this change, 27 of its 64 results differed.
  - **Breaking:** `ViolationKind` has a new variant, `RetiredNumber`.
- **PXF checks a well-known type's shape, not only its name** ([#120](https://github.com/trendvidia/protowire-rust/issues/120), protowire-go#224, draft -01 §wkt-shapes). A schema could declare a message named like a well-known type, say `google.protobuf.Timestamp`, without the fields the codec reads. Decode and marshal read those fields by name anyway. A literal such as `pw = 5` on a `pxf.BigInt` with no `abs` was accepted, and the value dropped. A field of the wrong type, or a missing one on some paths, made Rust panic. On protowire-go's cases, 381 of 642 results differed from Go's, and 42 of those were panics.
  - **Validation:** such a message is now reported at bind time as `ViolationKind::WellKnownShape`, with protowire-go's text, naming the field that is missing, mistyped or of the wrong cardinality. This covers Timestamp, Duration, Any, FieldMask, the nine wrappers, `pxf.Secret`, `pxf.BigInt`, `pxf.Decimal` and `pxf.BigFloat`.
  - **Without validation:** with `skip_validate`, and on marshal, the message is an ordinary one. It is read and written as a block, has no literal form, and is not used as a `_null` mask.
  - **Tests:** `tests/wkt_shape_oracle.rs` holds 45 schemas to what protowire-go does with each, in four decode modes and on marshal (`scripts/gen-wkt-shape-oracle.sh`).
  - **Breaking:** `ViolationKind` has a new variant, `WellKnownShape`.
- **The SBE XML converter keeps fixed-length arrays that are not text** ([#104](https://github.com/trendvidia/protowire-rust/issues/104), protowire-go#249, draft -01 § Fixed-Length Arrays). Import read `<type primitiveType="uint8" length="16"/>` as one `uint32`, so the field took 1 byte of its 16 and every later field in the block was misplaced. Export wrote a `bytes` field as a `char` array, which imported back as a `string`.
  - **Import:** a `uint8` or `int8` array of length N > 1 now becomes `bytes` with `(sbe.length) = N`. An array of a wider primitive is refused, naming the message or composite and the field.
  - **Export:** a `bytes` field is now a `uint8` array (`bytesN`). A schema with such arrays survives XML → proto → XML → proto.
  - **Tests:** `tests/convert_as_go.rs` holds the output to protowire-go's converter byte for byte (`scripts/gen-sbe-convert.sh`).
  - **Breaking:** `xml_to_proto` returns `SbeError`; a parse error keeps the parser's text.
- **SBE binds a proto3 `optional` field** ([#160](https://github.com/trendvidia/protowire-rust/issues/160)). The codec refused it as a oneof (`sbe: oneof field M.x not supported`), because proto3 puts an `optional` field in a synthetic oneof of its own. protowire-go skips synthetic oneofs, and so does this port now, at the root and in a composite. A real oneof is still refused.

- **SBE refuses a value, size or header field the wire cannot hold** ([#122](https://github.com/trendvidia/protowire-rust/issues/122), protowire-go#250, protowire-go#252). These used to be cut to their low bits with no error.
  - **Marshal** refuses a value outside its `(sbe.encoding)` primitive's range (an `int32` field narrowed to `int8` holding 300 was written as 44), a double past float's range narrowed to `float`, and a group of more than 65535 entries (65536 were written with a count of 0).
  - **Unmarshal** refuses a wire value its field cannot hold, such as a `uint32` primitive past `int32`'s range.
  - **Building the codec** refuses a `schema_id`, `version`, `template_id` or `(sbe.length)` past 65535, a block that ends past byte 65535, and an `(sbe.encoding)` that crosses between integers and floating point or sits on a string or bytes field. These used to bind.
  - **A `double` field with `(sbe.encoding) = "float"`, or a `float` field with `"double"`, reads back.** Unmarshal stored the primitive's type in the field, and prost-reflect panicked.
  - The texts are protowire-go's, and `tests/bounds_oracle.rs` holds 204 cases against what protowire-go v1.17.0 does with them (`scripts/gen-sbe-oracle.sh`). `field_encoding_size`'s error is now the reason alone; the codec names the field.

- **A `pxf.BigFloat` rounded past `MaxBigFloatExponent` is refused, not written** ([#111](https://github.com/trendvidia/protowire-rust/issues/111), protowire-go#261). `marshal` and an `@default` carrier checked the exponent as stored, before the mantissa is rounded to `prec`. Rounding can carry: 2^54 - 1 at exponent 16330 is at the bound (16384) as stored, and 2^54, exponent 16385, at prec 3. The port wrote `1.2e+4932p3`, which no reader takes. The draft says the producer fails, and now it does, with protowire-go's text: `pxf.BigFloat binary exponent 16385 after rounding to prec 3 exceeds MaxBigFloatExponent=16384`. For an `@default`, it fails with `@default's big_float_value has binary exponent 16385 after rounding to prec 3, …`. The check computes the rounded exponent without building a `prec`-bit mantissa. `pb::marshal` already refused this value.
  - **Tests:** `tests/bigfloat_render_bound.rs` (the spec's value, and the one below it, which is written and reads back) and a carrier unit test.
- **A `pxf.BigFloat` literal is read as protowire-go reads it** ([#108](https://github.com/trendvidia/protowire-rust/issues/108), [#109](https://github.com/trendvidia/protowire-rust/issues/109), [#124](https://github.com/trendvidia/protowire-rust/issues/124)). The port copied `big.Float.Parse` step by step and took its result as the value. protowire-go now uses `Parse` only to place a literal against the range checks, and so does this port.
  - **Rounded once (#108, protowire-go#260):** the value is the literal's exact value rounded once to its precision, ties to even. `Parse` scales by powers of ten at a working precision and rounds again, so a literal within a hair of a tie came out wrong. The spec's write-gate literal `4.2026…e-4932p2` was mantissa 3 and is now 2.
  - **Checks capped at 16384 bits (#109, protowire-go#274):** `Parse` now runs at no more than 16384 bits. Under a raised `MaxNumericLiteralDigits`, `1e-300p2147483647` took more than two minutes, and is now refused at once. A capped positive exponent is allowed one more before the first `MaxBigFloatExponent` check, and the check after the exact rounding decides.
  - **Precision ceiling (#124):** a non-zero value with a precision suffix past 2^31 - 1 bits is refused, `precision p2147483648 exceeds the widest mantissa big.Float can hold, 2147483647 bits`. A zero keeps such a suffix, as in Go.
  - **Tests:** `tests/bigfloat_as_go.rs` holds 100 literals to what protowire-go v1.17.0 gave for each, under the default limits and with `MaxNumericLiteralDigits` raised to 1.3 billion. Each must finish within 5 seconds.
- **A duration literal is read as protowire-go reads it** ([#153](https://github.com/trendvidia/protowire-rust/issues/153)). protowire-go validates and values a duration with Go's `time.ParseDuration`, and `src/go_duration.rs` is now a line-for-line port of it, used by the lexer and the decoder.
  - **Now accepted:** a segment's fraction with no digit before it (`1h.5m`, `5m.5s`, `2.5h.25m`). It was refused as `invalid duration`.
  - **Long fractions:** a long fraction of an hour rounds as Go's float64 arithmetic rounds it. `0.333333333333333333333h` is 1200 s, where it was 1199.999999999 s.
  - **Overflow:** a literal past the signed 64-bit count of nanoseconds is refused by the lexer, as `invalid duration: <literal>`, as Go refuses it. It was the decoder's `out of range` error.
  - **Tests:** `tests/durations.rs` holds 38 literals to what protowire-go v1.17.0 gave for each, its bytes or its error. The `1h.5m` stream parity case is unpinned.
- **`DatasetReader` reads as `parse` does, however the input is cut into reads** ([#117](https://github.com/trendvidia/protowire-rust/issues/117)). It found rows with its own byte scanner, so errors were its own: `1:1` or no position for bad rows on lines 2 to 4, and its own wording. One-byte reads refused a `//` comment between rows. A row that never closed took time growing with the square of its size (1.21 s for 4 MiB). It now finds rows with the lexer `parse` uses, resuming where the last read left it, and parses each row with `parse`'s parser at its document position, as protowire-go's reader does.
  - **Every row and error is `parse`'s.** The reader yields exactly `parse`'s rows, with their positions and their comments in `DatasetRow::leading_comments`, or fails with exactly `parse`'s error. That holds for whole reads, one-byte reads, and a read split at every offset.
  - **Linear time.** A header or row costs time linear in its size however many reads it arrives in. A complete row, or an error the bytes read already decide, is returned without waiting for more input.
  - **Faster:** a 10 MB table reads at 150 MB/s, up from 124 MB/s.
  - **New API:** `tail` returns a `DatasetTail` (still a `Read`). `DatasetReader::from_tail` reads the next table from it with document positions, and with the comments before its directives, as `parse` files them. A reader built on it with `new` counts positions from the tail's start.
  - **`parse` positions:** an entry beside a `@dataset` is reported at the entry, `@type` beside a `@dataset` at the first `@dataset`, and an `@proto` body that never closes at its `{`, as protowire-go reports them. Brace matching for directive and `@proto` bodies is protowire-go's `braceMatch`.
  - **Error texts:** a read failure is the I/O error's own text, and a buffer past `max_message_size` reads `dataset input holds N bytes without a row boundary, MaxMessageSize=M`, as in Go.
  - **Tests:** `tests/dataset_stream_parity.rs` ports protowire-go v1.17.0's `dataset_stream_parity_test.go`, with its fuzz seeds and the row-comment case of `format_comments_test.go`. The reader's unit tests port `dataset_scan_linear_test.go`, which counts the bytes the scan lexes. A row with `1h.5m` is pinned to #153, a duration Go accepts and `parse` refuses.
- **An illegal token is named as protowire-go names it** ([#149](https://github.com/trendvidia/protowire-rust/issues/149)). A character no token starts with was named by itself, so `$ = 1` read `got $` and a control character printed raw. It is now `unexpected character '$'`, with the character written as Go's `%q` writes it: escaped (`'\x00'`, `'\u00a0'`, `'\U000e0001'`) wherever Go's `strconv.IsPrint` refuses it. The new `src/go_print.rs` table holds that set, generated from Go by `scripts/gen-go-print-ranges.sh`. A bare `@`, `+` or `-`, and a raw newline in a string, now carry Go's sentences (`"+" is valid only in "+inf"`). At a value, `parse` reports the lexer's text alone, as Go's `parseValue` does. Before, it gave `expected value, got ILLEGAL ("+")`.
  - **Tests:** `tests/illegal_tokens.rs` holds 55 documents to protowire-go v1.17.0's `Parse` text, and `tests/entry_keys.rs` takes back its two `$` rows.
- **`parse` keeps every comment, and `format` writes it back where it was** ([#119](https://github.com/trendvidia/protowire-rust/issues/119), protowire-go#279). `format` used to lose 13 of protowire-go's 14 comment test documents. A comment was kept only if an entry followed it, so one at the end of a document or block, or around `@dataset` rows, was dropped or moved onto the next entry. A comment after a value on its line was dropped too. They now land as protowire-go's parser files them:
  - **Same line:** after a value, in `Assignment`, `MapEntry` and `TypeEntry` `trailing_comment`.
  - **Before a `}`:** in `Block` and `BlockVal` `trailing_comments`.
  - **At the end of the document:** in `Document::trailing_comments`.
  - **Before or inside a dataset row:** in `DatasetRow::leading_comments`.
  - **`canonicalize_keyed`** keeps a block's closing comments when it rewrites the block, and the comments of a key assignment it drops.
  - **Breaking:** `Document`, `Block`, `BlockVal`, `DatasetRow` and `TypeEntry` have new public fields, so code that builds them with struct literals must set them.
  - **Tests:** `tests/format_comments.rs` ports Go's `format_comments_test.go` (all but its `DatasetReader` case, which #117 covers) and holds six more same-line documents to Go's output.
- **An entry's key is read as protowire-go reads it** ([#123](https://github.com/trendvidia/protowire-rust/issues/123), protowire-go#197). Under `discard_unknown`, `1 = 2` and `1 { }` were skipped as unknown fields. Both readers now refuse an integer or bool key before `=` or `{` whatever the option, with the parser's error: `field assignment with '=' requires an identifier or string key, got integer ("1"); use ':' for map entries`. **Behaviour change:** such a document is now an error with `discard_unknown` on.
  - **Bool keys:** `true = 1` and `true { }` get the same two errors. The decoders used to leave bool out of the key set.
  - **Precision suffix:** a suffix on a key is checked only on a map key's `:` form, as Go checks it. Elsewhere it is written into the error as part of the key (`got integer ("1p5")`). A long suffix is shortened as Go shortens it (`p123456789012345678901234…`).
  - **`:` at the top level:** the decoders give the parser's `map entry (':' form) is only allowed inside a '{ … }' block` error.
  - **Error texts:** a wrong token at a key, a map key, a keyed entry name or a type entry now names the token as Go does, `got [ ("[")`, or the lexer's message for an illegal token (`got unterminated string`).
  - **Tests:** `tests/entry_keys.rs` holds 60 documents to what protowire-go v1.17.0 gave from `Unmarshal` and `UnmarshalFull`, with the option off and on, and from `Parse`. The lexer's own texts for an illegal token such as `$` still differ (#149).
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
