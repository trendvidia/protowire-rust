# Null-binding fixtures

Cross-port conformance fixtures for where a `null` literal may bind
([issue #339](https://github.com/trendvidia/protowire/issues/339); IETF
draft `-01` § Booleans, Null, and Identifier Values). A `null`:

- on a singular field with explicit presence (a message field, wrappers
  included, a proto3 `optional` scalar, a oneof member, a proto2
  `optional` scalar) MUST leave it unset, and it is present-but-null. It
  counts as present for `(pxf.required)`, and `(pxf.default)` is not
  applied;
- on a proto3 scalar declared without `optional` MUST be rejected: the
  field has no unset state, so `null` would read back as its zero value;
- on a proto2 `required` field MUST be rejected: leaving it unset makes
  the message invalid in protobuf binary (#379);
- on a repeated or map field, as an element of a repeated field, or as
  a map value, whatever the value type, MUST be rejected (#380);
- on a oneof member makes it the member the body chose, so a body that
  also writes a sibling MUST be rejected.

The owner settled this on #339 (2026-10-27, option 3), and the proto2
`required` and map-value cases on #379 and #380 (2026-10-28).

Every fixture binds against [`nulls.proto`](nulls.proto) (package
`nulls.v1`, message `Holder` unless the table names another), or
[`nulls_proto2.proto`](nulls_proto2.proto) for the proto2 cases and
[`nulls_editions.proto`](nulls_editions.proto) (`edition = "2023"`) for the editions ones. Each file's leading comment states the
expected behavior, and `cmd/pxf/nulls_test.go` runs them through the
reference implementation: `Parse` (no schema, which accepts them all), a
decoder bound to `nulls.v1.Holder`, and `pxf validate`. The same rule in a
`@dataset` cell is pinned by
[`../dataset/null-on-scalar-cell.pxf`](../dataset/null-on-scalar-cell.pxf)
and
[`../dataset/null-on-optional-cell.pxf`](../dataset/null-on-optional-cell.pxf).

| File | Verdict | Asserts |
|---|---|---|
| `err-scalar.pxf` | reject | `n = null` on a plain `int32`. |
| `err-repeated.pxf` | reject | `l = null` on a `repeated int32`. |
| `err-list-element.pxf` | reject | `l = [1, null]`: a list element is not nullable. |
| `err-map.pxf` | reject | `m = null` on a `map<string, int32>`. |
| `err-map-value.pxf` | reject | `m = { "a": null }`: a map value is not nullable (#380). |
| `err-map-message-value.pxf` | reject | `mm = { "a": null }`, a message-valued map: still not nullable (#380). |
| `err-proto2-required.pxf` | reject | `r = null` on a proto2 `required int32` (`nulls.v1.Strict`, #379). |
| `err-nested-scalar.pxf` | reject | `msg { x = null }`: the rule holds in every message body. |
| `err-oneof-sibling.pxf` | reject | `km { x = 1 }` then `ks = null`: two members of the oneof `k`. |
| `message.pxf` | accept | `msg = null` leaves `msg` unset. |
| `wrapper.pxf` | accept | `w = null` leaves `w` unset. |
| `optional-scalar.pxf` | accept | `o = null` on an `optional int32` leaves `o` unset. |
| `oneof-scalar.pxf` | accept | `ks = null` on a `string` oneof member leaves the oneof `k` unset. |
| `oneof-message.pxf` | accept | `km = null` leaves the oneof `k` unset. |
| `proto2-scalar.pxf` | accept | `p = null` on a proto2 `optional int32` (`Legacy`) leaves `p` unset. |
| `required-optional.pxf` | accept | `r = null` on an `optional int32` with `(pxf.required)` (`Required`): present, and unset. |
| `default-optional.pxf` | accept | `d = null` on an `optional int32` with `(pxf.default) = "5"` (`Defaulted`): unset, not 5. |
| `editions-explicit.pxf` | accept | In an editions file, `null` on a scalar with EXPLICIT presence, the 2023 default (`nulls.ed.v1.Ed`), leaves it unset ([#380](https://github.com/trendvidia/protowire/issues/380)). |
| `err-editions-implicit.pxf` | reject | `null` on an editions scalar with `features.field_presence = IMPLICIT`. |

Every accept fixture leaves no field set, and a full decode reports the
named field present-but-null.

An error names the field. The reference implementation's wording is
`null is not allowed on scalar field "n"`, `null is not allowed on
repeated field "l"`, `null is not allowed in repeated field "l"`, `null is
not allowed on map field "m"` and `oneof "k": field "ks" conflicts with
already-set field "km"`; ports need not match it.
