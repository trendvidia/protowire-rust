//! Schema-bound PXF decoder.
//!
//! Slice D1: scalars, enums, nested messages, repeated lists, oneof.
//! Mirrors the AST-based path in `protowire/encoding/pxf/decode_fast.go` and
//! the TS port's `pxf/decode.ts`, without the fused single-pass perf shortcut.
//!
//! Maps, well-known types (Timestamp/Duration/wrappers), `google.protobuf.Any`,
//! and the `Result`-tracking `unmarshal_full` (required/default/_null) land in
//! later D-slices.
//!
//! The decoder walks the input alongside a `MessageDescriptor` and writes
//! directly into a `prost_reflect::DynamicMessage`. No intermediate AST.

use prost_reflect::{
    DynamicMessage, FieldDescriptor, Kind, MessageDescriptor, OneofDescriptor, ReflectMessage,
    Value,
};
use std::collections::HashMap;

use crate::errors::PxfError;
use crate::lexer::Lexer;
use crate::token::{Position, Token, TokenKind};

/// Options controlling [`unmarshal`] behavior.
#[derive(Default, Clone, Copy, Debug)]
pub struct UnmarshalOptions {
    /// Silently skip fields not declared in the schema instead of erroring.
    pub discard_unknown: bool,
}

/// Decode PXF text into a fresh [`DynamicMessage`] for `desc`.
pub fn unmarshal(
    data: &str,
    desc: &MessageDescriptor,
    options: UnmarshalOptions,
) -> Result<DynamicMessage, PxfError> {
    let mut decoder = Decoder::new(data, options.discard_unknown);
    decoder.advance();

    if matches!(decoder.current.kind, TokenKind::AtType) {
        decoder.advance();
        if !matches!(decoder.current.kind, TokenKind::Ident) {
            return Err(decoder.err(format!(
                "expected type name after @type, got {}",
                decoder.current.kind
            )));
        }
        decoder.advance();
    }

    let mut msg = DynamicMessage::new(desc.clone());
    decoder.decode_fields(&mut msg, false)?;
    Ok(msg)
}

struct Decoder<'a> {
    lex: Lexer<'a>,
    current: Token,
    discard_unknown: bool,
}

impl<'a> Decoder<'a> {
    fn new(input: &'a str, discard_unknown: bool) -> Self {
        Self {
            lex: Lexer::new(input),
            current: Token::new(TokenKind::Eof, "", Position::new(1, 1)),
            discard_unknown,
        }
    }

    fn advance(&mut self) {
        loop {
            self.current = self.lex.next_token();
            if !matches!(self.current.kind, TokenKind::Comment | TokenKind::Newline) {
                return;
            }
        }
    }

    fn err(&self, msg: impl Into<String>) -> PxfError {
        PxfError::new(self.current.pos, msg)
    }

    fn err_at(&self, pos: Position, msg: impl Into<String>) -> PxfError {
        PxfError::new(pos, msg)
    }

    fn decode_fields(
        &mut self,
        msg: &mut DynamicMessage,
        in_block: bool,
    ) -> Result<(), PxfError> {
        let desc = msg.descriptor();
        let mut set_oneofs: HashMap<String, String> = HashMap::new();

        loop {
            if in_block && matches!(self.current.kind, TokenKind::RBrace) {
                self.advance();
                return Ok(());
            }
            if matches!(self.current.kind, TokenKind::Eof) {
                if in_block {
                    return Err(self.err("expected '}', got EOF"));
                }
                return Ok(());
            }

            let pos = self.current.pos;
            let key_kind = self.current.kind;
            if !matches!(
                key_kind,
                TokenKind::Ident | TokenKind::String | TokenKind::Int
            ) {
                return Err(self.err_at(
                    pos,
                    format!(
                        "expected identifier, string, or integer, got {} ({:?})",
                        key_kind, self.current.value
                    ),
                ));
            }
            let key = self.current.value.clone();
            self.advance();

            match self.current.kind {
                TokenKind::Equals => {
                    self.advance();
                    let fd = match desc.get_field_by_name(&key) {
                        Some(fd) => fd,
                        None => {
                            if self.discard_unknown {
                                self.skip_value();
                                continue;
                            }
                            return Err(self.err_at(
                                pos,
                                format!("unknown field {:?} in {}", key, desc.full_name()),
                            ));
                        }
                    };
                    self.check_oneof(&fd, &mut set_oneofs, pos)?;
                    if matches!(self.current.kind, TokenKind::Null) {
                        self.advance();
                        continue;
                    }
                    self.decode_field_value(msg, &fd)?;
                }
                TokenKind::LBrace => {
                    self.advance();
                    let fd = match desc.get_field_by_name(&key) {
                        Some(fd) => fd,
                        None => {
                            if self.discard_unknown {
                                self.skip_braced();
                                continue;
                            }
                            return Err(self.err_at(
                                pos,
                                format!("unknown field {:?} in {}", key, desc.full_name()),
                            ));
                        }
                    };
                    if fd.is_list() {
                        return Err(self.err_at(
                            pos,
                            format!(
                                "repeated field {:?} must use list syntax: {} = [...]",
                                key, key
                            ),
                        ));
                    }
                    if fd.is_map() {
                        return Err(self.err_at(
                            pos,
                            format!(
                                "map field {:?} must use assignment syntax: {} = {{ ... }}",
                                key, key
                            ),
                        ));
                    }
                    if !matches!(fd.kind(), Kind::Message(_)) {
                        return Err(self.err_at(
                            pos,
                            format!(
                                "field {:?} is not a message type, cannot use block syntax",
                                key
                            ),
                        ));
                    }
                    self.check_oneof(&fd, &mut set_oneofs, pos)?;
                    let inner_desc = match fd.kind() {
                        Kind::Message(m) => m,
                        _ => unreachable!(),
                    };
                    let mut sub = DynamicMessage::new(inner_desc);
                    self.decode_fields(&mut sub, true)?;
                    msg.set_field(&fd, Value::Message(sub));
                }
                TokenKind::Colon => {
                    return Err(self.err_at(
                        pos,
                        "unexpected ':' in message context, use '=' for field assignments",
                    ));
                }
                _ => {
                    return Err(self.err(format!(
                        "expected '=', ':', or '{{' after {:?}, got {}",
                        key, self.current.kind
                    )));
                }
            }
        }
    }

    fn check_oneof(
        &self,
        fd: &FieldDescriptor,
        set_oneofs: &mut HashMap<String, String>,
        pos: Position,
    ) -> Result<(), PxfError> {
        let oo: Option<OneofDescriptor> = fd.containing_oneof();
        let Some(oo) = oo else { return Ok(()) };
        // Synthetic oneofs (proto3 optional) wrap a single field, so they can
        // never produce a real conflict — leaving them in the map is harmless.
        let name = oo.name().to_string();
        if let Some(prev) = set_oneofs.get(&name) {
            return Err(self.err_at(
                pos,
                format!(
                    "oneof {:?}: field {:?} conflicts with already-set field {:?}",
                    name,
                    fd.name(),
                    prev
                ),
            ));
        }
        set_oneofs.insert(name, fd.name().to_string());
        Ok(())
    }

    fn decode_field_value(
        &mut self,
        msg: &mut DynamicMessage,
        fd: &FieldDescriptor,
    ) -> Result<(), PxfError> {
        if fd.is_map() {
            return Err(self.err(format!(
                "map field {:?} not supported in this slice",
                fd.name()
            )));
        }
        if fd.is_list() {
            return self.decode_list_inline(msg, fd);
        }
        if let Kind::Message(inner_desc) = fd.kind() {
            if !matches!(self.current.kind, TokenKind::LBrace) {
                return Err(self.err(format!(
                    "expected '{{' for message field {:?}",
                    fd.name()
                )));
            }
            self.advance();
            let mut sub = DynamicMessage::new(inner_desc);
            self.decode_fields(&mut sub, true)?;
            msg.set_field(fd, Value::Message(sub));
            return Ok(());
        }
        if matches!(fd.kind(), Kind::Enum(_)) {
            let v = self.consume_enum(fd)?;
            msg.set_field(fd, v);
            return Ok(());
        }
        let v = self.consume_scalar(fd)?;
        msg.set_field(fd, v);
        Ok(())
    }

    fn decode_list_inline(
        &mut self,
        msg: &mut DynamicMessage,
        fd: &FieldDescriptor,
    ) -> Result<(), PxfError> {
        if !matches!(self.current.kind, TokenKind::LBracket) {
            return Err(self.err(format!(
                "expected '[' for repeated field {:?}",
                fd.name()
            )));
        }
        self.advance();

        let mut elems: Vec<Value> = Vec::new();
        let element_kind = fd.kind();

        while !matches!(self.current.kind, TokenKind::RBracket | TokenKind::Eof) {
            if matches!(self.current.kind, TokenKind::Null) {
                return Err(self.err(format!(
                    "null is not allowed in repeated field {:?}",
                    fd.name()
                )));
            }
            let v = match &element_kind {
                Kind::Message(inner_desc) => {
                    if !matches!(self.current.kind, TokenKind::LBrace) {
                        return Err(self.err("expected '{' for repeated message element"));
                    }
                    self.advance();
                    let mut sub = DynamicMessage::new(inner_desc.clone());
                    self.decode_fields(&mut sub, true)?;
                    Value::Message(sub)
                }
                Kind::Enum(_) => self.consume_enum(fd)?,
                _ => self.consume_scalar(fd)?,
            };
            elems.push(v);
            if matches!(self.current.kind, TokenKind::Comma) {
                self.advance();
            }
        }

        if !matches!(self.current.kind, TokenKind::RBracket) {
            return Err(self.err(format!("expected ']', got {}", self.current.kind)));
        }
        self.advance();

        msg.set_field(fd, Value::List(elems));
        Ok(())
    }

    fn consume_scalar(&mut self, fd: &FieldDescriptor) -> Result<Value, PxfError> {
        let pos = self.current.pos;
        let kind = fd.kind();
        match kind {
            Kind::String => {
                if !matches!(self.current.kind, TokenKind::String) {
                    return Err(
                        self.err_at(pos, format!("expected string for field {:?}", fd.name()))
                    );
                }
                let v = Value::String(self.current.value.clone());
                self.advance();
                Ok(v)
            }
            Kind::Bool => {
                if !matches!(self.current.kind, TokenKind::Bool) {
                    return Err(
                        self.err_at(pos, format!("expected bool for field {:?}", fd.name()))
                    );
                }
                let v = Value::Bool(self.current.value == "true");
                self.advance();
                Ok(v)
            }
            Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 => {
                if !matches!(self.current.kind, TokenKind::Int) {
                    return Err(self.err_at(
                        pos,
                        format!("expected integer for field {:?}", fd.name()),
                    ));
                }
                let n: i32 = self
                    .current
                    .value
                    .parse()
                    .map_err(|_| self.err_at(pos, format!("invalid int32: {}", self.current.value)))?;
                self.advance();
                Ok(Value::I32(n))
            }
            Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 => {
                if !matches!(self.current.kind, TokenKind::Int) {
                    return Err(self.err_at(
                        pos,
                        format!("expected integer for field {:?}", fd.name()),
                    ));
                }
                let n: i64 = self
                    .current
                    .value
                    .parse()
                    .map_err(|_| self.err_at(pos, format!("invalid int64: {}", self.current.value)))?;
                self.advance();
                Ok(Value::I64(n))
            }
            Kind::Uint32 | Kind::Fixed32 => {
                if !matches!(self.current.kind, TokenKind::Int) {
                    return Err(self.err_at(
                        pos,
                        format!("expected integer for field {:?}", fd.name()),
                    ));
                }
                let n: u32 = self.current.value.parse().map_err(|_| {
                    self.err_at(pos, format!("invalid uint32: {}", self.current.value))
                })?;
                self.advance();
                Ok(Value::U32(n))
            }
            Kind::Uint64 | Kind::Fixed64 => {
                if !matches!(self.current.kind, TokenKind::Int) {
                    return Err(self.err_at(
                        pos,
                        format!("expected integer for field {:?}", fd.name()),
                    ));
                }
                let n: u64 = self.current.value.parse().map_err(|_| {
                    self.err_at(pos, format!("invalid uint64: {}", self.current.value))
                })?;
                self.advance();
                Ok(Value::U64(n))
            }
            Kind::Float => {
                if !matches!(self.current.kind, TokenKind::Float | TokenKind::Int) {
                    return Err(self.err_at(
                        pos,
                        format!("expected number for field {:?}", fd.name()),
                    ));
                }
                let f: f32 = self.current.value.parse().map_err(|_| {
                    self.err_at(pos, format!("invalid float: {}", self.current.value))
                })?;
                self.advance();
                Ok(Value::F32(f))
            }
            Kind::Double => {
                if !matches!(self.current.kind, TokenKind::Float | TokenKind::Int) {
                    return Err(self.err_at(
                        pos,
                        format!("expected number for field {:?}", fd.name()),
                    ));
                }
                let f: f64 = self.current.value.parse().map_err(|_| {
                    self.err_at(pos, format!("invalid double: {}", self.current.value))
                })?;
                self.advance();
                Ok(Value::F64(f))
            }
            Kind::Bytes => {
                if !matches!(self.current.kind, TokenKind::Bytes) {
                    return Err(
                        self.err_at(pos, format!("expected bytes for field {:?}", fd.name()))
                    );
                }
                let decoded = decode_base64(&self.current.value).ok_or_else(|| {
                    self.err_at(
                        pos,
                        format!("invalid base64 for field {:?}", fd.name()),
                    )
                })?;
                self.advance();
                Ok(Value::Bytes(decoded.into()))
            }
            Kind::Enum(_) => self.consume_enum(fd),
            Kind::Message(_) => Err(self.err_at(
                pos,
                format!(
                    "internal: consume_scalar called on message field {:?}",
                    fd.name()
                ),
            )),
        }
    }

    fn consume_enum(&mut self, fd: &FieldDescriptor) -> Result<Value, PxfError> {
        let pos = self.current.pos;
        let enum_desc = match fd.kind() {
            Kind::Enum(e) => e,
            _ => {
                return Err(self.err_at(
                    pos,
                    format!("internal: consume_enum on non-enum field {:?}", fd.name()),
                ))
            }
        };
        match self.current.kind {
            TokenKind::Ident => {
                let ev = enum_desc.get_value_by_name(&self.current.value).ok_or_else(|| {
                    self.err_at(
                        pos,
                        format!(
                            "unknown enum value {:?} for {}",
                            self.current.value,
                            enum_desc.full_name()
                        ),
                    )
                })?;
                self.advance();
                Ok(Value::EnumNumber(ev.number()))
            }
            TokenKind::Int => {
                let n: i32 = self.current.value.parse().map_err(|_| {
                    self.err_at(
                        pos,
                        format!("invalid enum number: {}", self.current.value),
                    )
                })?;
                self.advance();
                Ok(Value::EnumNumber(n))
            }
            _ => Err(self.err_at(
                pos,
                format!(
                    "expected enum name or number for field {:?}",
                    fd.name()
                ),
            )),
        }
    }

    fn skip_value(&mut self) {
        match self.current.kind {
            TokenKind::LBrace => {
                self.advance();
                self.skip_braced();
            }
            TokenKind::LBracket => {
                self.advance();
                self.skip_bracketed();
            }
            _ => self.advance(),
        }
    }

    fn skip_braced(&mut self) {
        let mut depth: i32 = 1;
        while depth > 0 && !matches!(self.current.kind, TokenKind::Eof) {
            match self.current.kind {
                TokenKind::LBrace => depth += 1,
                TokenKind::RBrace => depth -= 1,
                _ => {}
            }
            self.advance();
        }
    }

    fn skip_bracketed(&mut self) {
        let mut depth: i32 = 1;
        while depth > 0 && !matches!(self.current.kind, TokenKind::Eof) {
            match self.current.kind {
                TokenKind::LBracket => depth += 1,
                TokenKind::RBracket => depth -= 1,
                _ => {}
            }
            self.advance();
        }
    }
}

/// Decode a base64 string, accepting both standard (padded) and raw (unpadded)
/// alphabets — matching the Go reference's `StdEncoding`/`RawStdEncoding`
/// fallback. The lexer has already validated the alphabet, so we only need to
/// handle padding here.
fn decode_base64(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() {
        return Some(Vec::new());
    }
    let bytes = s.as_bytes();
    let mut padded: Vec<u8>;
    let input: &[u8] = if bytes.len() % 4 == 0 {
        bytes
    } else {
        padded = Vec::with_capacity(bytes.len() + 4);
        padded.extend_from_slice(bytes);
        while padded.len() % 4 != 0 {
            padded.push(b'=');
        }
        &padded
    };
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for &b in input {
        if b == b'=' {
            break;
        }
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}
