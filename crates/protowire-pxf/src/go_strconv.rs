// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Go's own parsers, with what they accept and their error text (#167).
//!
//! protowire-go reads a `(pxf.default)` literal with `strconv.ParseInt`,
//! `ParseUint`, `ParseFloat` and `time.Parse`, and a bytes literal, in a
//! document or a default, with `decodeBase64Lenient` over `encoding/base64`.
//! Its errors wrap what those return. These are ports of them, so this crate
//! accepts the same literals and says the same thing about the others, as
//! protowire-cpp (`detail/go_strconv`) and protowire-java
//! (`Base64Literal`) do.

/// Go's `strconv.Quote`: `s` in double quotes, with `\a \b \f \n \r \t \v`,
/// `\\` and `\"` escaped, another control character as `\x`, `\u` or `\U`,
/// and every other character as itself. Go keeps a character Unicode calls
/// printable; this keeps every non-control character, which agrees on every
/// literal these errors quote in practice.
pub(crate) fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\x07' => out.push_str("\\a"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x0b' => out.push_str("\\v"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c if c.is_control() => {
                if (c as u32) <= 0xffff {
                    out.push_str(&format!("\\u{:04x}", c as u32))
                } else {
                    out.push_str(&format!("\\U{:08x}", c as u32))
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn num_error(func: &str, s: &str, why: &str) -> String {
    format!("strconv.{func}: parsing {}: {why}", quote(s))
}

const SYNTAX: &str = "invalid syntax";
const RANGE: &str = "value out of range";

enum Uint {
    Ok(u64),
    Syntax,
    Range,
}

/// `ParseUint`'s loop over base-10 digits: a bad byte past the point of
/// overflow is never reached, so `"9999999999x"` is out of range for 32 bits.
fn read_uint(s: &str, bits: u32) -> Uint {
    if s.is_empty() {
        return Uint::Syntax;
    }
    let max = if bits == 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    };
    let cutoff = u64::MAX / 10 + 1;
    let mut n: u64 = 0;
    for b in s.bytes() {
        if !b.is_ascii_digit() {
            return Uint::Syntax;
        }
        if n >= cutoff {
            return Uint::Range;
        }
        n *= 10;
        let n1 = n.wrapping_add(u64::from(b - b'0'));
        if n1 < n || n1 > max {
            return Uint::Range;
        }
        n = n1;
    }
    Uint::Ok(n)
}

/// `strconv.ParseUint(s, 10, bits)`.
pub(crate) fn parse_uint(s: &str, bits: u32) -> Result<u64, String> {
    match read_uint(s, bits) {
        Uint::Ok(n) => Ok(n),
        Uint::Syntax => Err(num_error("ParseUint", s, SYNTAX)),
        Uint::Range => Err(num_error("ParseUint", s, RANGE)),
    }
}

/// `strconv.ParseInt(s, 10, bits)`: an optional sign, then `ParseUint`'s
/// digits.
pub(crate) fn parse_int(s: &str, bits: u32) -> Result<i64, String> {
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'+') => (false, &s[1..]),
        Some(b'-') => (true, &s[1..]),
        _ => (false, s),
    };
    let un = match read_uint(digits, bits) {
        Uint::Syntax => return Err(num_error("ParseInt", s, SYNTAX)),
        Uint::Range => return Err(num_error("ParseInt", s, RANGE)),
        Uint::Ok(n) => n,
    };
    let cutoff = 1u64 << (bits - 1);
    if (!neg && un >= cutoff) || (neg && un > cutoff) {
        return Err(num_error("ParseInt", s, RANGE));
    }
    Ok(if neg {
        (!un).wrapping_add(1) as i64
    } else {
        un as i64
    })
}

/// strconv's rule for underscores: one separates digits, or follows a base
/// prefix, and never ends the number.
fn underscore_ok(s: &str) -> bool {
    let s = s.strip_prefix(['+', '-']).unwrap_or(s).as_bytes();
    let mut prev = b'^';
    let mut i = 0;
    let mut hex = false;
    if s.len() >= 2 && s[0] == b'0' && matches!(s[1].to_ascii_lowercase(), b'b' | b'o' | b'x') {
        i = 2;
        prev = b'0';
        hex = s[1].eq_ignore_ascii_case(&b'x');
    }
    while i < s.len() {
        let c = s[i];
        if c.is_ascii_digit() || (hex && c.is_ascii_hexdigit()) {
            prev = b'0';
        } else if c == b'_' {
            if prev != b'0' {
                return false;
            }
            prev = b'_';
        } else {
            if prev == b'_' {
                return false;
            }
            prev = b'!';
        }
        i += 1;
    }
    prev != b'_'
}

/// The length of the case-insensitive match of `s` and the lower-case `word`.
fn common_prefix(s: &[u8], word: &[u8]) -> usize {
    s.iter()
        .zip(word)
        .take_while(|(a, b)| a.to_ascii_lowercase() == **b)
        .count()
}

/// `strconv.ParseFloat(s, bits)`, `bits` being 32 or 64, and the value as
/// a double holding the `bits`-wide result. A NaN is Go's `math.NaN()`;
/// callers that write it canonicalize it.
pub(crate) fn parse_float(s: &str, bits: u32) -> Result<f64, String> {
    let syntax = || Err(num_error("ParseFloat", s, SYNTAX));
    let b = s.as_bytes();
    if b.is_empty() {
        return syntax();
    }
    // Inf, Infinity and NaN, in any case. A sign is taken on the first two.
    {
        let (sign, t) = match b[0] {
            b'+' => (Some(1.0), &b[1..]),
            b'-' => (Some(-1.0), &b[1..]),
            _ => (None, b),
        };
        if matches!(t.first(), Some(b'i' | b'I')) {
            let n = common_prefix(t, b"infinity");
            if (n == 3 && t.len() == 3) || (n == 8 && t.len() == 8) {
                return Ok(sign.unwrap_or(1.0) * f64::INFINITY);
            }
        }
        if sign.is_none() && common_prefix(t, b"nan") == 3 && t.len() == 3 {
            return Ok(f64::from_bits(0x7FF8_0000_0000_0001));
        }
    }

    let mut i = usize::from(matches!(b[0], b'+' | b'-'));
    let mut hex = false;
    if i + 2 < b.len() && b[i] == b'0' && b[i + 1].eq_ignore_ascii_case(&b'x') {
        hex = true;
        i += 2;
    }
    let (mut saw_dot, mut saw_digits, mut underscores) = (false, false, false);
    while i < b.len() {
        let c = b[i];
        if c == b'_' {
            underscores = true;
        } else if c == b'.' {
            if saw_dot {
                break;
            }
            saw_dot = true;
        } else if c.is_ascii_digit() || (hex && c.is_ascii_hexdigit()) {
            saw_digits = true;
        } else {
            break;
        }
        i += 1;
    }
    if !saw_digits {
        return syntax();
    }
    let mut saw_exp = false;
    if i < b.len() && b[i].to_ascii_lowercase() == if hex { b'p' } else { b'e' } {
        i += 1;
        if i >= b.len() {
            return syntax();
        }
        if matches!(b[i], b'+' | b'-') {
            i += 1;
        }
        if i >= b.len() || !b[i].is_ascii_digit() {
            return syntax();
        }
        while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
            underscores |= b[i] == b'_';
            i += 1;
        }
        saw_exp = true;
    }
    if (hex && !saw_exp) || i != b.len() || (underscores && !underscore_ok(s)) {
        return syntax();
    }

    let plain: String = s.chars().filter(|&c| c != '_').collect();
    let v = if hex {
        hex_float(&plain, bits)
    } else if bits == 32 {
        f64::from(
            plain
                .parse::<f32>()
                .map_err(|_| num_error("ParseFloat", s, SYNTAX))?,
        )
    } else {
        plain
            .parse::<f64>()
            .map_err(|_| num_error("ParseFloat", s, SYNTAX))?
    };
    if v.is_infinite() {
        return Err(num_error("ParseFloat", s, RANGE));
    }
    Ok(v)
}

/// A hex float the scanner above admitted (`[+-]0x` digits, an optional
/// point, `p` and a decimal exponent, underscores gone), rounded once, to
/// nearest with ties to even, to a `bits`-wide float, as Go's `atof` does.
/// Out of range is an infinity.
fn hex_float(s: &str, bits: u32) -> f64 {
    let (neg, s) = match s.as_bytes()[0] {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    let s = &s[2..];
    let (mant, exp) = s.split_at(s.find(['p', 'P']).expect("scanned"));
    let exp: i64 = exp[1..].parse().unwrap_or(if exp[1..].starts_with('-') {
        i64::MIN / 4
    } else {
        i64::MAX / 4
    });

    // The mantissa's digits into 124 bits; the rest only say whether
    // anything was dropped (sticky), and integer digits past it scale.
    let mut m: u128 = 0;
    let mut e: i64 = exp;
    let mut sticky = false;
    let mut after_dot = false;
    for c in mant.chars() {
        if c == '.' {
            after_dot = true;
            continue;
        }
        let d = c.to_digit(16).expect("scanned") as u128;
        if m >> 120 == 0 {
            m = m << 4 | d;
            if after_dot {
                e -= 4;
            }
        } else {
            sticky |= d != 0;
            if !after_dot {
                e += 4;
            }
        }
    }
    let sign = if neg { -1.0 } else { 1.0 };
    if m == 0 {
        return sign * 0.0;
    }

    let (p, emin, emax): (i64, i64, i64) = if bits == 32 {
        (24, -126, 127)
    } else {
        (53, -1022, 1023)
    };
    // The value is m * 2^e, m of n bits, so its leading bit is 2^x. The
    // result's least significant bit is 2^lsb: p bits below the leading one,
    // or the least subnormal's.
    let n = 128 - i64::from(m.leading_zeros());
    let x = n - 1 + e;
    if x > emax {
        return sign * f64::INFINITY;
    }
    let lsb = (x - (p - 1)).max(emin - (p - 1));
    let shift = lsb - e;
    let q: u128 = if shift <= 0 {
        m << -shift
    } else if shift > 127 {
        0 // below half the least subnormal
    } else {
        let q = m >> shift;
        let rem = m & ((1u128 << shift) - 1);
        let half = 1u128 << (shift - 1);
        let up = rem > half || (rem == half && (sticky || q & 1 == 1));
        q + u128::from(up)
    };
    if q == 0 {
        return sign * 0.0;
    }
    // Rounding may carry one bit past p; the range check is after it.
    if i64::from(q.ilog2()) + lsb > emax {
        return sign * f64::INFINITY;
    }
    // q * 2^lsb is exact in the target now. Two steps, so neither power of
    // two under- or overflows on its own.
    let half = lsb / 2;
    let v = q as f64 * 2f64.powi(half as i32) * 2f64.powi((lsb - half) as i32);
    sign * v
}

/// Package time's quote: `s` in double quotes, a non-ASCII or control byte
/// as `\x`, `"` and `\` escaped.
fn time_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for b in s.bytes() {
        if !(0x20..0x80).contains(&b) {
            out.push_str(&format!("\\x{b:02x}"));
        } else {
            if b == b'"' || b == b'\\' {
                out.push('\\');
            }
            out.push(b as char);
        }
    }
    out.push('"');
    out
}

const LAYOUT: &str = "2006-01-02T15:04:05Z07:00";

/// The error of `time.Parse(time.RFC3339, value)` for a `value` it refuses:
/// where in the layout the value stops matching, or which element is out of
/// range. protowire-cpp's `GoTimestampError`.
pub(crate) fn timestamp_error(value: &str) -> String {
    let parse_err = |rest: &str, elem: &str| {
        format!(
            "parsing time {} as {}: cannot parse {} as {}",
            time_quote(value),
            time_quote(LAYOUT),
            time_quote(rest),
            time_quote(elem)
        )
    };
    let range = |what: &str| format!("parsing time {}: {what} out of range", time_quote(value));
    // getnum: one or two digits, or exactly two if fixed.
    fn get_num(s: &mut &str, fixed: bool) -> Option<u32> {
        let b = s.as_bytes();
        if b.is_empty() || !b[0].is_ascii_digit() {
            return None;
        }
        if b.len() < 2 || !b[1].is_ascii_digit() {
            if fixed {
                return None;
            }
            *s = &s[1..];
            return Some(u32::from(b[0] - b'0'));
        }
        *s = &s[2..];
        Some(u32::from(b[0] - b'0') * 10 + u32::from(b[1] - b'0'))
    }
    let literal = |s: &mut &str, c: char| -> bool {
        if s.starts_with(c) {
            *s = &s[1..];
            true
        } else {
            false
        }
    };

    let mut v: &str = value;
    let b = v.as_bytes();
    if b.len() < 4 || !b[..4].iter().all(u8::is_ascii_digit) {
        return parse_err(v, "2006");
    }
    let year: u32 = v[..4].parse().expect("digits");
    v = &v[4..];
    if !literal(&mut v, '-') {
        return parse_err(v, "-");
    }
    let hold = v;
    let Some(month) = get_num(&mut v, true) else {
        return parse_err(hold, "01");
    };
    if month == 0 || month > 12 {
        return range("month");
    }
    if !literal(&mut v, '-') {
        return parse_err(v, "-");
    }
    let hold = v;
    let Some(day) = get_num(&mut v, true) else {
        return parse_err(hold, "02");
    };
    if !literal(&mut v, 'T') {
        return parse_err(v, "T");
    }
    let hold = v;
    let Some(hour) = get_num(&mut v, false) else {
        return parse_err(hold, "15");
    };
    if hour >= 24 {
        return range("hour");
    }
    if !literal(&mut v, ':') {
        return parse_err(v, ":");
    }
    let hold = v;
    let Some(minute) = get_num(&mut v, true) else {
        return parse_err(hold, "04");
    };
    if minute >= 60 {
        return range("minute");
    }
    if !literal(&mut v, ':') {
        return parse_err(v, ":");
    }
    let hold = v;
    let Some(second) = get_num(&mut v, true) else {
        return parse_err(hold, "05");
    };
    if second >= 60 {
        return range("second");
    }
    let vb = v.as_bytes();
    if vb.len() >= 2 && matches!(vb[0], b'.' | b',') && vb[1].is_ascii_digit() {
        let n = 2 + vb[2..].iter().take_while(|c| c.is_ascii_digit()).count();
        v = &v[n..];
    }
    let hold = v;
    if v.starts_with('Z') {
        v = &v[1..];
    } else {
        let vb = v.as_bytes();
        if vb.len() < 6 || vb[3] != b':' {
            return parse_err(hold, "Z07:00");
        }
        let (mut hh, mut mm) = (&v[1..3], &v[4..6]);
        let sign = vb[0];
        v = &v[6..];
        let oh = get_num(&mut hh, true);
        let om = get_num(&mut mm, true);
        if oh.is_some_and(|h| h > 24) {
            return range("time zone offset hour");
        }
        if om.is_some_and(|m| m > 60) {
            return range("time zone offset minute");
        }
        if oh.is_none() || om.is_none() || (sign != b'+' && sign != b'-') {
            return parse_err(hold, "Z07:00");
        }
    }
    if !v.is_empty() {
        return format!(
            "parsing time {}: extra text: {}",
            time_quote(value),
            time_quote(v)
        );
    }
    const DAYS: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let leap = month == 2 && year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    if day < 1 || day > DAYS[month as usize - 1] + u32::from(leap) {
        return range("day");
    }
    format!("parsing time {}: extra text: \"\"", time_quote(value))
}

const STD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn index(alphabet: &[u8; 64], c: u8) -> Option<u32> {
    alphabet.iter().position(|&a| a == c).map(|i| i as u32)
}

/// Go's `Encoding.DecodeString` for one alphabet, padded or not, as a loop
/// of its `decodeQuantum`; the error is the input byte it stopped at.
/// `\r` and `\n` are skipped wherever they appear.
fn decode_with(src: &[u8], alphabet: &[u8; 64], padded: bool) -> Result<Vec<u8>, usize> {
    let skip_newlines = |si: &mut usize| {
        while *si < src.len() && matches!(src[*si], b'\n' | b'\r') {
            *si += 1;
        }
    };
    let mut out = Vec::with_capacity(src.len() / 4 * 3 + 3);
    let mut si = 0;
    while si < src.len() {
        let mut dbuf = [0u32; 4];
        let mut dlen = 4;
        let mut done = false;
        let mut j = 0;
        while j < 4 {
            if si == src.len() {
                if j == 0 {
                    return Ok(out);
                }
                if j == 1 || padded {
                    return Err(si - j);
                }
                dlen = j;
                done = true;
                break;
            }
            let c = src[si];
            si += 1;
            if let Some(v) = index(alphabet, c) {
                dbuf[j] = v;
                j += 1;
                continue;
            }
            if c == b'\n' || c == b'\r' {
                continue;
            }
            if !padded || c != b'=' {
                return Err(si - 1);
            }
            // The end, and padding.
            match j {
                0 | 1 => return Err(si - 1),
                2 => {
                    // "==" is expected; the first '=' is consumed.
                    skip_newlines(&mut si);
                    if si == src.len() {
                        return Err(src.len());
                    }
                    if src[si] != b'=' {
                        return Err(si - 1);
                    }
                    si += 1;
                }
                _ => {}
            }
            skip_newlines(&mut si);
            if si < src.len() {
                return Err(si); // trailing garbage
            }
            dlen = j;
            done = true;
            break;
        }
        let val = dbuf[0] << 18 | dbuf[1] << 12 | dbuf[2] << 6 | dbuf[3];
        let quantum = [(val >> 16) as u8, (val >> 8) as u8, val as u8];
        out.extend_from_slice(&quantum[..dlen - 1]);
        if done {
            break;
        }
    }
    Ok(out)
}

fn base64_error(offset: usize) -> String {
    format!("illegal base64 data at input byte {offset}")
}

/// protowire-go's `decodeBase64Lenient`, for a bytes literal in a document
/// or a default: the standard alphabet padded, then unpadded, then the
/// URL-safe one padded and unpadded; when none takes `raw`, the standard
/// padded one's error.
pub(crate) fn decode_base64_lenient(raw: &str) -> Result<Vec<u8>, String> {
    let src = raw.as_bytes();
    let first = match decode_with(src, STD, true) {
        Ok(v) => return Ok(v),
        Err(at) => at,
    };
    [(STD, false), (URL, true), (URL, false)]
        .into_iter()
        .find_map(|(a, p)| decode_with(src, a, p).ok())
        .ok_or_else(|| base64_error(first))
}

/// A `@proto b"…"` descriptor body, as protowire-go's parser decodes it:
/// the standard alphabet, then the URL-safe one, both padded; when neither
/// takes it, the URL-safe one's error, which is the one Go's parser keeps.
pub(crate) fn decode_base64_descriptor(raw: &str) -> Result<Vec<u8>, String> {
    let src = raw.as_bytes();
    decode_with(src, STD, true)
        .or_else(|_| decode_with(src, URL, true))
        .map_err(base64_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ints_as_strconv() {
        assert_eq!(parse_int("+5", 32), Ok(5));
        assert_eq!(parse_int("-2147483648", 32), Ok(-2147483648));
        assert_eq!(
            parse_int("2147483648", 32).unwrap_err(),
            r#"strconv.ParseInt: parsing "2147483648": value out of range"#
        );
        assert_eq!(
            parse_int("3000000000x", 32).unwrap_err(),
            r#"strconv.ParseInt: parsing "3000000000x": invalid syntax"#
        );
        assert_eq!(
            parse_int("9999999999x", 32).unwrap_err(),
            r#"strconv.ParseInt: parsing "9999999999x": value out of range"#
        );
        assert_eq!(
            parse_uint("+5", 32).unwrap_err(),
            r#"strconv.ParseUint: parsing "+5": invalid syntax"#
        );
        assert_eq!(parse_uint("18446744073709551615", 64), Ok(u64::MAX));
    }

    #[test]
    fn hex_floats_round_once() {
        assert_eq!(parse_float("0x1p-2", 64), Ok(0.25));
        assert_eq!(parse_float("-0x1.8p1", 64), Ok(-3.0));
        assert_eq!(parse_float("0x1p-1074", 64), Ok(f64::from_bits(1)));
        assert_eq!(parse_float("0x1p-1075", 64), Ok(0.0));
        assert_eq!(parse_float("0x1.8p-1074", 64), Ok(f64::from_bits(2)));
        assert_eq!(parse_float("0x1.fffffffffffff8p0", 64), Ok(2.0));
        assert_eq!(parse_float("0x1.fffffep127", 32), Ok(f64::from(f32::MAX)));
        assert!(parse_float("0x1p128", 32).is_err());
        assert_eq!(
            parse_float("0x1p-149", 32),
            Ok(f64::from(f32::from_bits(1)))
        );
        assert_eq!(parse_float("0x1.000001p0", 32), Ok(1.0));
        // 1 + 1.5 ulp: a tie, to the even 1 + 2 ulp.
        assert_eq!(
            parse_float("0x1.000003p0", 32),
            Ok(f64::from(1.0f32 + 2.0 * f32::EPSILON))
        );
    }

    #[test]
    fn base64_as_go() {
        assert_eq!(decode_base64_lenient("aGk-").unwrap(), b"hi>");
        assert_eq!(decode_base64_lenient("aGk"), Ok(b"hi".to_vec()));
        assert_eq!(
            decode_base64_lenient("aGVsbG8==").unwrap_err(),
            "illegal base64 data at input byte 8"
        );
        assert_eq!(
            decode_base64_lenient("aG k=").unwrap_err(),
            "illegal base64 data at input byte 2"
        );
        assert_eq!(decode_base64_lenient("aGVs\nbG8="), Ok(b"hello".to_vec()));
    }
}
