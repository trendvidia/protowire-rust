// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Arbitrary-precision literal forms: `pxf.BigInt` and `pxf.Decimal`
//! (`proto/pxf/bignum.proto`; draft -01 §well-known types).
//!
//! A `pxf.BigInt` field takes a bare integer literal and a `pxf.Decimal`
//! field a decimal literal preserving its exact scale (`1.00` has
//! scale 2). On the wire both carry an unsigned big-endian magnitude and a
//! sign flag, so the codec needs exactly two conversions — decimal digits
//! to a big-endian magnitude and back — and no arithmetic. They are
//! hand-rolled here rather than pulled from a bignum crate, in the same
//! spirit as the lexer, base64 and RFC 3339 code: ~40 lines each, no
//! semantic drift, no new dependency. Both are quadratic in the digit
//! count, which [`MAX_NUMERIC_LITERAL_DIGITS`] bounds *before* they run.
//!
//! `pxf.BigFloat`'s literal form is not implemented: producing the same
//! mantissa/exponent bytes as the reference for a decimal literal means
//! reproducing `big.Float`'s 256-bit rounding, which is real
//! arbitrary-precision arithmetic; tracked in protowire-rust#39. A
//! `pxf.BigFloat` field still reads and writes its block form.

/// HARDENING.md § Mandatory limits: the digit count of any single PXF
/// numeric literal before it is parsed into a `pxf.BigInt` / `pxf.Decimal`
/// / `pxf.BigFloat`, bounding the quadratic conversions above. A literal
/// of exactly this many digits is within the limit. Also bounds the
/// magnitude of `pxf.Decimal.scale` on the PB wire, since a scale is a
/// digit count and a decoder that materialises the value computes
/// `10^scale` from it.
pub const MAX_NUMERIC_LITERAL_DIGITS: usize = 4096;

/// A parsed `pxf.BigInt` literal: unsigned big-endian magnitude with no
/// leading zero bytes (empty for zero), and the sign. Zero is never
/// negative, as `big.Int.Sign()` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BigIntLit {
    pub abs: Vec<u8>,
    pub negative: bool,
}

/// A parsed `pxf.Decimal` literal: value = (-1)^negative × unscaled ×
/// 10^(−scale). `unscaled` is the big-endian magnitude of the digits with
/// the point removed (empty for zero); `scale` is the number of digits
/// after the point. `negative` is the literal's sign as written, so
/// `-0.0` keeps it — the reference reads the flag from the text too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecimalLit {
    pub unscaled: Vec<u8>,
    pub scale: i32,
    pub negative: bool,
}

/// Enforce `MaxNumericLiteralDigits` on a literal: counts ASCII digits,
/// so the sign and the point do not count, and a literal of exactly
/// `max_digits` digits passes. Mirrors the reference's
/// `checkLiteralDigits`, wording included.
pub fn check_literal_digits(s: &str, max_digits: usize) -> Result<(), String> {
    if s.len() <= max_digits {
        return Ok(());
    }
    let n = s.bytes().filter(u8::is_ascii_digit).count();
    if n > max_digits {
        return Err(format!(
            "numeric literal has {} digits; MaxNumericLiteralDigits={}",
            n, max_digits
        ));
    }
    Ok(())
}

/// Parse a `pxf.BigInt` literal: an optional `-` and one or more decimal
/// digits.
pub fn parse_big_int(s: &str, max_digits: usize) -> Result<BigIntLit, String> {
    check_literal_digits(s, max_digits)?;
    let (negative, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("invalid big integer: {}", s));
    }
    let abs = decimal_digits_to_be_bytes(digits.as_bytes());
    Ok(BigIntLit {
        negative: negative && !abs.is_empty(),
        abs,
    })
}

/// Parse a `pxf.Decimal` literal: an optional `-`, decimal digits, and an
/// optional `.` followed by decimal digits. An exponent is not admitted —
/// `1e5` is a float literal, not a decimal one — and the reference rejects
/// it the same way.
pub fn parse_decimal(s: &str, max_digits: usize) -> Result<DecimalLit, String> {
    check_literal_digits(s, max_digits)?;
    let (negative, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let (int_part, frac_part) = match body.split_once('.') {
        Some((i, f)) => (i, f),
        None => (body, ""),
    };
    let mut digits = String::with_capacity(int_part.len() + frac_part.len());
    digits.push_str(int_part);
    digits.push_str(frac_part);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("invalid decimal: {}", s));
    }
    // frac_part.len() <= max_digits, which fits an i32 for any bound a
    // caller can express in practice; saturate rather than wrap regardless.
    let scale = i32::try_from(frac_part.len()).unwrap_or(i32::MAX);
    Ok(DecimalLit {
        unscaled: decimal_digits_to_be_bytes(digits.as_bytes()),
        scale,
        negative,
    })
}

/// Render a `pxf.BigInt` as its literal: the magnitude in decimal with a
/// leading `-` when negative. An empty magnitude is `0`, and never signed.
pub fn format_big_int(abs: &[u8], negative: bool) -> String {
    let digits = be_bytes_to_decimal(abs);
    if negative && digits != "0" {
        format!("-{}", digits)
    } else {
        digits
    }
}

/// Render a `pxf.Decimal` as its literal, preserving the scale: unscaled
/// 5 with scale 2 is `0.05`, unscaled 100 with scale 2 is `1.00`. A
/// negative scale means trailing zeros (value = unscaled × 10^(−scale)),
/// so they are written out; the reference's formatter drops them, which
/// loses the value, and is tracked against it.
pub fn format_decimal(unscaled: &[u8], scale: i32, negative: bool) -> String {
    let mut digits = be_bytes_to_decimal(unscaled);
    let mut out = String::with_capacity(digits.len() + 2);
    if negative {
        out.push('-');
    }
    if scale <= 0 {
        out.push_str(&digits);
        for _ in 0..scale.unsigned_abs() {
            out.push('0');
        }
        return out;
    }
    let scale = scale as usize;
    // Pad with leading zeros so there is at least one digit before the
    // point (unscaled 5, scale 2 → "0.05").
    while digits.len() <= scale {
        digits.insert(0, '0');
    }
    let (int_part, frac_part) = digits.split_at(digits.len() - scale);
    out.push_str(int_part);
    out.push('.');
    out.push_str(frac_part);
    out
}

/// Convert ASCII decimal digits to a big-endian magnitude with no leading
/// zero bytes; zero is the empty vector, as `big.Int.Bytes()` returns it.
/// Schoolbook: a little-endian base-256 accumulator multiplied by ten and
/// added to, digit by digit. Quadratic in the digit count — bound it
/// first.
pub fn decimal_digits_to_be_bytes(digits: &[u8]) -> Vec<u8> {
    let mut le: Vec<u8> = Vec::with_capacity(digits.len() / 2 + 1);
    for &d in digits {
        let mut carry = u32::from(d - b'0');
        for limb in le.iter_mut() {
            let v = u32::from(*limb) * 10 + carry;
            *limb = (v & 0xff) as u8;
            carry = v >> 8;
        }
        while carry > 0 {
            le.push((carry & 0xff) as u8);
            carry >>= 8;
        }
    }
    le.reverse();
    le
}

/// Convert a big-endian magnitude to its decimal digits; the empty
/// magnitude is `"0"`. Schoolbook long division by ten over the bytes,
/// most significant first, collecting remainders. Quadratic in the
/// byte count — a wire-supplied magnitude is bounded by the message size.
pub fn be_bytes_to_decimal(bytes: &[u8]) -> String {
    let mut mag: Vec<u8> = bytes.iter().copied().skip_while(|&b| b == 0).collect();
    if mag.is_empty() {
        return "0".to_string();
    }
    let mut digits: Vec<u8> = Vec::with_capacity(mag.len() * 5 / 2 + 1);
    while !mag.is_empty() {
        let mut rem: u32 = 0;
        for limb in mag.iter_mut() {
            let v = (rem << 8) | u32::from(*limb);
            *limb = (v / 10) as u8;
            rem = v % 10;
        }
        digits.push(b'0' + rem as u8);
        while mag.first() == Some(&0) {
            mag.remove(0);
        }
    }
    digits.reverse();
    String::from_utf8(digits).expect("ASCII digits")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_to_bytes_known_values() {
        assert_eq!(decimal_digits_to_be_bytes(b"0"), Vec::<u8>::new());
        assert_eq!(decimal_digits_to_be_bytes(b"000"), Vec::<u8>::new());
        assert_eq!(decimal_digits_to_be_bytes(b"1"), vec![1]);
        assert_eq!(decimal_digits_to_be_bytes(b"255"), vec![0xff]);
        assert_eq!(decimal_digits_to_be_bytes(b"256"), vec![1, 0]);
        assert_eq!(decimal_digits_to_be_bytes(b"007"), vec![7]);
        assert_eq!(decimal_digits_to_be_bytes(b"65536"), vec![1, 0, 0]);
        // 2^256 - 1
        assert_eq!(
            decimal_digits_to_be_bytes(
                b"115792089237316195423570985008687907853269984665640564039457584007913129639935"
            ),
            vec![0xff; 32]
        );
        // 2^96 - 1
        assert_eq!(
            decimal_digits_to_be_bytes(b"79228162514264337593543950335"),
            vec![0xff; 12]
        );
    }

    #[test]
    fn bytes_to_digits_known_values() {
        assert_eq!(be_bytes_to_decimal(&[]), "0");
        assert_eq!(be_bytes_to_decimal(&[0, 0]), "0");
        assert_eq!(be_bytes_to_decimal(&[1]), "1");
        assert_eq!(be_bytes_to_decimal(&[0xff]), "255");
        assert_eq!(be_bytes_to_decimal(&[1, 0]), "256");
        assert_eq!(be_bytes_to_decimal(&[0, 7]), "7");
        assert_eq!(
            be_bytes_to_decimal(&[0xff; 32]),
            "115792089237316195423570985008687907853269984665640564039457584007913129639935"
        );
    }

    #[test]
    fn conversions_round_trip_a_4096_digit_literal() {
        let s: String = (0..4096)
            .map(|i| char::from(b'1' + (i % 9) as u8))
            .collect();
        let bytes = decimal_digits_to_be_bytes(s.as_bytes());
        assert_eq!(be_bytes_to_decimal(&bytes), s);
    }

    #[test]
    fn big_int_literals() {
        assert_eq!(
            parse_big_int("42", 4096).unwrap(),
            BigIntLit {
                abs: vec![42],
                negative: false
            }
        );
        assert_eq!(
            parse_big_int("-42", 4096).unwrap(),
            BigIntLit {
                abs: vec![42],
                negative: true
            }
        );
        // Zero is never negative.
        assert_eq!(
            parse_big_int("-0", 4096).unwrap(),
            BigIntLit {
                abs: vec![],
                negative: false
            }
        );
        assert_eq!(
            parse_big_int("1.5", 4096).unwrap_err(),
            "invalid big integer: 1.5"
        );
        assert_eq!(
            parse_big_int("-", 4096).unwrap_err(),
            "invalid big integer: -"
        );
        assert_eq!(
            parse_big_int("", 4096).unwrap_err(),
            "invalid big integer: "
        );
        assert_eq!(format_big_int(&[42], true), "-42");
        assert_eq!(format_big_int(&[], true), "0");
    }

    #[test]
    fn decimal_literals() {
        assert_eq!(
            parse_decimal("1.00", 4096).unwrap(),
            DecimalLit {
                unscaled: vec![100],
                scale: 2,
                negative: false
            }
        );
        assert_eq!(
            parse_decimal("-123.45", 4096).unwrap(),
            DecimalLit {
                unscaled: vec![0x30, 0x39],
                scale: 2,
                negative: true
            }
        );
        assert_eq!(
            parse_decimal("42", 4096).unwrap(),
            DecimalLit {
                unscaled: vec![42],
                scale: 0,
                negative: false
            }
        );
        // "1." is a float token; it is a decimal with scale 0.
        assert_eq!(parse_decimal("1.", 4096).unwrap().scale, 0);
        // The sign is read from the text, zero or not.
        let z = parse_decimal("-0.0", 4096).unwrap();
        assert_eq!((z.unscaled, z.scale, z.negative), (vec![], 1, true));
        assert_eq!(
            parse_decimal("1e5", 4096).unwrap_err(),
            "invalid decimal: 1e5"
        );
        assert_eq!(
            parse_decimal("1.5.2", 4096).unwrap_err(),
            "invalid decimal: 1.5.2"
        );
        assert_eq!(format_decimal(&[5], 2, false), "0.05");
        assert_eq!(format_decimal(&[100], 2, false), "1.00");
        assert_eq!(format_decimal(&[0x30, 0x39], 2, true), "-123.45");
        assert_eq!(format_decimal(&[42], 0, false), "42");
        assert_eq!(format_decimal(&[5], -2, false), "500");
        assert_eq!(format_decimal(&[], 1, true), "-0.0");
    }

    #[test]
    fn digit_cap_is_inclusive_and_counts_digits_only() {
        let at = "9".repeat(4096);
        assert!(check_literal_digits(&at, 4096).is_ok());
        assert!(check_literal_digits(&format!("-{}", at), 4096).is_ok());
        assert!(check_literal_digits(&format!("1.{}", &at[1..]), 4096).is_ok());
        let over = "9".repeat(4097);
        assert_eq!(
            check_literal_digits(&over, 4096).unwrap_err(),
            "numeric literal has 4097 digits; MaxNumericLiteralDigits=4096"
        );
        assert!(parse_big_int(&over, 4096).is_err());
        assert!(parse_decimal(&format!("{}.5", at), 4096).is_err());
        // The cap is per call.
        assert!(parse_big_int("12345", 4).is_err());
        assert!(parse_big_int("1234", 4).is_ok());
    }
}
