//! Standard API response envelope for cross-system communication.
//!
//! Separates transport errors from application errors and carries
//! machine-readable codes with positional format arguments for
//! client-side localization.
//!
//! Wire format mirrors the Go `protowire/envelope` package: field tags
//! 1..N on each struct map to a binary protobuf wire format. The binary
//! codec (driven by `protowire-pb`) lands alongside this module once
//! the `pb` slice is in; this file defines the data shapes, builders,
//! and queries.

use std::collections::HashMap;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldError {
    pub field: String,
    pub code: String,
    pub message: String,
    pub args: Vec<String>,
}

impl FieldError {
    pub fn new(
        field: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
        args: Vec<String>,
    ) -> Self {
        Self {
            field: field.into(),
            code: code.into(),
            message: message.into(),
            args,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppError {
    pub code: String,
    pub message: String,
    pub args: Vec<String>,
    pub details: Vec<FieldError>,
    pub metadata: HashMap<String, String>,
}

impl AppError {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        args: Vec<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            args,
            details: Vec::new(),
            metadata: HashMap::new(),
        }
    }

    pub fn with_field(
        &mut self,
        field: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
        args: Vec<String>,
    ) -> &mut Self {
        self.details
            .push(FieldError::new(field, code, message, args));
        self
    }

    pub fn with_meta(
        &mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> &mut Self {
        self.metadata.insert(key.into(), value.into());
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Envelope {
    pub status: i32,
    pub transport_error: String,
    pub data: Vec<u8>,
    pub error: Option<AppError>,
}

impl Envelope {
    pub fn ok(status: i32, data: Vec<u8>) -> Self {
        Self {
            status,
            data,
            ..Default::default()
        }
    }

    pub fn err(
        status: i32,
        code: impl Into<String>,
        message: impl Into<String>,
        args: Vec<String>,
    ) -> Self {
        Self {
            status,
            error: Some(AppError::new(code, message, args)),
            ..Default::default()
        }
    }

    pub fn transport_err(message: impl Into<String>) -> Self {
        Self {
            transport_error: message.into(),
            ..Default::default()
        }
    }

    pub fn is_ok(&self) -> bool {
        self.transport_error.is_empty() && self.error.is_none()
    }

    pub fn is_transport_error(&self) -> bool {
        !self.transport_error.is_empty()
    }

    pub fn is_app_error(&self) -> bool {
        self.error.is_some()
    }

    pub fn error_code(&self) -> &str {
        self.error.as_ref().map(|e| e.code.as_str()).unwrap_or("")
    }

    /// Returns field errors indexed by field name, or `None` when there are none.
    ///
    /// Mirrors the Go API which returns nil for both "no app error" and
    /// "app error has no details" — callers should treat `None` as empty.
    pub fn field_errors(&self) -> Option<HashMap<&str, &FieldError>> {
        let err = self.error.as_ref()?;
        if err.details.is_empty() {
            return None;
        }
        let mut out = HashMap::with_capacity(err.details.len());
        for fe in &err.details {
            out.insert(fe.field.as_str(), fe);
        }
        Some(out)
    }
}

pub fn new_app_error(
    code: impl Into<String>,
    message: impl Into<String>,
    args: Vec<String>,
) -> AppError {
    AppError::new(code, message, args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> String {
        v.to_string()
    }

    #[test]
    fn ok_envelope_is_ok_and_has_no_errors() {
        let e = Envelope::ok(200, vec![1, 2, 3]);
        assert_eq!(e.status, 200);
        assert_eq!(e.data, vec![1, 2, 3]);
        assert_eq!(e.transport_error, "");
        assert!(e.error.is_none());
        assert!(e.is_ok());
        assert!(!e.is_transport_error());
        assert!(!e.is_app_error());
        assert_eq!(e.error_code(), "");
    }

    #[test]
    fn err_carries_app_error_and_is_not_ok() {
        let e = Envelope::err(400, "INVALID", "bad input", vec![s("name"), s("too short")]);
        assert_eq!(e.status, 400);
        let ae = e.error.as_ref().expect("error set");
        assert_eq!(ae.code, "INVALID");
        assert_eq!(ae.message, "bad input");
        assert_eq!(ae.args, vec![s("name"), s("too short")]);
        assert!(!e.is_ok());
        assert!(!e.is_transport_error());
        assert!(e.is_app_error());
        assert_eq!(e.error_code(), "INVALID");
    }

    #[test]
    fn err_works_without_args() {
        let e = Envelope::err(500, "OOPS", "", vec![]);
        let ae = e.error.as_ref().unwrap();
        assert_eq!(ae.code, "OOPS");
        assert_eq!(ae.message, "");
        assert!(ae.args.is_empty());
    }

    #[test]
    fn transport_err_carries_transport_error() {
        let e = Envelope::transport_err("connection refused");
        assert_eq!(e.transport_error, "connection refused");
        assert!(e.error.is_none());
        assert!(!e.is_ok());
        assert!(e.is_transport_error());
        assert!(!e.is_app_error());
        assert_eq!(e.error_code(), "");
    }

    #[test]
    fn with_field_appends_field_errors_via_chain() {
        let mut ae = new_app_error("VALIDATION", "fields invalid", vec![]);
        ae.with_field("email", "FORMAT", "invalid email", vec![s("user@bad")])
            .with_field("age", "RANGE", "must be positive", vec![]);
        assert_eq!(ae.details.len(), 2);
        assert_eq!(ae.details[0].field, "email");
        assert_eq!(ae.details[0].args, vec![s("user@bad")]);
        assert_eq!(ae.details[1].field, "age");
        assert!(ae.details[1].args.is_empty());
    }

    #[test]
    fn with_meta_sets_metadata_entries_via_chain() {
        let mut ae = new_app_error("X", "", vec![]);
        ae.with_meta("region", "us-east").with_meta("tier", "free");
        assert_eq!(ae.metadata.len(), 2);
        assert_eq!(ae.metadata.get("region").map(String::as_str), Some("us-east"));
        assert_eq!(ae.metadata.get("tier").map(String::as_str), Some("free"));
    }

    #[test]
    fn field_errors_none_when_no_app_error() {
        assert!(Envelope::ok(200, vec![]).field_errors().is_none());
        assert!(Envelope::transport_err("nope").field_errors().is_none());
    }

    #[test]
    fn field_errors_none_when_app_error_has_no_details() {
        assert!(Envelope::err(400, "BAD", "", vec![]).field_errors().is_none());
    }

    #[test]
    fn field_errors_indexes_by_field_name() {
        let mut ae = new_app_error("VALIDATION", "", vec![]);
        ae.with_field("email", "FORMAT", "", vec![])
            .with_field("age", "RANGE", "", vec![]);
        let e = Envelope {
            status: 400,
            error: Some(ae),
            ..Default::default()
        };
        let idx = e.field_errors().expect("indexed");
        let mut keys: Vec<&str> = idx.keys().copied().collect();
        keys.sort();
        assert_eq!(keys, vec!["age", "email"]);
        assert_eq!(idx.get("email").unwrap().code, "FORMAT");
        assert_eq!(idx.get("age").unwrap().code, "RANGE");
    }

    #[test]
    fn default_envelope_is_ok_equivalent() {
        let e = Envelope::default();
        assert_eq!(e.status, 0);
        assert_eq!(e.transport_error, "");
        assert!(e.data.is_empty());
        assert!(e.error.is_none());
        assert!(e.is_ok());
    }

    #[test]
    fn default_app_error_has_empty_message_args_details_metadata() {
        let ae = AppError::new("CODE", "", vec![]);
        assert_eq!(ae.message, "");
        assert!(ae.args.is_empty());
        assert!(ae.details.is_empty());
        assert!(ae.metadata.is_empty());
    }
}
