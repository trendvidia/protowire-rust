// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Go's `%g` of a float64, for error texts that state a value as
//! protowire-go's do. A copy of `protowire-pxf`'s `go_shortest_g`: this crate
//! does not depend on that one.

/// `fmt.Sprintf("%g", f)`: the shortest digits that read back as `f`, in
/// exponent form when the decimal exponent is below -4 or at least 6.
pub(crate) fn go_g(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 { "+Inf" } else { "-Inf" }.to_string();
    }
    go_shortest_g(&format!("{f:e}"))
}

/// Lays out a finite float as Go's `strconv.FormatFloat(f, 'g', -1, bits)`
/// does, which is what protowire-go writes. `sci` is Rust's `{:e}` of the
/// value: the same shortest round-trip digits, as `d.ddde<exp>`.
///
/// Go uses the exponent form when the decimal exponent is below -4 or at
/// least 6 (`1e+21`, `1.5e-07`, `1.234567e+06`), with a sign and at least
/// two exponent digits; otherwise it writes the digits plainly (`100000`,
/// `0.0001`). Rust's `{}` never uses an exponent, so it wrote
/// `1000000000000000000000` where the reference writes `1e+21`.
fn go_shortest_g(sci: &str) -> String {
    let (neg, rest) = match sci.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, sci),
    };
    let (mant, exp) = rest
        .split_once('e')
        .expect("{:e} of a finite float has an exponent");
    let exp: i32 = exp.parse().expect("{:e} exponent is an integer");
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let mut out = String::with_capacity(digits.len() + 8);
    if neg {
        out.push('-');
    }
    if !(-4..6).contains(&exp) {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        let a = exp.unsigned_abs();
        if a < 10 {
            out.push('0');
        }
        out.push_str(&a.to_string());
    } else if exp < 0 {
        out.push_str("0.");
        for _ in 0..(-exp - 1) {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        let int_len = exp as usize + 1;
        if digits.len() <= int_len {
            out.push_str(&digits);
            for _ in digits.len()..int_len {
                out.push('0');
            }
        } else {
            out.push_str(&digits[..int_len]);
            out.push('.');
            out.push_str(&digits[int_len..]);
        }
    }
    out
}
