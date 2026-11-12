// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! A duration literal, read as protowire-go reads it (#153).
//!
//! protowire-go's lexer validates a duration with Go's
//! `time.ParseDuration`, and its decoder takes the value from the same
//! call. This is that function, line for line: a segment is digits, an
//! optional `.` and digits (one side may be empty, so `.5m` is a segment),
//! and a unit; a fraction is applied with float64 arithmetic, as Go applies
//! it, so a long fraction of an hour rounds as Go rounds it; and a total
//! past 2^63 nanoseconds (2^63 - 1 when positive) is refused, so the lexer
//! refuses it, as Go's does. Errors carry the time package's text.

const NANOSECOND: u64 = 1;
const MICROSECOND: u64 = 1_000;
const MILLISECOND: u64 = 1_000_000;
const SECOND: u64 = 1_000_000_000;
const MINUTE: u64 = 60 * SECOND;
const HOUR: u64 = 60 * MINUTE;

/// The nanoseconds `s` stands for, or the time package's error for it.
pub(crate) fn parse(s: &str) -> Result<i64, String> {
    // [-+]?([0-9]*(\.[0-9]*)?[a-z]+)+
    let orig = s;
    let invalid = || format!("time: invalid duration {}", quote(orig));
    let mut s = s.as_bytes();
    let mut d: u64 = 0;
    let mut neg = false;

    // Consume [-+]?
    if let Some(&c) = s.first() {
        if c == b'-' || c == b'+' {
            neg = c == b'-';
            s = &s[1..];
        }
    }
    // Special case: if all that is left is "0", this is zero.
    if s == b"0" {
        return Ok(0);
    }
    if s.is_empty() {
        return Err(invalid());
    }
    while !s.is_empty() {
        // The next character must be [0-9.]
        if !(s[0] == b'.' || s[0].is_ascii_digit()) {
            return Err(invalid());
        }
        // Consume [0-9]*
        let pl = s.len();
        let (mut v, rest) = leading_int(s).ok_or_else(invalid)?;
        s = rest;
        let pre = pl != s.len(); // whether we consumed anything before a period

        // Consume (\.[0-9]*)?
        let mut post = false;
        let (mut f, mut scale) = (0u64, 1f64);
        if s.first() == Some(&b'.') {
            s = &s[1..];
            let pl = s.len();
            let (ff, sc, rest) = leading_fraction(s);
            (f, scale, s) = (ff, sc, rest);
            post = pl != s.len();
        }
        if !pre && !post {
            // no digits (e.g. ".s" or "-.s")
            return Err(invalid());
        }

        // Consume unit.
        let i = s
            .iter()
            .position(|&c| c == b'.' || c.is_ascii_digit())
            .unwrap_or(s.len());
        if i == 0 {
            return Err(format!("time: missing unit in duration {}", quote(orig)));
        }
        let u = &s[..i];
        s = &s[i..];
        let unit = match unit_of(u) {
            Some(unit) => unit,
            None => {
                return Err(format!(
                    "time: unknown unit {} in duration {}",
                    quote(&String::from_utf8_lossy(u)),
                    quote(orig)
                ))
            }
        };
        if v > (1u64 << 63) / unit {
            // overflow
            return Err(invalid());
        }
        v *= unit;
        if f > 0 {
            // float64 is needed to be nanosecond accurate for fractions of
            // hours. v >= 0 && (f*unit/scale) <= 3.6e+12 (ns/h, h is the
            // largest unit)
            v += (f as f64 * (unit as f64 / scale)) as u64;
            if v > 1u64 << 63 {
                // overflow
                return Err(invalid());
            }
        }
        d += v;
        if d > 1u64 << 63 {
            return Err(invalid());
        }
    }
    if neg {
        return Ok((d as i64).wrapping_neg());
    }
    if d > (1u64 << 63) - 1 {
        return Err(invalid());
    }
    Ok(d as i64)
}

/// Go's `unitMap`: the nanoseconds in one of `u`. "µs" is U+00B5 MICRO
/// SIGN and "μs" U+03BC GREEK SMALL LETTER MU; the lexer passes only the
/// first (draft -01 micro-us).
fn unit_of(u: &[u8]) -> Option<u64> {
    Some(match u {
        b"ns" => NANOSECOND,
        b"us" | b"\xc2\xb5s" | b"\xce\xbcs" => MICROSECOND,
        b"ms" => MILLISECOND,
        b"s" => SECOND,
        b"m" => MINUTE,
        b"h" => HOUR,
        _ => return None,
    })
}

/// Go's `leadingInt`: the leading digits of `s` and the rest, or `None`
/// past 2^63.
fn leading_int(s: &[u8]) -> Option<(u64, &[u8])> {
    let mut x: u64 = 0;
    let mut i = 0;
    while i < s.len() && s[i].is_ascii_digit() {
        if x > (1u64 << 63) / 10 {
            return None; // overflow
        }
        x = x * 10 + u64::from(s[i] - b'0');
        if x > 1u64 << 63 {
            return None; // overflow
        }
        i += 1;
    }
    Some((x, &s[i..]))
}

/// Go's `leadingFraction`: the leading digits of `s` as a number and the
/// power of ten they are scaled by, digits past what fits ignored, and the
/// rest.
fn leading_fraction(s: &[u8]) -> (u64, f64, &[u8]) {
    let mut x: u64 = 0;
    let mut scale = 1f64;
    let mut overflow = false;
    let mut i = 0;
    while i < s.len() && s[i].is_ascii_digit() {
        let c = s[i];
        i += 1;
        if overflow {
            continue;
        }
        if x > ((1u64 << 63) - 1) / 10 {
            // It's possible for overflow to give a positive number, so take
            // care.
            overflow = true;
            continue;
        }
        let y = x * 10 + u64::from(c - b'0');
        if y > 1u64 << 63 {
            overflow = true;
            continue;
        }
        x = y;
        scale *= 10.0;
    }
    (x, scale, &s[i..])
}

/// The time package's `quote`: `s` in double quotes, a non-ASCII or control
/// character as `\x` escapes of its bytes, `"` and `\` escaped.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if !c.is_ascii() || c < ' ' {
            let mut b = [0u8; 4];
            for byte in c.encode_utf8(&mut b).bytes() {
                out.push_str(&format!("\\x{byte:02x}"));
            }
        } else {
            if c == '"' || c == '\\' {
                out.push('\\');
            }
            out.push(c);
        }
    }
    out.push('"');
    out
}
