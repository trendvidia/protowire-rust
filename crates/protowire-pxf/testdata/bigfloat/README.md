# pxf.BigFloat text-form fixtures

Cross-port conformance fixtures for how a `pxf.BigFloat` is written as PXF
text and how a literal is read back ([issue #406](https://github.com/trendvidia/protowire/issues/406),
decided by the owner; [issue #393](https://github.com/trendvidia/protowire/issues/393);
IETF draft `-01` § PXF Annotations).
- **Writing:** N = (decimal digits of 2^prec) + 1 significant digits, round half even, in C's `%g` form, followed by the precision suffix `p<prec>`. A mantissa wider than prec is the value rounded to prec. A zero is `0p<prec>` or `-0p<prec>`; only a prec-0 zero is `0`.
- **Reading:** a suffixed literal is read at exactly the suffix's precision, an unsuffixed one at 256 bits, round half even. The stored mantissa is the significand as a prec-bit integer, and the exponent is the binary exponent minus prec.
- **Zero** is stored with an empty mantissa and exponent 0, prec as its suffix states (0 without one), and `negative` for a leading `-`.
- **A suffix elsewhere,** on a field that isn't a `pxf.BigFloat`, is rejected, naming the field, and so is `p0` and a precision whose N would exceed `MaxNumericLiteralDigits` (above `p13603` at the default).
- **Limits:** `MaxBigFloatExponent` is checked after rounding to the read precision, and `MaxNumericLiteralDigits` counts mantissa digits only (both decided on [#410](https://github.com/trendvidia/protowire/issues/410)).

Every case binds to [`bigfloat.proto`](bigfloat.proto) (`bf.v1.H`: `pxf.BigFloat bf`, `int32 count`).
- An `enc-<name>` case is a PB message (`.binpb`) and the PXF text an encoder MUST write for it (`.pxf`).
- A `dec-<name>` case is a PXF document (`.pxf`) and the PB bytes a decoder MUST produce (`.binpb`, deterministic field order, proto3 zero fields omitted).
- An `err-<name>` case is a document a decoder MUST reject.

[`generate.py`](generate.py) computes every expected value with exact rational arithmetic, from the rule alone, independently of any port. Edit it, not the outputs, and commit both; it is idempotent. `cmd/pxf/bigfloat_test.go` drives the corpus through protowire-go.

| Case | Value or literal | Asserts |
|---|---|---|
| `enc-2pow1024-prec64` | 2^1024, prec 64 | `1.79769313486231590773e+308p64`: 21 digits |
| `enc-tenth-as-double` | the double nearest 0.1, prec 53 | `0.10000000000000001p53`: 17 digits. 16 would read back as a different value. |
| `enc-long-mantissa` | (2^100 − 1)·2^−50, prec 100, a 13-byte mantissa | 32 digits, positional |
| `enc-three-quarters-negative` | −0.75 (`negative = true`) | `-0.75p64`, trailing zeros removed |
| `enc-one-and-a-half` | 1.5, prec 53 | `1.5p53` |
| `enc-integer` | 1000, prec 64 | `1000p64`, positional below N digits |
| `enc-tiny-negative-exponent` | 2^−16385, at the `MaxBigFloatExponent` bound | exponent form |
| `enc-prec-zero` | prec 0 | `0`, no suffix |
| `dec-tenth-p53` | `0.10000000000000001p53` | read at 53 bits: the double nearest 0.1 again |
| `dec-2pow1024-p64` | `1.79769313486231590773e+308p64` | exactly 2^1024 at 64 bits |
| `dec-hundred-digits-p333` | a 100-digit literal with `p333` | read at the suffix's 333 bits |
| `dec-hundred-digits-unsuffixed` | the same literal without a suffix | read at 256 bits |
| `dec-tenth-unsuffixed` | `0.1` | read at 256 bits |
| `dec-minus-two-and-a-half` | `-2.5` | `negative` set |
| `dec-1e4932` | `1e4932` | binary exponent 16384, the bound |
| `dec-small-positional` | `0.000125` | |
| `dec-zero` | `0` | every field 0 |
| `dec-negative-zero` | `-0` | only `negative` set |
| `dec-integer-p64` | `1000p64` | a suffix on the integer form |
| `dec-exponent-only-p24` | `1e-5p24` | a suffix on the exponent-only form |
| `dec-rounds-up-to-power-p8` | `0.999999p8` | rounds up past the 8-bit significand, to 1 |
| `dec-one-at-max-precision` | `1p13603` | the largest precision within `MaxNumericLiteralDigits` |
| `err-suffix-on-int32` | `count = 5p64` | a suffix on a field that isn't a BigFloat; the error names `count` |
| `err-precision-zero` | `bf = 1.5p0` | a zero precision |
| `err-precision-past-limit` | `bf = 1p13604` | one past the largest precision |
| `err-precision-overflow` | `bf = 1p99999999999999999999999` | a precision past every fixed-width integer |
| `enc-zero-prec64` | zero at prec 64 | `0p64` (#410) |
| `enc-zero-prec64-negative` | −0 at prec 64 | `-0p64` |
| `enc-wide-mantissa-prec53` | (2^100 − 1)·2^−50 at prec 53, a mantissa wider than prec | `1125899906842624p53`: the value rounded to 53 bits |
| `enc-exponent-bound-prec256` | 2^−16385 at the 256-bit default | its literal lies just past the bound and still reads back |
| `enc-max-precision-with-exponent` | (2^13603 − 1)·2^2781, binary exponent 16384 | 4095 significant digits plus exponent and suffix: only the mantissa digits count |
| `dec-zero-p64` | `0p64` | prec 64 kept |
| `dec-negative-zero-p64` | `-0p64` | prec 64 and `negative` kept |
| `dec-exponent-bound-prec256` | `enc-exponent-bound-prec256`'s literal | accepted: the bound is on the rounded value |
| `dec-max-precision-with-exponent` | `enc-max-precision-with-exponent`'s literal | accepted at the digit limit |
