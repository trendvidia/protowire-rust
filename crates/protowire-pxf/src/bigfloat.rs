// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! `pxf.BigFloat`'s literal form (protowire-rust#39): a numeric literal on
//! a `pxf.BigFloat` field decodes to the wire message of
//! `proto/pxf/bignum.proto` — an unsigned big-endian mantissa, a binary
//! exponent, a precision and a sign — and the message encodes back to a
//! literal.
//!
//! STABILITY.md promise 2 makes the reference's bytes the contract, and
//! the reference parses a literal with Go's `math/big` at 256 bits:
//! `new(big.Float).SetPrec(256).Parse(s, 10)`. That conversion is **not**
//! correctly rounded — it multiplies or divides the exact digit integer by
//! a power of five that was itself rounded to 320 bits and built by
//! square-and-multiply at 384 bits, then rounds once more to 256 — so a
//! port that converted exactly would disagree with the reference in the
//! last bit for many literals. This module therefore mirrors `math/big`'s
//! steps rather than the mathematically nearest value: [`Fl::mul`] and
//! [`Fl::quo`] are correctly rounded (as Go's `umul` / `uquo` are), and
//! [`parse_big_float`] applies them in the reference's order with the
//! reference's precisions. The reference's text output is
//! `big.Float.Text('g', 78)`, mirrored by [`format_big_float`].
//!
//! The arithmetic is hand-rolled on a small unsigned big integer, in the
//! spirit of the rest of this crate (no bignum dependency); every
//! conversion is bounded by the literal's digit count
//! ([`MAX_NUMERIC_LITERAL_DIGITS`]) on the way in. Rendering is
//! proportional to the wire exponent's magnitude, as the reference's is
//! (protowire#281 is the open limit there).

use crate::bignum::check_literal_digits;
use crate::limits::MAX_NUMERIC_LITERAL_DIGITS;

/// The precision a PXF `BigFloat` literal is parsed at, in bits — the
/// reference's `bigFloatPrec`.
pub const BIG_FLOAT_PREC: u32 = 256;

/// A parsed `pxf.BigFloat` literal, as the wire message carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BigFloatLit {
    /// Unsigned big-endian mantissa; empty for zero. For a non-zero value
    /// it is exactly [`BIG_FLOAT_PREC`] bits with the top bit set.
    pub mantissa: Vec<u8>,
    /// Binary exponent: value = mantissa × 2^exponent.
    pub exponent: i32,
    pub prec: u32,
    pub negative: bool,
}

// ---------------------------------------------------------------------------
// Unsigned big integer: little-endian u32 limbs, no leading zero limbs.

#[derive(Debug, Clone, PartialEq, Eq)]
struct BigUint(Vec<u32>);

impl BigUint {
    fn zero() -> Self {
        BigUint(Vec::new())
    }

    fn from_u64(v: u64) -> Self {
        let mut n = BigUint(vec![v as u32, (v >> 32) as u32]);
        n.trim();
        n
    }

    fn from_be_bytes(bytes: &[u8]) -> Self {
        let mut limbs = Vec::with_capacity(bytes.len() / 4 + 1);
        let mut acc: u32 = 0;
        let mut shift = 0;
        for &b in bytes.iter().rev() {
            acc |= u32::from(b) << shift;
            shift += 8;
            if shift == 32 {
                limbs.push(acc);
                acc = 0;
                shift = 0;
            }
        }
        if shift > 0 {
            limbs.push(acc);
        }
        let mut n = BigUint(limbs);
        n.trim();
        n
    }

    /// ASCII decimal digits → integer (leading zeros fine).
    fn from_decimal_digits(digits: &[u8]) -> Self {
        let mut n = BigUint::zero();
        // Nine digits at a time: 10^9 fits a u32 and keeps the inner loop short.
        for chunk in digits.chunks(9) {
            let mut v: u32 = 0;
            let mut scale: u32 = 1;
            for &d in chunk {
                v = v * 10 + u32::from(d - b'0');
                scale *= 10;
            }
            n.mul_small_add(scale, v);
        }
        n
    }

    fn trim(&mut self) {
        while self.0.last() == Some(&0) {
            self.0.pop();
        }
    }

    fn is_zero(&self) -> bool {
        self.0.is_empty()
    }

    fn bit_len(&self) -> u64 {
        match self.0.last() {
            None => 0,
            Some(&top) => (self.0.len() as u64 - 1) * 32 + (32 - u64::from(top.leading_zeros())),
        }
    }

    fn bit(&self, i: u64) -> bool {
        let limb = (i / 32) as usize;
        limb < self.0.len() && (self.0[limb] >> (i % 32)) & 1 == 1
    }

    /// Any bit strictly below position `i` set.
    fn any_bit_below(&self, i: u64) -> bool {
        let limb = (i / 32) as usize;
        if self.0[..limb.min(self.0.len())].iter().any(|&w| w != 0) {
            return true;
        }
        limb < self.0.len() && (self.0[limb] & ((1u32 << (i % 32)) - 1)) != 0
    }

    fn lsb(&self) -> bool {
        self.bit(0)
    }

    fn mul_small_add(&mut self, m: u32, a: u32) {
        let mut carry = u64::from(a);
        for w in self.0.iter_mut() {
            let v = u64::from(*w) * u64::from(m) + carry;
            *w = v as u32;
            carry = v >> 32;
        }
        if carry > 0 {
            self.0.push(carry as u32);
        }
    }

    fn add_one(&mut self) {
        for w in self.0.iter_mut() {
            let (v, overflow) = w.overflowing_add(1);
            *w = v;
            if !overflow {
                return;
            }
        }
        self.0.push(1);
    }

    fn shl(&self, n: u64) -> Self {
        if self.is_zero() {
            return BigUint::zero();
        }
        let limbs = (n / 32) as usize;
        let bits = (n % 32) as u32;
        let mut out = vec![0u32; limbs];
        if bits == 0 {
            out.extend_from_slice(&self.0);
        } else {
            let mut carry: u32 = 0;
            for &w in &self.0 {
                out.push((w << bits) | carry);
                carry = w >> (32 - bits);
            }
            if carry > 0 {
                out.push(carry);
            }
        }
        let mut r = BigUint(out);
        r.trim();
        r
    }

    fn shr(&self, n: u64) -> Self {
        let limbs = (n / 32) as usize;
        if limbs >= self.0.len() {
            return BigUint::zero();
        }
        let bits = (n % 32) as u32;
        let src = &self.0[limbs..];
        let mut out = Vec::with_capacity(src.len());
        if bits == 0 {
            out.extend_from_slice(src);
        } else {
            for i in 0..src.len() {
                let lo = src[i] >> bits;
                let hi = if i + 1 < src.len() {
                    src[i + 1] << (32 - bits)
                } else {
                    0
                };
                out.push(lo | hi);
            }
        }
        let mut r = BigUint(out);
        r.trim();
        r
    }

    fn mul(&self, other: &BigUint) -> Self {
        if self.is_zero() || other.is_zero() {
            return BigUint::zero();
        }
        let mut out = vec![0u32; self.0.len() + other.0.len()];
        for (i, &a) in self.0.iter().enumerate() {
            let mut carry: u64 = 0;
            for (j, &b) in other.0.iter().enumerate() {
                let v = u64::from(a) * u64::from(b) + u64::from(out[i + j]) + carry;
                out[i + j] = v as u32;
                carry = v >> 32;
            }
            let mut k = i + other.0.len();
            while carry > 0 {
                let v = u64::from(out[k]) + carry;
                out[k] = v as u32;
                carry = v >> 32;
                k += 1;
            }
        }
        let mut r = BigUint(out);
        r.trim();
        r
    }

    fn cmp(&self, other: &BigUint) -> std::cmp::Ordering {
        if self.0.len() != other.0.len() {
            return self.0.len().cmp(&other.0.len());
        }
        for (a, b) in self.0.iter().rev().zip(other.0.iter().rev()) {
            if a != b {
                return a.cmp(b);
            }
        }
        std::cmp::Ordering::Equal
    }

    /// self -= other; requires self >= other.
    fn sub_assign(&mut self, other: &BigUint) {
        let mut borrow: i64 = 0;
        for (i, w) in self.0.iter_mut().enumerate() {
            let o = other.0.get(i).copied().unwrap_or(0);
            let v = i64::from(*w) - i64::from(o) - borrow;
            if v < 0 {
                *w = (v + (1i64 << 32)) as u32;
                borrow = 1;
            } else {
                *w = v as u32;
                borrow = 0;
            }
        }
        self.trim();
    }

    fn set_bit(&mut self, i: u64) {
        let limb = (i / 32) as usize;
        if self.0.len() <= limb {
            self.0.resize(limb + 1, 0);
        }
        self.0[limb] |= 1u32 << (i % 32);
    }

    /// Quotient and remainder by shift-subtract. Intended for quotients of
    /// a few hundred bits (the callers arrange the operand lengths so).
    fn divrem(&self, den: &BigUint) -> (BigUint, BigUint) {
        debug_assert!(!den.is_zero());
        if self.cmp(den) == std::cmp::Ordering::Less {
            return (BigUint::zero(), self.clone());
        }
        let shift = self.bit_len() - den.bit_len();
        let mut rem = self.clone();
        let mut q = BigUint::zero();
        let mut d = den.shl(shift);
        for i in (0..=shift).rev() {
            if rem.cmp(&d) != std::cmp::Ordering::Less {
                rem.sub_assign(&d);
                q.set_bit(i);
            }
            d = d.shr(1);
        }
        q.trim();
        (q, rem)
    }

    /// Exact 5^k.
    fn pow5_exact(k: u64) -> Self {
        let mut result = BigUint::from_u64(1);
        let mut base = BigUint::from_u64(5);
        let mut n = k;
        while n > 0 {
            if n & 1 == 1 {
                result = result.mul(&base);
            }
            n >>= 1;
            if n > 0 {
                base = base.mul(&base);
            }
        }
        result
    }

    fn to_be_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.0.len() * 4);
        for &w in self.0.iter().rev() {
            out.extend_from_slice(&w.to_be_bytes());
        }
        let first = out.iter().position(|&b| b != 0).unwrap_or(out.len());
        out.split_off(first)
    }

    fn to_decimal_string(&self) -> String {
        if self.is_zero() {
            return "0".to_string();
        }
        let mut limbs = self.0.clone();
        let mut chunks: Vec<u32> = Vec::new();
        while !limbs.is_empty() {
            let mut rem: u64 = 0;
            for w in limbs.iter_mut().rev() {
                let v = (rem << 32) | u64::from(*w);
                *w = (v / 1_000_000_000) as u32;
                rem = v % 1_000_000_000;
            }
            chunks.push(rem as u32);
            while limbs.last() == Some(&0) {
                limbs.pop();
            }
        }
        let mut s = String::with_capacity(chunks.len() * 9);
        let mut it = chunks.iter().rev();
        if let Some(first) = it.next() {
            s.push_str(&first.to_string());
        }
        for c in it {
            s.push_str(&format!("{:09}", c));
        }
        s
    }
}

// ---------------------------------------------------------------------------
// A `math/big`-shaped float: value = 0.mant × 2^exp, mant normalized to
// `prec` bits (top bit set) once rounded, with Go's forms and exponent range.

const MAX_EXP: i64 = i32::MAX as i64;
const MIN_EXP: i64 = i32::MIN as i64;

#[derive(Debug, Clone)]
enum Fl {
    Zero,
    Inf,
    /// `mant` has exactly `prec` bits with the top bit set; value =
    /// mant × 2^(exp − prec).
    Finite {
        mant: BigUint,
        exp: i64,
        prec: u32,
    },
}

impl Fl {
    /// Go's `setExpAndRound`: `mant` is an arbitrary-length integer, `exp`
    /// the exponent of the value `0.mant × 2^exp` (mant read as a binary
    /// fraction with its top bit just below the point), `sticky` whether
    /// any non-zero bits were dropped before this call. Rounds to `prec`
    /// bits, to nearest even.
    fn round_into(mant: BigUint, exp: i64, prec: u32, sticky: bool) -> Fl {
        if mant.is_zero() {
            return Fl::Zero;
        }
        if exp < MIN_EXP {
            return Fl::Zero;
        }
        if exp > MAX_EXP {
            return Fl::Inf;
        }
        let bits = mant.bit_len();
        let prec64 = u64::from(prec);
        if bits <= prec64 {
            return Fl::Finite {
                mant: mant.shl(prec64 - bits),
                exp,
                prec,
            };
        }
        let r = bits - prec64 - 1;
        let rbit = mant.bit(r);
        let sbit = sticky || mant.any_bit_below(r);
        let mut kept = mant.shr(bits - prec64);
        let mut exp = exp;
        if rbit && (sbit || kept.lsb()) {
            kept.add_one();
            if kept.bit_len() > prec64 {
                if exp >= MAX_EXP {
                    return Fl::Inf;
                }
                exp += 1;
                kept = kept.shr(1);
            }
        }
        Fl::Finite {
            mant: kept,
            exp,
            prec,
        }
    }

    fn from_u64(v: u64, prec: u32) -> Fl {
        let m = BigUint::from_u64(v);
        let bits = m.bit_len();
        Fl::round_into(m, bits as i64, prec, false)
    }

    /// Go's `Float.Mul` at precision `prec`: the exact product, rounded.
    fn mul(&self, other: &Fl, prec: u32) -> Fl {
        match (self, other) {
            (Fl::Zero, _) | (_, Fl::Zero) => Fl::Zero,
            (Fl::Inf, _) | (_, Fl::Inf) => Fl::Inf,
            (
                Fl::Finite {
                    mant: xm,
                    exp: xe,
                    prec: xp,
                },
                Fl::Finite {
                    mant: ym,
                    exp: ye,
                    prec: yp,
                },
            ) => {
                let product = xm.mul(ym);
                // value = (xm / 2^xp)(ym / 2^yp) × 2^(xe + ye)
                //       = product × 2^(xe + ye − xp − yp)
                //       = 0.product × 2^(xe + ye − xp − yp + bits)
                let exp = xe + ye - i64::from(*xp) - i64::from(*yp) + product.bit_len() as i64;
                Fl::round_into(product, exp, prec, false)
            }
        }
    }

    /// Go's `Float.Quo` at precision `prec`: the correctly rounded quotient,
    /// the remainder standing in for the dropped bits.
    fn quo(&self, other: &Fl, prec: u32) -> Fl {
        match (self, other) {
            (Fl::Zero, _) => Fl::Zero,
            (_, Fl::Inf) => Fl::Zero,
            (Fl::Inf, _) => Fl::Inf,
            (_, Fl::Zero) => Fl::Inf,
            (
                Fl::Finite {
                    mant: xm,
                    exp: xe,
                    prec: xp,
                },
                Fl::Finite {
                    mant: ym,
                    exp: ye,
                    prec: yp,
                },
            ) => {
                // Shift so the integer quotient carries prec + 2 bits or more.
                let want = i64::from(prec) + 2;
                let s = want + ym.bit_len() as i64 - xm.bit_len() as i64;
                let (num, den) = if s >= 0 {
                    (xm.shl(s as u64), ym.clone())
                } else {
                    (xm.clone(), ym.shl((-s) as u64))
                };
                let (q, r) = num.divrem(&den);
                // value = (xm / 2^xp) / (ym / 2^yp) × 2^(xe − ye)
                //       = q × 2^(−s) × 2^(xe − ye − xp + yp)
                //       = 0.q × 2^(xe − ye − xp + yp − s + bits)
                let exp = xe - ye - i64::from(*xp) + i64::from(*yp) - s + q.bit_len() as i64;
                Fl::round_into(q, exp, prec, !r.is_zero())
            }
        }
    }
}

/// Go's `(*Float).pow5(n)` at the receiver precision `prec`: exact from a
/// table up to 5^27, then square-and-multiply with the multiplier held at
/// `prec + 64` bits — every intermediate product rounded, exactly as the
/// reference computes it.
fn pow5(n: u64, prec: u32) -> Fl {
    const TABLE_MAX: u64 = 27; // 5^27 is the largest power of five in a uint64
    if n <= TABLE_MAX {
        return Fl::from_u64(5u64.pow(n as u32), prec);
    }
    let mut z = Fl::from_u64(5u64.pow(TABLE_MAX as u32), prec);
    let mut n = n - TABLE_MAX;
    let mut f = Fl::from_u64(5, prec + 64);
    while n > 0 {
        if n & 1 == 1 {
            z = z.mul(&f, prec);
        }
        f = f.mul(&f, prec + 64);
        n >>= 1;
    }
    z
}

fn clip_literal(s: &str) -> String {
    if s.len() <= 24 {
        return s.to_string();
    }
    format!("{}…", &s[..24])
}

fn has_non_zero_mantissa_digit(s: &str) -> bool {
    for b in s.bytes() {
        match b {
            b'e' | b'E' => return false,
            b'1'..=b'9' => return true,
            _ => {}
        }
    }
    false
}

/// Parse a `pxf.BigFloat` literal — an optional `-`, decimal digits with
/// an optional `.` fraction, and an optional `e` / `E` exponent — into the
/// wire message's fields, producing the bytes the reference produces for
/// the same literal.
pub fn parse_big_float(s: &str, max_digits: usize) -> Result<BigFloatLit, String> {
    check_literal_digits(s, max_digits)?;
    let invalid = || format!("invalid big float: {}", s);
    let (negative, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let (mant_text, exp_text) = match body.find(['e', 'E']) {
        Some(i) => (&body[..i], Some(&body[i + 1..])),
        None => (body, None),
    };
    let (int_part, frac_part) = match mant_text.split_once('.') {
        Some((i, f)) => (i, f),
        None => (mant_text, ""),
    };
    if (int_part.is_empty() && frac_part.is_empty())
        || !int_part.bytes().all(|b| b.is_ascii_digit())
        || !frac_part.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(invalid());
    }
    let exp10: i64 = match exp_text {
        None => 0,
        Some(e) => {
            let (sign, digits) = match e.as_bytes().first() {
                Some(b'+') => (1i64, &e[1..]),
                Some(b'-') => (-1i64, &e[1..]),
                _ => (1i64, e),
            };
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err(invalid());
            }
            let mut v: i64 = 0;
            for b in digits.bytes() {
                v = v
                    .checked_mul(10)
                    .and_then(|v| v.checked_add(i64::from(b - b'0')))
                    .ok_or_else(invalid)?;
            }
            sign * v
        }
    };

    let mut digits = Vec::with_capacity(int_part.len() + frac_part.len());
    digits.extend_from_slice(int_part.as_bytes());
    digits.extend_from_slice(frac_part.as_bytes());
    let d = BigUint::from_decimal_digits(&digits);
    let fcount = -(frac_part.len() as i64);

    let value = if d.is_zero() {
        Fl::Zero
    } else {
        // value = d × 10^(fcount + exp10) = d × 2^e × 5^e with e = fcount + exp10.
        let exp5 = fcount.checked_add(exp10).ok_or_else(invalid)?;
        let exp2 = (d.bit_len() as i64).checked_add(exp5).ok_or_else(invalid)?;
        if !(MIN_EXP..=MAX_EXP).contains(&exp2) {
            return Err(invalid()); // Go: "exponent overflow"
        }
        let bits = d.bit_len();
        let exact = Fl::Finite {
            mant: d.clone(),
            exp: exp2,
            prec: bits as u32,
        };
        if exp5 == 0 {
            Fl::round_into(d, exp2, BIG_FLOAT_PREC, false)
        } else {
            let p = pow5(exp5.unsigned_abs(), BIG_FLOAT_PREC + 64);
            if exp5 < 0 {
                exact.quo(&p, BIG_FLOAT_PREC)
            } else {
                exact.mul(&p, BIG_FLOAT_PREC)
            }
        }
    };

    match value {
        Fl::Inf => Err(format!(
            "big float literal {} is above big.Float's range",
            clip_literal(s)
        )),
        Fl::Zero if has_non_zero_mantissa_digit(body) => Err(format!(
            "big float literal {} is below big.Float's range",
            clip_literal(s)
        )),
        Fl::Zero => Ok(BigFloatLit {
            mantissa: Vec::new(),
            exponent: -(BIG_FLOAT_PREC as i32),
            prec: BIG_FLOAT_PREC,
            negative,
        }),
        Fl::Finite { mant, exp, prec } => {
            let wire_exp = exp - i64::from(prec);
            if wire_exp < MIN_EXP {
                return Err(format!(
                    "big float literal {} is below the wire's exponent range",
                    clip_literal(s)
                ));
            }
            Ok(BigFloatLit {
                mantissa: mant.to_be_bytes(),
                exponent: wire_exp as i32,
                prec,
                negative,
            })
        }
    }
}

/// Render a `pxf.BigFloat` message as the literal the reference writes:
/// `big.Float.Text('g', digits)` with `digits = ⌊prec × 0.30103⌋ + 1` (78 at
/// 256 bits) — the value's exact decimal expansion rounded to that many
/// significant digits, trailing zeros trimmed, in `%e` form when the
/// decimal exponent is below −4 or at least the digit count, else `%f`.
/// An empty mantissa is `0` (or `-0`); a message whose `prec` would render
/// to more than `MaxNumericLiteralDigits` digits, or whose exponent
/// overflows, is an error.
pub fn format_big_float(
    mantissa: &[u8],
    exponent: i32,
    prec: u32,
    negative: bool,
) -> Result<String, String> {
    if prec == 0 {
        return Ok("0".to_string());
    }
    // The reference's literal 0.30103, not LOG10_2: the two differ in the
    // sixth digit, and the floor below must agree with Go's for every
    // `prec` a producer may write.
    #[allow(clippy::approx_constant)]
    let dec_digits = (f64::from(prec) * 0.30103) as i64 + 1;
    if dec_digits > MAX_NUMERIC_LITERAL_DIGITS as i64 {
        return Err(format!(
            "pxf.BigFloat prec {} bits renders to {} digits; MaxNumericLiteralDigits={}",
            prec, dec_digits, MAX_NUMERIC_LITERAL_DIGITS
        ));
    }
    let sign = if negative { "-" } else { "" };
    let m = BigUint::from_be_bytes(mantissa);
    if m.is_zero() {
        return Ok(format!("{}0", sign));
    }
    // readBigFloat: SetPrec(prec).SetInt(m) rounds m to prec bits, then
    // SetMantExp scales by 2^exponent.
    let bits = m.bit_len() as i64;
    let value = Fl::round_into(m, bits, prec, false);
    let Fl::Finite { mant, exp, prec } = value else {
        return Ok(format!("{}Inf", if negative { "-" } else { "+" }));
    };
    let exp = exp + i64::from(exponent);
    if exp > MAX_EXP {
        return Ok(format!("{}Inf", if negative { "-" } else { "+" }));
    }
    if exp < MIN_EXP {
        return Ok(format!("{}0", sign));
    }
    // Exact decimal expansion of mant × 2^(exp − prec).
    let e2 = exp - i64::from(prec);
    let (digits, mut dexp): (String, i64) = if e2 >= 0 {
        let n = mant.shl(e2 as u64);
        let s = n.to_decimal_string();
        let len = s.len() as i64;
        (s, len)
    } else {
        let k = (-e2) as u64;
        let n = mant.mul(&BigUint::pow5_exact(k));
        let s = n.to_decimal_string();
        let len = s.len() as i64;
        (s, len - k as i64)
    };
    let mut d: Vec<u8> = digits.into_bytes();
    trim_zeros(&mut d, &mut dexp);

    // d.round(prec) with prec = dec_digits.
    let n = dec_digits as usize;
    if n < d.len() {
        let up = if d[n] == b'5' && n + 1 == d.len() {
            n > 0 && (d[n - 1] - b'0') & 1 != 0
        } else {
            d[n] >= b'5'
        };
        if up {
            let mut i = n;
            while i > 0 && d[i - 1] >= b'9' {
                i -= 1;
            }
            if i == 0 {
                d.clear();
                d.push(b'1');
                dexp += 1;
            } else {
                d[i - 1] += 1;
                d.truncate(i);
            }
        } else {
            d.truncate(n);
            trim_zeros(&mut d, &mut dexp);
        }
    }

    let mut out = String::from(sign);
    let mut prec_g = dec_digits;
    let mut eprec = prec_g;
    if eprec > d.len() as i64 && d.len() as i64 >= dexp {
        eprec = d.len() as i64;
    }
    let exp10 = dexp - 1;
    if exp10 < -4 || exp10 >= eprec {
        if prec_g > d.len() as i64 {
            prec_g = d.len() as i64;
        }
        fmt_e(&mut out, (prec_g - 1) as usize, &d, dexp);
    } else {
        if prec_g > dexp {
            prec_g = d.len() as i64;
        }
        fmt_f(&mut out, (prec_g - dexp).max(0) as usize, &d, dexp);
    }
    if out.len() > MAX_NUMERIC_LITERAL_DIGITS {
        let n = out.bytes().filter(u8::is_ascii_digit).count();
        if n > MAX_NUMERIC_LITERAL_DIGITS {
            return Err(format!(
                "pxf.BigFloat renders to {} digits; MaxNumericLiteralDigits={}",
                n, MAX_NUMERIC_LITERAL_DIGITS
            ));
        }
    }
    Ok(out)
}

fn trim_zeros(d: &mut Vec<u8>, dexp: &mut i64) {
    while d.last() == Some(&b'0') {
        d.pop();
    }
    if d.is_empty() {
        *dexp = 0;
    }
}

fn fmt_e(out: &mut String, prec: usize, d: &[u8], dexp: i64) {
    out.push(d.first().map_or('0', |&b| b as char));
    if prec > 0 {
        out.push('.');
        let m = d.len().min(prec + 1);
        let mut i = 1;
        if i < m {
            out.push_str(std::str::from_utf8(&d[i..m]).unwrap());
            i = m;
        }
        while i <= prec {
            out.push('0');
            i += 1;
        }
    }
    out.push('e');
    let exp = if d.is_empty() { 0 } else { dexp - 1 };
    if exp < 0 {
        out.push('-');
    } else {
        out.push('+');
    }
    let a = exp.unsigned_abs();
    if a < 10 {
        out.push('0');
    }
    out.push_str(&a.to_string());
}

fn fmt_f(out: &mut String, prec: usize, d: &[u8], dexp: i64) {
    if dexp > 0 {
        let m = d.len().min(dexp as usize);
        out.push_str(std::str::from_utf8(&d[..m]).unwrap());
        for _ in m..dexp as usize {
            out.push('0');
        }
    } else {
        out.push('0');
    }
    if prec > 0 {
        out.push('.');
        for i in 0..prec as i64 {
            let j = dexp + i;
            let ch = if j >= 0 && (j as usize) < d.len() {
                d[j as usize] as char
            } else {
                '0'
            };
            out.push(ch);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn biguint_basics() {
        let a = BigUint::from_decimal_digits(b"340282366920938463463374607431768211456"); // 2^128
        assert_eq!(a.bit_len(), 129);
        assert_eq!(
            a.to_decimal_string(),
            "340282366920938463463374607431768211456"
        );
        assert_eq!(a.to_be_bytes().len(), 17);
        assert_eq!(a.shr(128), BigUint::from_u64(1));
        assert_eq!(BigUint::from_u64(1).shl(128), a);
        let (q, r) = a.divrem(&BigUint::from_u64(10));
        assert_eq!(
            q.to_decimal_string(),
            "34028236692093846346337460743176821145"
        );
        assert_eq!(r, BigUint::from_u64(6));
        assert_eq!(
            BigUint::pow5_exact(27),
            BigUint::from_u64(7_450_580_596_923_828_125)
        );
        assert_eq!(
            BigUint::from_be_bytes(&[0, 0, 1, 0]).to_decimal_string(),
            "256"
        );
    }

    #[test]
    fn round_half_even() {
        // 0b1011 rounded to 3 bits: 101|1 → tie, lsb 1 → up → 110 (exp +0 since no carry past width)
        let f = Fl::round_into(BigUint::from_u64(0b1011), 4, 3, false);
        let Fl::Finite { mant, exp, .. } = f else {
            panic!()
        };
        assert_eq!(mant, BigUint::from_u64(0b110));
        assert_eq!(exp, 4);
        // 0b1001 rounded to 3 bits: 100|1 → tie, lsb 0 → down → 100
        let f = Fl::round_into(BigUint::from_u64(0b1001), 4, 3, false);
        let Fl::Finite { mant, .. } = f else { panic!() };
        assert_eq!(mant, BigUint::from_u64(0b100));
        // 0b1111 rounded to 3 bits: 111|1 → up → 1000 → renormalize → 100, exp+1
        let f = Fl::round_into(BigUint::from_u64(0b1111), 4, 3, false);
        let Fl::Finite { mant, exp, .. } = f else {
            panic!()
        };
        assert_eq!(mant, BigUint::from_u64(0b100));
        assert_eq!(exp, 5);
    }
}
