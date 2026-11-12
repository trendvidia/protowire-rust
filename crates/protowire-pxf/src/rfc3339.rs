// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! A timestamp literal, read as protowire-go reads it (#118).
//!
//! protowire-go reads a timestamp with `time.Parse(time.RFC3339Nano, s)`,
//! then `time.Parse(time.RFC3339, s)`. Each tries Go's strict RFC 3339 fast
//! path first and falls back to its general layout parser, which is looser.
//! For these two layouts the general parser accepts everything the fast
//! path accepts, with the same value, and the two layouts accept the same
//! literals, so this follows the general parser over
//! `2006-01-02T15:04:05Z07:00`:
//!
//! - year: four digits; month, day, minute, second: two digits each; hour:
//!   one or two digits;
//! - month 1-12, day within the month, hour 0-23, minute and second 0-59
//!   (no leap second);
//! - after the seconds, `.` or `,` and at least one digit is a fraction, of
//!   any length, cut to nanoseconds (the lexer ends a timestamp at `,`, so
//!   a PXF literal never has the comma form);
//! - `Z`, or `+hh:mm` / `-hh:mm` with two digits each, hour up to 24 and
//!   minute up to 60, as Go allows;
//! - `T`, `Z`, `-` and `:` exactly as written, case included, and nothing
//!   after the zone.
//!
//! `testdata/conformance/timestamp-oracle.tsv` holds the port to what
//! protowire-go decides for each literal of `timestamp-cases.tsv`.

/// The seconds since the Unix epoch and the nanoseconds of `s`, or `None`
/// where protowire-go refuses the literal.
pub(crate) fn parse(s: &str) -> Option<(i64, i32)> {
    let mut v = s.as_bytes();
    // stdLongYear: four bytes, the first a digit, all digits.
    if v.len() < 4 || !v[..4].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let year = num(&v[..4]);
    v = &v[4..];
    v = skip(v, b'-')?;
    let (month, rest) = getnum(v, true)?;
    if !(1..=12).contains(&month) {
        return None;
    }
    v = skip(rest, b'-')?;
    let (day, rest) = getnum(v, true)?;
    v = skip(rest, b'T')?;
    let (hour, rest) = getnum(v, false)?;
    if hour >= 24 {
        return None;
    }
    v = skip(rest, b':')?;
    let (minute, rest) = getnum(v, true)?;
    if minute >= 60 {
        return None;
    }
    v = skip(rest, b':')?;
    let (second, rest) = getnum(v, true)?;
    if second >= 60 {
        return None;
    }
    v = rest;

    // A fraction after the seconds: '.' or ',' and a digit, then every digit.
    let mut nanos: i64 = 0;
    if v.len() >= 2 && (v[0] == b'.' || v[0] == b',') && v[1].is_ascii_digit() {
        let mut n = 2;
        while n < v.len() && v[n].is_ascii_digit() {
            n += 1;
        }
        // Go's parseNanoseconds reads at most nine digits.
        let digits = &v[1..n.min(10)];
        nanos = num(digits) as i64;
        for _ in digits.len()..9 {
            nanos *= 10;
        }
        v = &v[n..];
    }

    // stdISO8601ColonTZ.
    let mut offset: i64 = 0;
    if v.first() == Some(&b'Z') {
        v = &v[1..];
    } else {
        if v.len() < 6 || v[3] != b':' {
            return None;
        }
        let (hr, _) = getnum(&v[1..3], true)?;
        let (mm, _) = getnum(&v[4..6], true)?;
        if hr > 24 || mm > 60 {
            return None;
        }
        offset = i64::from(hr * 60 + mm) * 60;
        match v[0] {
            b'+' => {}
            b'-' => offset = -offset,
            _ => return None,
        }
        v = &v[6..];
    }
    if !v.is_empty() {
        return None;
    }
    if day < 1 || day > days_in_month(year, month) {
        return None;
    }
    let days = days_from_civil(year as i64, month, day);
    let seconds =
        days * 86400 + i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second) - offset;
    Some((seconds, nanos as i32))
}

/// Go's `skip` for a one-byte literal of the layout.
fn skip(v: &[u8], want: u8) -> Option<&[u8]> {
    match v.first() {
        Some(&b) if b == want => Some(&v[1..]),
        _ => None,
    }
}

/// Go's `getnum`: two digits, or one when the next byte is not a digit and
/// `fixed` is false.
fn getnum(v: &[u8], fixed: bool) -> Option<(u32, &[u8])> {
    let d0 = *v.first().filter(|b| b.is_ascii_digit())?;
    match v.get(1).filter(|b| b.is_ascii_digit()) {
        Some(&d1) => Some((u32::from(d0 - b'0') * 10 + u32::from(d1 - b'0'), &v[2..])),
        None if fixed => None,
        None => Some((u32::from(d0 - b'0'), &v[1..])),
    }
}

fn num(digits: &[u8]) -> u32 {
    digits.iter().fold(0, |n, d| n * 10 + u32::from(d - b'0'))
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Howard Hinnant's "days from civil": days since 1970-01-01 of a date in
/// the proleptic Gregorian calendar.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y / 400 } else { (y - 399) / 400 };
    let yoe = y - era * 400;
    let m_shift = i64::from(if m > 2 { m - 3 } else { m + 9 });
    let doy = (153 * m_shift + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}
