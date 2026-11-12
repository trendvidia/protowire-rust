// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Big-number struct fields: `pxf.BigInt`, `pxf.Decimal` and `pxf.BigFloat`
//! as protowire-go's `encoding/pb` binds `*big.Int`, `*big.Rat` and
//! `*big.Float`, and protowire-cpp's `pb_big.h` binds them byte-backed (#142).
//!
//! Each type holds the message's fields, so a program with no big-number
//! library can still read and write the values. Writing follows
//! protowire-go, which writes one form per value:
//!
//! - a [`BigInt`] as its magnitude and sign, with no leading zero byte and
//!   no `-0`;
//! - a [`Decimal`] at its smallest scale >= 0, since Go holds a `big.Rat`,
//!   which has no scale to keep: `{100, 2}` goes out as `{1, 0}`;
//! - a [`BigFloat`] as a prec-bit mantissa with its top bit set, rounded to
//!   prec half to even, and a zero with no exponent: `{25, -5, 64}` goes out
//!   as an eight-byte mantissa and exponent -64, the same number.
//!
//! A value a reader would refuse is refused on write too, with
//! protowire-go's text: a Decimal scale or a BigFloat prec past
//! `MaxNumericLiteralDigits`, a BigFloat binary exponent past
//! `MaxBigFloatExponent`. Reading checks the same bounds under the
//! reader's [`Limits`] before anything is built for the value.
//!
//! A field of one of these types is written with [`write_big_int`],
//! [`write_decimal`] or [`write_big_float`], which leave a zero out as Go
//! leaves a zero singular field out. A list element or a map value is
//! written with [`crate::write_message`] or [`crate::write_map_entry`],
//! which always write it, so a zero or null element is an empty message,
//! as in Go. Read any of them with [`crate::read_message`].

use crate::codec::{write_message, Message};
use crate::wire::{Error, Limits, Reader, Result, WireType, Writer};
use crate::wire::{MAX_BIG_FLOAT_EXPONENT, MAX_NUMERIC_LITERAL_DIGITS};

/// `pxf.BigInt`: value = (-1)^negative × abs, `abs` big-endian.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BigInt {
    /// Big-endian magnitude; empty, or all zero bytes, is zero.
    pub abs: Vec<u8>,
    pub negative: bool,
}

/// `pxf.Decimal`: value = (-1)^negative × unscaled × 10^(-scale).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Decimal {
    /// Big-endian magnitude; empty, or all zero bytes, is zero.
    pub unscaled: Vec<u8>,
    pub scale: i32,
    pub negative: bool,
}

/// `pxf.BigFloat`: value = (-1)^negative × mantissa × 2^exponent, rounded
/// to `prec` bits, half to even. A prec of 0 is a zero, signed by
/// `negative`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BigFloat {
    /// Big-endian magnitude.
    pub mantissa: Vec<u8>,
    pub exponent: i32,
    pub prec: u32,
    pub negative: bool,
}

impl BigInt {
    /// Whether the value is zero.
    pub fn is_zero(&self) -> bool {
        self.abs.iter().all(|b| *b == 0)
    }
}

impl Decimal {
    /// Whether the value is zero, at any scale.
    pub fn is_zero(&self) -> bool {
        self.unscaled.iter().all(|b| *b == 0)
    }
}

impl BigFloat {
    /// Whether the value is zero, of either sign.
    pub fn is_zero(&self) -> bool {
        self.prec == 0 || self.mantissa.iter().all(|b| *b == 0)
    }
}

/// Write a singular `pxf.BigInt` field: left out when the value is zero,
/// as protowire-go leaves out a zero `big.Int`.
pub fn write_big_int(w: &mut Writer, field_number: u32, v: &BigInt) {
    if !v.is_zero() {
        write_message(w, field_number, v);
    }
}

/// Write a singular `pxf.Decimal` field: left out when the value is zero,
/// at any scale and of either sign, as protowire-go leaves out a zero
/// `big.Rat`.
pub fn write_decimal(w: &mut Writer, field_number: u32, v: &Decimal) {
    if !v.is_zero() {
        write_message(w, field_number, v);
    }
}

/// Write a singular `pxf.BigFloat` field: left out only for `+0` at prec 0,
/// the empty message. `-0` is `{negative: true}`, as the PXF literal `-0`
/// reads (protowire#410), and a zero at a prec carries its prec.
pub fn write_big_float(w: &mut Writer, field_number: u32, v: &BigFloat) {
    if v.prec != 0 || v.negative {
        write_message(w, field_number, v);
    }
}

impl Message for BigInt {
    fn encode_to(&self, w: &mut Writer) {
        let abs = trimmed(&self.abs);
        if !abs.is_empty() {
            w.tag(1, WireType::LengthDelimited);
            w.bytes(abs);
            if self.negative {
                w.tag(2, WireType::Varint);
                w.varint(1);
            }
        }
    }

    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
        match num {
            1 => self.abs = r.bytes()?,
            2 => self.negative = r.varint()? != 0,
            _ => r.skip(wt)?,
        }
        Ok(())
    }

    /// A value, as protowire-go reads it: no leading zeros, and no `-0`.
    fn finish(&mut self, _: &Limits) -> Result<()> {
        trim_leading_zeros(&mut self.abs);
        if self.abs.is_empty() {
            self.negative = false;
        }
        Ok(())
    }
}

impl Message for Decimal {
    fn encode_to(&self, w: &mut Writer) {
        let mut unscaled = trimmed(&self.unscaled).to_vec();
        if unscaled.is_empty() {
            return; // a zero at any scale, with no sign
        }
        let max = MAX_NUMERIC_LITERAL_DIGITS as i64;
        let mut scale = i64::from(self.scale);
        // A scale below -MaxNumericLiteralDigits is not reduced: the bound
        // stops scale -2^31 asking for 2 GiB, and such a value is refused
        // below.
        if scale >= -max && scale != 0 {
            reduce(&mut unscaled, &mut scale);
        }
        // A reader refuses a scale past the bound, and so does
        // protowire-go's Marshal.
        if scale > max || scale < -max {
            w.refuse(Error::BigNumber(format!(
                "pxf.Decimal scale {scale} exceeds MaxNumericLiteralDigits={max}"
            )));
            return;
        }
        w.tag(1, WireType::LengthDelimited);
        w.bytes(&unscaled);
        if scale != 0 {
            // `int32 scale = 2`: a plain varint, sign-extended when negative.
            w.tag(2, WireType::Varint);
            w.varint_i64(scale);
        }
        if self.negative {
            w.tag(3, WireType::Varint);
            w.varint(1);
        }
    }

    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
        match num {
            1 => self.unscaled = r.bytes()?,
            // Plain varint: the low 32 bits, read as signed.
            2 => self.scale = r.varint()? as i32,
            3 => self.negative = r.varint()? != 0,
            _ => r.skip(wt)?,
        }
        Ok(())
    }

    /// The scale is a digit count, and a consumer that materialises the
    /// value computes 10^|scale| from five written bytes, so
    /// `max_numeric_literal_digits` bounds it on both signs (HARDENING.md §
    /// Mandatory limits, protowire#279). Then a value, as protowire-go
    /// reads it: no leading zeros, and no `-0`.
    fn finish(&mut self, limits: &Limits) -> Result<()> {
        let max = limits.max_numeric_literal_digits as i64;
        let scale = i64::from(self.scale);
        if scale > max || scale < -max {
            return Err(Error::BigNumber(format!(
                "pxf.Decimal scale {scale} exceeds MaxNumericLiteralDigits={max}"
            )));
        }
        trim_leading_zeros(&mut self.unscaled);
        if self.unscaled.is_empty() {
            self.negative = false;
        }
        Ok(())
    }
}

impl Message for BigFloat {
    fn encode_to(&self, w: &mut Writer) {
        if self.prec == 0 {
            // ±0, and its sign is all there is to write (protowire#410).
            if self.negative {
                w.tag(4, WireType::Varint);
                w.varint(1);
            }
            return;
        }
        // The wire mantissa is the value widened to prec bits, so a reader
        // refuses a prec past the digits a precision suffix may have
        // (protowire#499). Checked first, as protowire-go does, and for a
        // zero too: it carries its prec. The bound also keeps the widening
        // below from allocating what prec asks for.
        let digits = big_float_digits(self.prec);
        if digits > MAX_NUMERIC_LITERAL_DIGITS as u64 {
            w.refuse(Error::BigNumber(format!(
                "pxf.BigFloat prec {} implies {digits} digits; MaxNumericLiteralDigits={}",
                self.prec, MAX_NUMERIC_LITERAL_DIGITS
            )));
            return;
        }
        let prec = i64::from(self.prec);
        let mut mantissa = trimmed(&self.mantissa).to_vec();
        let mut exponent: i64 = 0;
        if !mantissa.is_empty() {
            exponent = i64::from(self.exponent);
            let bits = bit_len(&mantissa);
            if bits > prec {
                shift_right_rounded(&mut mantissa, bits - prec);
                exponent += bits - prec;
            }
            let bits = bit_len(&mantissa);
            // The value is mantissa × 2^widened with a prec-bit mantissa, and
            // its binary exponent is widened + prec, which a round up can
            // carry one past the bound.
            let widened = exponent + bits - prec;
            if widened < i64::from(i32::MIN) || widened > i64::from(i32::MAX) {
                w.refuse(Error::BigNumber(format!(
                    "pxf.BigFloat exponent {widened} does not fit int32"
                )));
                return;
            }
            // A reader refuses a value past MaxBigFloatExponent, so writing
            // one would produce bytes this port's own reader refuses
            // (protowire#281).
            let binary = widened + prec;
            let max = MAX_BIG_FLOAT_EXPONENT as i64;
            if binary > max || binary < -max {
                w.refuse(Error::BigNumber(format!(
                    "pxf.BigFloat binary exponent {binary} exceeds MaxBigFloatExponent={max}"
                )));
                return;
            }
            if bits > prec {
                // The round up carried past the top bit.
                shift_right_rounded(&mut mantissa, bits - prec);
            } else {
                shift_left(&mut mantissa, prec - bits);
            }
            exponent = widened;
        }
        if !mantissa.is_empty() {
            w.tag(1, WireType::LengthDelimited);
            w.bytes(&mantissa);
        }
        if exponent != 0 {
            // `int32 exponent = 2`: a plain varint, as for Decimal.scale.
            w.tag(2, WireType::Varint);
            w.varint_i64(exponent);
        }
        w.tag(3, WireType::Varint);
        w.varint(u64::from(self.prec));
        if self.negative {
            w.tag(4, WireType::Varint);
            w.varint(1);
        }
    }

    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
        match num {
            1 => self.mantissa = r.bytes()?,
            // Plain varint, as for Decimal.scale.
            2 => self.exponent = r.varint()? as i32,
            // uint32: the low 32 bits, as a protobuf reader keeps them
            // (protowire-go#230).
            3 => self.prec = r.varint()? as u32,
            4 => self.negative = r.varint()? != 0,
            _ => r.skip(wt)?,
        }
        Ok(())
    }

    /// The bounds, then the value as protowire-go reads it: a prec of 0 is a
    /// zero signed by `negative`; otherwise prec is bounded by the digits it
    /// implies, the binary exponent by `max_big_float_exponent`, and a
    /// mantissa wider than prec is rounded to it. Widening to prec is left
    /// to the writer, so a read never allocates what five bytes of prec
    /// ask for.
    fn finish(&mut self, limits: &Limits) -> Result<()> {
        if self.prec == 0 {
            self.mantissa.clear();
            self.exponent = 0;
            return Ok(());
        }
        // prec is bounded as a literal's precision suffix is, before anything
        // is built for the value, zeros included: 11 bytes at prec 2^31-1
        // would re-encode to 256 MiB (draft -01 § Mandatory Limits,
        // protowire#499).
        let digits = big_float_digits(self.prec);
        let max_digits = limits.max_numeric_literal_digits;
        if digits > max_digits as u64 {
            return Err(Error::BigNumber(format!(
                "pxf.BigFloat prec {} implies {digits} digits; MaxNumericLiteralDigits={max_digits}",
                self.prec
            )));
        }
        trim_leading_zeros(&mut self.mantissa);
        let bits = bit_len(&self.mantissa);
        if bits == 0 {
            // a zero at any exponent
            self.exponent = 0;
            return Ok(());
        }
        // Rendering the value costs time that grows with the square of its
        // binary exponent, and five written bytes set it, so the bound comes
        // first (HARDENING.md § Mandatory limits, protowire#281).
        let binary = i64::from(self.exponent) + bits;
        let max = limits.max_big_float_exponent as i64;
        if binary > max || binary < -max {
            return Err(Error::BigNumber(format!(
                "pxf.BigFloat binary exponent {binary} exceeds MaxBigFloatExponent={max}"
            )));
        }
        // The value is the mantissa rounded to prec. The bound was on the
        // wire's mantissa, so a round up can land one past it, as in
        // protowire-go.
        let prec = i64::from(self.prec);
        let mut exponent = i64::from(self.exponent);
        if bits > prec {
            shift_right_rounded(&mut self.mantissa, bits - prec);
            exponent += bits - prec;
        }
        // Only a bound raised near i32::MAX reaches this: big.Float's
        // exponent is an int32, and past it the value would be an infinity
        // (protowire#278).
        if exponent + bit_len(&self.mantissa) > i64::from(i32::MAX) {
            return Err(Error::BigNumber(format!(
                "pxf.BigFloat exponent {} with a {bits}-bit mantissa overflows big.Float",
                self.exponent
            )));
        }
        self.exponent = exponent as i32;
        Ok(())
    }
}

/// The decimal digits a BigFloat at `prec` bits renders to: those of 2^prec,
/// plus one. The same count as `protowire_pxf::bigfloat::big_float_digits`,
/// which this crate cannot depend on.
pub fn big_float_digits(prec: u32) -> u64 {
    pow2_digits(prec) + 1
}

/// floor(log10(2) × 2^128), high word first.
const LOG10_OF_2: (u128, u128) = (0x4d10_4d42_7de7_fbcc, 0x47c4_acd6_05be_48bc);

/// The decimal digits of 2^p, floor(p·log10(2)) + 1.
fn pow2_digits(p: u32) -> u64 {
    let p = u128::from(p);
    (((p * LOG10_OF_2.0) + ((p * LOG10_OF_2.1) >> 64)) >> 64) as u64 + 1
}

fn trimmed(mag: &[u8]) -> &[u8] {
    let first = mag.iter().position(|b| *b != 0).unwrap_or(mag.len());
    &mag[first..]
}

fn trim_leading_zeros(mag: &mut Vec<u8>) {
    let first = mag.iter().position(|b| *b != 0).unwrap_or(mag.len());
    mag.drain(..first);
}

/// The bit length of a magnitude with no leading zero byte.
fn bit_len(mag: &[u8]) -> i64 {
    match mag.first() {
        None => 0,
        Some(top) => (mag.len() as i64 - 1) * 8 + i64::from(8 - top.leading_zeros()),
    }
}

/// Add `v` to a big-endian magnitude.
fn add_u8(mag: &mut Vec<u8>, v: u8) {
    let mut carry = u16::from(v);
    for b in mag.iter_mut().rev() {
        if carry == 0 {
            return;
        }
        let sum = u16::from(*b) + carry;
        *b = sum as u8;
        carry = sum >> 8;
    }
    if carry != 0 {
        mag.insert(0, carry as u8);
    }
}

/// Drop the low `n` bits of a magnitude, 0 < n < its bit length, rounding
/// half to even as big.Float does.
fn shift_right_rounded(mag: &mut Vec<u8>, n: i64) {
    let len = mag.len();
    let bit = |m: &[u8], i: i64| (m[len - 1 - (i / 8) as usize] >> (i % 8)) & 1 != 0;
    let half = bit(mag, n - 1);
    let sticky = (0..n - 1).any(|i| bit(mag, i));
    mag.truncate(len - (n / 8) as usize);
    let shift = (n % 8) as u32;
    if shift != 0 {
        for i in (0..mag.len()).rev() {
            let high = if i == 0 { 0 } else { u16::from(mag[i - 1]) };
            mag[i] = (((high << 8) | u16::from(mag[i])) >> shift) as u8;
        }
    }
    if half && (sticky || mag.last().is_some_and(|b| b & 1 != 0)) {
        add_u8(mag, 1);
    }
    trim_leading_zeros(mag);
}

fn shift_left(mag: &mut Vec<u8>, n: i64) {
    let shift = (n % 8) as u32;
    if shift != 0 {
        let mut carry: u16 = 0;
        for b in mag.iter_mut().rev() {
            let v = (u16::from(*b) << shift) | carry;
            *b = v as u8;
            carry = v >> 8;
        }
        if carry != 0 {
            mag.insert(0, carry as u8);
        }
    }
    mag.resize(mag.len() + (n / 8) as usize, 0);
}

// Reduce works on base 2^32 limbs, least significant first, nine digits at a
// time: one digit at a time over bytes took 48 s on a 4 MiB value in
// protowire-cpp, and a message that size must not be able to stall marshal.
const POW10: [u32; 10] = [
    1,
    10,
    100,
    1_000,
    10_000,
    100_000,
    1_000_000,
    10_000_000,
    100_000_000,
    1_000_000_000,
];
const BATCH_DIGITS: i64 = 9;

fn to_limbs(mag: &[u8]) -> Vec<u32> {
    let mut limbs = vec![0u32; mag.len().div_ceil(4)];
    for (i, b) in mag.iter().rev().enumerate() {
        limbs[i / 4] |= u32::from(*b) << (8 * (i % 4));
    }
    limbs
}

fn from_limbs(limbs: &[u32]) -> Vec<u8> {
    let mut mag = Vec::with_capacity(limbs.len() * 4);
    for limb in limbs.iter().rev() {
        mag.extend_from_slice(&limb.to_be_bytes());
    }
    trim_leading_zeros(&mut mag);
    mag
}

fn multiply_limbs(limbs: &mut Vec<u32>, factor: u32) {
    let mut carry: u64 = 0;
    for limb in limbs.iter_mut() {
        let v = u64::from(*limb) * u64::from(factor) + carry;
        *limb = v as u32;
        carry = v >> 32;
    }
    if carry != 0 {
        limbs.push(carry as u32);
    }
}

/// Leaves the quotient in `quotient` and returns the remainder.
fn divide_limbs(limbs: &[u32], divisor: u32, quotient: &mut Vec<u32>) -> u32 {
    quotient.clear();
    quotient.resize(limbs.len(), 0);
    let mut rem: u64 = 0;
    for i in (0..limbs.len()).rev() {
        let cur = (rem << 32) | u64::from(limbs[i]);
        quotient[i] = (cur / u64::from(divisor)) as u32;
        rem = cur % u64::from(divisor);
    }
    while quotient.last() == Some(&0) {
        quotient.pop();
    }
    rem as u32
}

/// Rewrite a non-zero unscaled × 10^(-scale) with the smallest scale >= 0
/// that holds it exactly, which is what a big.Rat comes back as.
fn reduce(unscaled: &mut Vec<u8>, scale: &mut i64) {
    let mut limbs = to_limbs(unscaled);
    while *scale < 0 {
        let step = BATCH_DIGITS.min(-*scale);
        multiply_limbs(&mut limbs, POW10[step as usize]);
        *scale += step;
    }
    let mut quotient = Vec::new();
    for step in [BATCH_DIGITS, 1] {
        while *scale >= step && divide_limbs(&limbs, POW10[step as usize], &mut quotient) == 0 {
            std::mem::swap(&mut limbs, &mut quotient);
            *scale -= step;
        }
    }
    *unscaled = from_limbs(&limbs);
}
