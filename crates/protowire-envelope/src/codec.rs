// SPDX-License-Identifier: MIT
// Copyright (c) 2026 TrendVidia, LLC.
//! Binary protobuf codec for [`Envelope`], [`AppError`], [`FieldError`].
//!
//! Field numbers and wire-format choices match
//! `proto/envelope/v1/envelope.proto` from the canonical Go module:
//!
//! ```text
//! Envelope { status=1 int32, transport_error=2 string, data=3 bytes, error=4 AppError }
//! AppError { code=1 string, message=2 string, args=3 repeated string,
//!            details=4 repeated FieldError, metadata=5 map<string,string> }
//! FieldError { field=1 string, code=2 string, message=3 string, args=4 repeated string }
//! ```
//!
//! Status uses proto3 `int32` (plain varint, sign-extends on negatives) for
//! cross-port byte-equivalence. The envelope has no negative status values
//! in practice, so this is identical to `sint32` on the wire — but we pin
//! the contract regardless.

use protowire_pb::wire::{Reader, Result, WireType, Writer};
use protowire_pb::{read_message, write_message, Message};

use crate::{AppError, Envelope, FieldError};

impl Message for FieldError {
    fn encode_to(&self, w: &mut Writer) {
        if !self.field.is_empty() {
            w.tag(1, WireType::LengthDelimited);
            w.string(&self.field);
        }
        if !self.code.is_empty() {
            w.tag(2, WireType::LengthDelimited);
            w.string(&self.code);
        }
        if !self.message.is_empty() {
            w.tag(3, WireType::LengthDelimited);
            w.string(&self.message);
        }
        for a in &self.args {
            w.tag(4, WireType::LengthDelimited);
            w.string(a);
        }
    }

    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
        match num {
            1 => self.field = r.string()?,
            2 => self.code = r.string()?,
            3 => self.message = r.string()?,
            4 => self.args.push(r.string()?),
            _ => r.skip(wt)?,
        }
        Ok(())
    }
}

impl Message for AppError {
    fn encode_to(&self, w: &mut Writer) {
        if !self.code.is_empty() {
            w.tag(1, WireType::LengthDelimited);
            w.string(&self.code);
        }
        if !self.message.is_empty() {
            w.tag(2, WireType::LengthDelimited);
            w.string(&self.message);
        }
        for a in &self.args {
            w.tag(3, WireType::LengthDelimited);
            w.string(a);
        }
        for fe in &self.details {
            write_message(w, 4, fe);
        }
        // map<string,string> entry: { key=1 string, value=2 string }
        for (k, v) in &self.metadata {
            let mut inner = Writer::new();
            if !k.is_empty() {
                inner.tag(1, WireType::LengthDelimited);
                inner.string(k);
            }
            if !v.is_empty() {
                inner.tag(2, WireType::LengthDelimited);
                inner.string(v);
            }
            let bytes = inner.finish();
            w.tag(5, WireType::LengthDelimited);
            w.varint(bytes.len() as u64);
            w.raw(&bytes);
        }
    }

    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
        match num {
            1 => self.code = r.string()?,
            2 => self.message = r.string()?,
            3 => self.args.push(r.string()?),
            4 => self.details.push(read_message(r)?),
            5 => {
                let len = r.varint()? as usize;
                let end = r.pos + len;
                let mut k = String::new();
                let mut v = String::new();
                while r.pos < end {
                    let (n, ewt) = r.tag()?;
                    match n {
                        1 => k = r.string()?,
                        2 => v = r.string()?,
                        _ => r.skip(ewt)?,
                    }
                }
                self.metadata.insert(k, v);
            }
            _ => r.skip(wt)?,
        }
        Ok(())
    }
}

impl Message for Envelope {
    fn encode_to(&self, w: &mut Writer) {
        if self.status != 0 {
            w.tag(1, WireType::Varint);
            w.varint_i32(self.status);
        }
        if !self.transport_error.is_empty() {
            w.tag(2, WireType::LengthDelimited);
            w.string(&self.transport_error);
        }
        if !self.data.is_empty() {
            w.tag(3, WireType::LengthDelimited);
            w.bytes(&self.data);
        }
        if let Some(ref err) = self.error {
            write_message(w, 4, err);
        }
    }

    fn merge_field(&mut self, num: u32, wt: WireType, r: &mut Reader<'_>) -> Result<()> {
        match num {
            1 => self.status = r.varint()? as i32,
            2 => self.transport_error = r.string()?,
            3 => self.data = r.bytes()?,
            4 => self.error = Some(read_message(r)?),
            _ => r.skip(wt)?,
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::new_app_error;
    use protowire_pb::{marshal, unmarshal};

    fn s(v: &str) -> String {
        v.to_string()
    }

    // --- FieldError ---

    #[test]
    fn field_error_round_trips_all_fields() {
        let fe = FieldError::new(
            "email",
            "FORMAT",
            "bad email",
            vec![s("user@bad"), s("tld")],
        );
        let bytes = marshal(&fe);
        let got: FieldError = unmarshal(&bytes).unwrap();
        assert_eq!(got, fe);
    }

    #[test]
    fn field_error_zero_value_marshals_to_empty_bytes() {
        let bytes = marshal(&FieldError::default());
        assert!(bytes.is_empty());
        let got: FieldError = unmarshal(&bytes).unwrap();
        assert_eq!(got, FieldError::default());
    }

    // --- AppError ---

    #[test]
    fn app_error_round_trips_nested_details_and_metadata() {
        let mut ae = new_app_error("VALIDATION", "fields invalid", vec![s("ctx")]);
        ae.with_field("email", "FORMAT", "bad", vec![s("u@bad")])
            .with_field("age", "RANGE", "", vec![])
            .with_meta("region", "us-east")
            .with_meta("retry_after", "30");

        let bytes = marshal(&ae);
        let got: AppError = unmarshal(&bytes).unwrap();
        assert_eq!(got.code, "VALIDATION");
        assert_eq!(got.message, "fields invalid");
        assert_eq!(got.args, vec![s("ctx")]);
        assert_eq!(got.details.len(), 2);
        assert_eq!(got.details[0].field, "email");
        assert_eq!(got.details[0].args, vec![s("u@bad")]);
        assert_eq!(got.details[1].field, "age");
        assert_eq!(
            got.metadata.get("region").map(String::as_str),
            Some("us-east")
        );
        assert_eq!(
            got.metadata.get("retry_after").map(String::as_str),
            Some("30")
        );
    }

    // --- Envelope ---

    #[test]
    fn envelope_ok_preserves_data_payload() {
        let env = Envelope::ok(200, vec![0xde, 0xad, 0xbe, 0xef]);
        let bytes = marshal(&env);
        let got: Envelope = unmarshal(&bytes).unwrap();
        assert_eq!(got.status, 200);
        assert_eq!(got.data, vec![0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(got.transport_error, "");
        assert!(got.error.is_none());
        assert!(got.is_ok());
    }

    #[test]
    fn envelope_error_preserves_nested_app_error_with_details_and_metadata() {
        let mut ae = new_app_error(
            "INSUFFICIENT_FUNDS",
            "balance too low",
            vec![s("$3.50"), s("$10.00")],
        );
        ae.with_field("amount", "MIN_VALUE", "below minimum", vec![s("10.00")])
            .with_meta("request_id", "req-123");
        let env = Envelope {
            status: 402,
            error: Some(ae),
            ..Default::default()
        };

        let bytes = marshal(&env);
        let got: Envelope = unmarshal(&bytes).unwrap();
        assert_eq!(got.status, 402);
        assert!(got.is_app_error());
        assert_eq!(got.error_code(), "INSUFFICIENT_FUNDS");
        let err = got.error.as_ref().unwrap();
        assert_eq!(err.args, vec![s("$3.50"), s("$10.00")]);
        assert_eq!(err.details.len(), 1);
        assert_eq!(err.details[0].code, "MIN_VALUE");
        assert_eq!(
            err.metadata.get("request_id").map(String::as_str),
            Some("req-123")
        );
    }

    #[test]
    fn envelope_transport_error_round_trips() {
        let env = Envelope::transport_err("connection refused");
        let bytes = marshal(&env);
        let got: Envelope = unmarshal(&bytes).unwrap();
        assert!(got.is_transport_error());
        assert_eq!(got.transport_error, "connection refused");
        assert_eq!(got.status, 0);
        assert!(got.error.is_none());
    }

    #[test]
    fn zero_envelope_marshals_to_empty_bytes() {
        let bytes = marshal(&Envelope::default());
        assert!(bytes.is_empty());
        let got: Envelope = unmarshal(&bytes).unwrap();
        assert!(got.is_ok());
    }

    #[test]
    fn envelope_preserves_data_integrity_across_nested_message_boundary() {
        // 1KB payload exercises length-prefix off-by-one paths.
        let mut payload = vec![0u8; 1024];
        for (i, b) in payload.iter_mut().enumerate() {
            *b = ((i as u32 * 37) & 0xff) as u8;
        }
        let env = Envelope::ok(200, payload.clone());
        let bytes = marshal(&env);
        let got: Envelope = unmarshal(&bytes).unwrap();
        assert_eq!(got.data, payload);
    }

    // --- Cross-port wire compatibility ---

    /// Canonical envelope from `protowire/scripts/dump_envelope/main.go`.
    /// Every port (Go/C++/TS/Java/Rust) must produce these exact bytes.
    const CANONICAL_HEX: &str =
        "0892031a04deadbeef22760a12494e53554646494349454e545f46554e4453120f62616c\
         616e636520746f6f206c6f771a0524332e35301a062431302e303022290a06616d6f756e\
         7412094d494e5f56414c55451a0d62656c6f77206d696e696d756d220531302e30302a15\
         0a0a726571756573745f696412077265712d313233";

    fn canonical_envelope() -> Envelope {
        let mut env = Envelope::err(
            402,
            "INSUFFICIENT_FUNDS",
            "balance too low",
            vec![s("$3.50"), s("$10.00")],
        );
        env.data = vec![0xde, 0xad, 0xbe, 0xef];
        env.error
            .as_mut()
            .unwrap()
            .with_field("amount", "MIN_VALUE", "below minimum", vec![s("10.00")])
            .with_meta("request_id", "req-123");
        env
    }

    fn hex_decode(s: &str) -> Vec<u8> {
        let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        (0..cleaned.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&cleaned[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn canonical_envelope_matches_cross_port_hex() {
        let bytes = marshal(&canonical_envelope());
        assert_eq!(bytes, hex_decode(CANONICAL_HEX));
    }

    #[test]
    fn canonical_envelope_unmarshals_from_cross_port_hex() {
        let bytes = hex_decode(CANONICAL_HEX);
        let got: Envelope = unmarshal(&bytes).unwrap();
        let want = canonical_envelope();
        assert_eq!(got.status, want.status);
        assert_eq!(got.data, want.data);
        let g_err = got.error.as_ref().unwrap();
        let w_err = want.error.as_ref().unwrap();
        assert_eq!(g_err.code, w_err.code);
        assert_eq!(g_err.message, w_err.message);
        assert_eq!(g_err.args, w_err.args);
        assert_eq!(g_err.details, w_err.details);
        assert_eq!(g_err.metadata, w_err.metadata);
    }
}
