//! Tokenizer for PXF (Proto eXpressive Format).
//!
//! Mirrors `protowire/encoding/pxf/lexer.go` byte-for-byte. Operates on the
//! UTF-8 bytes of the input; identifiers and keywords are ASCII, while
//! string contents may contain arbitrary UTF-8 (copied through unchanged).
//!
//! Recognizes:
//!  - Comments: `# ...`, `// ...`, `/* ... */`
//!  - Strings: `"..."` (with `\"`, `\\`, `\n`, `\t`, `\r` escapes) and
//!    `"""..."""` triple-quoted with closing-line indent dedent
//!  - Bytes: `b"<base64>"` (standard or raw, validated at lex time)
//!  - Integers, floats (with optional sign and exponent)
//!  - RFC 3339 timestamps: 4 digits + `-` triggers timestamp lex; validated
//!  - Go-style durations: digits + a unit letter (h/m/s/ns/us/ms); validated
//!  - Identifiers (with `.` allowed for dotted package names), `true` /
//!    `false` / `null` keywords, `@type` directive
//!  - Punctuation: `{ } [ ] = : ,`

use crate::token::{Position, Token, TokenKind};

pub struct Lexer<'a> {
    input: &'a [u8],
    pos: usize,
    line: usize,
    col: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    /// Returns the next token. Returns an EOF token at end-of-input forever.
    pub fn next_token(&mut self) -> Token {
        self.skip_spaces();
        if self.pos >= self.input.len() {
            return Token::new(TokenKind::Eof, "", self.current_pos());
        }

        let pos = self.current_pos();
        let ch = self.peek();

        if ch == b'\n' {
            self.advance();
            return Token::new(TokenKind::Newline, "", pos);
        }

        if ch == b'#' {
            return self.lex_line_comment(pos);
        }
        if ch == b'/' && self.peek_at(1) == b'/' {
            return self.lex_line_comment(pos);
        }
        if ch == b'/' && self.peek_at(1) == b'*' {
            return self.lex_block_comment(pos);
        }

        if ch == b'"' {
            if self.peek_at(1) == b'"' && self.peek_at(2) == b'"' {
                return self.lex_triple_string(pos);
            }
            return self.lex_string(pos);
        }
        if ch == b'b' && self.peek_at(1) == b'"' {
            return self.lex_bytes(pos);
        }

        match ch {
            b'{' => {
                self.advance();
                return Token::new(TokenKind::LBrace, "{", pos);
            }
            b'}' => {
                self.advance();
                return Token::new(TokenKind::RBrace, "}", pos);
            }
            b'[' => {
                self.advance();
                return Token::new(TokenKind::LBracket, "[", pos);
            }
            b']' => {
                self.advance();
                return Token::new(TokenKind::RBracket, "]", pos);
            }
            b'=' => {
                self.advance();
                return Token::new(TokenKind::Equals, "=", pos);
            }
            b':' => {
                self.advance();
                return Token::new(TokenKind::Colon, ":", pos);
            }
            b',' => {
                self.advance();
                return Token::new(TokenKind::Comma, ",", pos);
            }
            b'@' => return self.lex_directive(pos),
            _ => {}
        }

        if ch == b'-' || is_digit(ch) {
            return self.lex_number(pos);
        }
        if is_ident_start(ch) {
            return self.lex_ident(pos);
        }

        self.advance();
        Token::new(TokenKind::Illegal, byte_as_string(ch), pos)
    }

    /// Drains the lexer into a Vec, including a terminating EOF token.
    pub fn collect_tokens(mut self) -> Vec<Token> {
        let mut out = Vec::new();
        loop {
            let t = self.next_token();
            let is_eof = matches!(t.kind, TokenKind::Eof);
            out.push(t);
            if is_eof {
                return out;
            }
        }
    }

    fn peek(&self) -> u8 {
        if self.pos >= self.input.len() {
            0
        } else {
            self.input[self.pos]
        }
    }

    fn peek_at(&self, offset: usize) -> u8 {
        let i = self.pos + offset;
        if i >= self.input.len() {
            0
        } else {
            self.input[i]
        }
    }

    fn advance(&mut self) -> u8 {
        if self.pos >= self.input.len() {
            return 0;
        }
        let ch = self.input[self.pos];
        self.pos += 1;
        if ch == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        ch
    }

    fn current_pos(&self) -> Position {
        Position::new(self.line, self.col)
    }

    fn skip_spaces(&mut self) {
        while self.pos < self.input.len() {
            let ch = self.input[self.pos];
            if ch == b' ' || ch == b'\t' || ch == b'\r' {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn lex_line_comment(&mut self, pos: Position) -> Token {
        let start = self.pos;
        while self.pos < self.input.len() && self.input[self.pos] != b'\n' {
            self.advance();
        }
        Token::new(TokenKind::Comment, slice_to_string(&self.input[start..self.pos]), pos)
    }

    fn lex_block_comment(&mut self, pos: Position) -> Token {
        let start = self.pos;
        self.advance(); // /
        self.advance(); // *
        while self.pos + 1 < self.input.len() {
            if self.input[self.pos] == b'*' && self.input[self.pos + 1] == b'/' {
                self.advance();
                self.advance();
                return Token::new(
                    TokenKind::Comment,
                    slice_to_string(&self.input[start..self.pos]),
                    pos,
                );
            }
            self.advance();
        }
        Token::new(TokenKind::Illegal, "unterminated block comment", pos)
    }

    fn lex_string(&mut self, pos: Position) -> Token {
        self.advance(); // opening "
        let mut buf: Vec<u8> = Vec::new();
        while self.pos < self.input.len() {
            let ch = self.advance();
            if ch == b'"' {
                // Token values are stored as Rust `String`. Reject input that
                // would produce a non-UTF-8 sequence — Rust's typed strings
                // require validity, and proto3 strings are spec'd as UTF-8
                // anyway. Note this is stricter than the Go reference, which
                // permissively stores arbitrary bytes.
                return match String::from_utf8(buf) {
                    Ok(s) => Token::new(TokenKind::String, s, pos),
                    Err(_) => Token::new(
                        TokenKind::Illegal,
                        "string contains invalid UTF-8",
                        pos,
                    ),
                };
            }
            if ch != b'\\' {
                buf.push(ch);
                continue;
            }
            if self.pos >= self.input.len() {
                return Token::new(TokenKind::Illegal, "unterminated escape sequence", pos);
            }
            let esc = self.advance();
            match esc {
                b'"' | b'\\' | b'\'' | b'?' => buf.push(esc),
                b'a' => buf.push(0x07),
                b'b' => buf.push(0x08),
                b'f' => buf.push(0x0C),
                b'n' => buf.push(b'\n'),
                b'r' => buf.push(b'\r'),
                b't' => buf.push(b'\t'),
                b'v' => buf.push(0x0B),
                b'x' => match self.read_hex_byte() {
                    Some(b) => buf.push(b),
                    None => {
                        return Token::new(
                            TokenKind::Illegal,
                            "invalid \\x escape: expected 2 hex digits",
                            pos,
                        );
                    }
                },
                b'0' | b'1' | b'2' | b'3' => match self.read_oct_rest(esc) {
                    Some(b) => buf.push(b),
                    None => {
                        return Token::new(
                            TokenKind::Illegal,
                            "invalid octal escape: expected 3 octal digits",
                            pos,
                        );
                    }
                },
                b'u' => match self.read_hex_rune(4).and_then(valid_rune) {
                    Some(r) => encode_rune(r, &mut buf),
                    None => {
                        return Token::new(
                            TokenKind::Illegal,
                            "invalid \\u escape: expected 4 hex digits forming a valid codepoint",
                            pos,
                        );
                    }
                },
                b'U' => match self.read_hex_rune(8).and_then(valid_rune) {
                    Some(r) => encode_rune(r, &mut buf),
                    None => {
                        return Token::new(
                            TokenKind::Illegal,
                            "invalid \\U escape: expected 8 hex digits forming a valid codepoint",
                            pos,
                        );
                    }
                },
                other => {
                    return Token::new(
                        TokenKind::Illegal,
                        format!("unknown escape sequence \\{}", other as char),
                        pos,
                    );
                }
            }
        }
        Token::new(TokenKind::Illegal, "unterminated string", pos)
    }

    /// Reads exactly 2 hex digits and returns the assembled byte.
    fn read_hex_byte(&mut self) -> Option<u8> {
        if self.pos + 1 >= self.input.len() {
            return None;
        }
        let hi = hex_val(self.input[self.pos])?;
        let lo = hex_val(self.input[self.pos + 1])?;
        self.advance();
        self.advance();
        Some(((hi << 4) | lo) as u8)
    }

    /// Reads exactly N hex digits and returns the assembled codepoint.
    fn read_hex_rune(&mut self, n: usize) -> Option<u32> {
        if self.pos + n > self.input.len() {
            return None;
        }
        let mut r: u32 = 0;
        for _ in 0..n {
            r = (r << 4) | hex_val(self.input[self.pos])?;
            self.advance();
        }
        Some(r)
    }

    /// Reads two more octal digits after the leading one already consumed
    /// (as part of `\nnn` — exactly 3 octal digits total). The caller has
    /// restricted `first` to 0-3 so the result fits in a byte.
    fn read_oct_rest(&mut self, first: u8) -> Option<u8> {
        if self.pos + 1 >= self.input.len() {
            return None;
        }
        let d1 = oct_val(self.input[self.pos])?;
        let d2 = oct_val(self.input[self.pos + 1])?;
        self.advance();
        self.advance();
        Some((((first - b'0') as u32) << 6 | (d1 << 3) | d2) as u8)
    }

    fn lex_triple_string(&mut self, pos: Position) -> Token {
        self.advance();
        self.advance();
        self.advance();
        let start = self.pos;
        while self.pos + 2 < self.input.len() {
            if self.input[self.pos] == b'"'
                && self.input[self.pos + 1] == b'"'
                && self.input[self.pos + 2] == b'"'
            {
                let raw = slice_to_string(&self.input[start..self.pos]);
                self.advance();
                self.advance();
                self.advance();
                return Token::new(TokenKind::String, dedent(&raw), pos);
            }
            self.advance();
        }
        Token::new(TokenKind::Illegal, "unterminated triple-quoted string", pos)
    }

    fn lex_bytes(&mut self, pos: Position) -> Token {
        self.advance(); // b
        if self.pos >= self.input.len() || self.input[self.pos] != b'"' {
            return Token::new(TokenKind::Illegal, "expected '\"' after b", pos);
        }
        self.advance(); // opening "
        let start = self.pos;
        while self.pos < self.input.len() {
            let ch = self.input[self.pos];
            if ch == b'"' {
                let raw = slice_to_string(&self.input[start..self.pos]);
                self.advance(); // closing "
                if !is_valid_base64(&raw) {
                    return Token::new(
                        TokenKind::Illegal,
                        "invalid base64 in bytes literal",
                        pos,
                    );
                }
                return Token::new(TokenKind::Bytes, raw, pos);
            }
            if ch == b'\n' {
                return Token::new(TokenKind::Illegal, "unterminated bytes literal", pos);
            }
            self.advance();
        }
        Token::new(TokenKind::Illegal, "unterminated bytes literal", pos)
    }

    fn lex_directive(&mut self, pos: Position) -> Token {
        self.advance(); // @
        let start = self.pos;
        while self.pos < self.input.len() && is_ident_part(self.input[self.pos]) {
            self.advance();
        }
        let name = slice_to_string(&self.input[start..self.pos]);
        if name == "type" {
            return Token::new(TokenKind::AtType, "@type", pos);
        }
        Token::new(TokenKind::Illegal, format!("@{}", name), pos)
    }

    fn lex_number(&mut self, pos: Position) -> Token {
        let start = self.pos;
        let mut neg = false;
        if self.peek() == b'-' {
            neg = true;
            self.advance();
            if self.pos >= self.input.len() || !is_digit(self.peek()) {
                return Token::new(TokenKind::Illegal, "-", pos);
            }
        }

        let digit_start = self.pos;
        while self.pos < self.input.len() && is_digit(self.peek()) {
            self.advance();
        }
        let digit_count = self.pos - digit_start;

        if !neg && digit_count == 4 && self.pos < self.input.len() && self.peek() == b'-' {
            return self.lex_timestamp(pos, start);
        }

        if self.pos < self.input.len() {
            let c = self.peek();
            if c == b'.' || c == b'e' || c == b'E' {
                return self.lex_float(pos, start);
            }
        }

        if self.pos < self.input.len() && is_duration_unit(self.peek()) {
            return self.lex_duration(pos, start);
        }

        Token::new(
            TokenKind::Int,
            slice_to_string(&self.input[start..self.pos]),
            pos,
        )
    }

    fn lex_float(&mut self, pos: Position, start: usize) -> Token {
        if self.peek() == b'.' {
            self.advance();
            while self.pos < self.input.len() && is_digit(self.peek()) {
                self.advance();
            }
        }
        if self.pos < self.input.len() && (self.peek() == b'e' || self.peek() == b'E') {
            self.advance();
            if self.pos < self.input.len() && (self.peek() == b'+' || self.peek() == b'-') {
                self.advance();
            }
            while self.pos < self.input.len() && is_digit(self.peek()) {
                self.advance();
            }
        }
        Token::new(
            TokenKind::Float,
            slice_to_string(&self.input[start..self.pos]),
            pos,
        )
    }

    fn lex_timestamp(&mut self, pos: Position, start: usize) -> Token {
        while self.pos < self.input.len() {
            let ch = self.peek();
            if ch == b' '
                || ch == b'\n'
                || ch == b'\t'
                || ch == b'\r'
                || ch == b','
                || ch == b']'
                || ch == b'}'
                || ch == b'#'
            {
                break;
            }
            if ch == b'/' && (self.peek_at(1) == b'/' || self.peek_at(1) == b'*') {
                break;
            }
            self.advance();
        }
        let raw = slice_to_string(&self.input[start..self.pos]);
        if !is_valid_rfc3339(&raw) {
            return Token::new(TokenKind::Illegal, format!("invalid timestamp: {}", raw), pos);
        }
        Token::new(TokenKind::Timestamp, raw, pos)
    }

    fn lex_duration(&mut self, pos: Position, start: usize) -> Token {
        while self.pos < self.input.len() && (is_digit(self.peek()) || is_lower_alpha(self.peek())) {
            self.advance();
        }
        let raw = slice_to_string(&self.input[start..self.pos]);
        if !is_valid_go_duration(&raw) {
            return Token::new(TokenKind::Illegal, format!("invalid duration: {}", raw), pos);
        }
        Token::new(TokenKind::Duration, raw, pos)
    }

    fn lex_ident(&mut self, pos: Position) -> Token {
        let start = self.pos;
        while self.pos < self.input.len() && is_ident_part(self.input[self.pos]) {
            self.advance();
        }
        let val = slice_to_string(&self.input[start..self.pos]);
        match val.as_str() {
            "true" | "false" => Token::new(TokenKind::Bool, val, pos),
            "null" => Token::new(TokenKind::Null, val, pos),
            _ => Token::new(TokenKind::Ident, val, pos),
        }
    }
}

fn slice_to_string(bytes: &[u8]) -> String {
    // Input slices come from a `&str`, so byte-aligned views remain valid UTF-8.
    String::from_utf8(bytes.to_vec()).expect("lexer slice is valid UTF-8")
}

fn byte_as_string(b: u8) -> String {
    let mut s = String::new();
    s.push(b as char);
    s
}

fn is_digit(ch: u8) -> bool {
    ch.is_ascii_digit()
}

fn is_ident_start(ch: u8) -> bool {
    (b'a'..=b'z').contains(&ch) || (b'A'..=b'Z').contains(&ch) || ch == b'_'
}

fn is_ident_part(ch: u8) -> bool {
    is_ident_start(ch) || is_digit(ch) || ch == b'.'
}

fn is_duration_unit(ch: u8) -> bool {
    ch == b'h' || ch == b'm' || ch == b's' || ch == b'n' || ch == b'u'
}

fn is_lower_alpha(ch: u8) -> bool {
    (b'a'..=b'z').contains(&ch)
}

fn hex_val(ch: u8) -> Option<u32> {
    match ch {
        b'0'..=b'9' => Some((ch - b'0') as u32),
        b'a'..=b'f' => Some((ch - b'a') as u32 + 10),
        b'A'..=b'F' => Some((ch - b'A') as u32 + 10),
        _ => None,
    }
}

fn oct_val(ch: u8) -> Option<u32> {
    match ch {
        b'0'..=b'7' => Some((ch - b'0') as u32),
        _ => None,
    }
}

/// Mirrors Go's `utf8.ValidRune`: rejects values > U+10FFFF and the surrogate
/// range U+D800..U+DFFF. Returns `Some(r)` if valid for use in `\u` / `\U`.
fn valid_rune(r: u32) -> Option<u32> {
    if r <= 0x10_FFFF && !(0xD800..=0xDFFF).contains(&r) {
        Some(r)
    } else {
        None
    }
}

/// Writes the UTF-8 encoding of a valid Unicode scalar to `out`.
fn encode_rune(r: u32, out: &mut Vec<u8>) {
    if r <= 0x7F {
        out.push(r as u8);
    } else if r <= 0x7FF {
        out.push(0xC0 | (r >> 6) as u8);
        out.push(0x80 | (r & 0x3F) as u8);
    } else if r <= 0xFFFF {
        out.push(0xE0 | (r >> 12) as u8);
        out.push(0x80 | ((r >> 6) & 0x3F) as u8);
        out.push(0x80 | (r & 0x3F) as u8);
    } else {
        out.push(0xF0 | (r >> 18) as u8);
        out.push(0x80 | ((r >> 12) & 0x3F) as u8);
        out.push(0x80 | ((r >> 6) & 0x3F) as u8);
        out.push(0x80 | (r & 0x3F) as u8);
    }
}

/// Strip the closing-line indent from each line in a triple-quoted string body.
fn dedent(s: &str) -> String {
    let s = s.strip_prefix('\n').unwrap_or(s);
    let mut lines: Vec<&str> = s.split('\n').collect();
    if lines.is_empty() {
        return String::new();
    }
    let last = *lines.last().unwrap();
    if last.trim().is_empty() {
        let indent = last.to_string();
        lines.pop();
        let stripped: Vec<String> = lines
            .into_iter()
            .map(|line| {
                if let Some(rest) = line.strip_prefix(indent.as_str()) {
                    rest.to_string()
                } else {
                    line.to_string()
                }
            })
            .collect();
        return stripped.join("\n");
    }
    lines.join("\n")
}

/// Validate either standard (padded) or raw (unpadded) base64. Mirrors the
/// dual-fallback in `lexer.go` (`StdEncoding` then `RawStdEncoding`).
fn is_valid_base64(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    let bytes = s.as_bytes();
    let mut content_end = bytes.len();
    let mut pad = 0;
    while content_end > 0 && bytes[content_end - 1] == b'=' && pad < 2 {
        content_end -= 1;
        pad += 1;
    }
    for &b in &bytes[..content_end] {
        let ok = (b'A'..=b'Z').contains(&b)
            || (b'a'..=b'z').contains(&b)
            || (b'0'..=b'9').contains(&b)
            || b == b'+'
            || b == b'/';
        if !ok {
            return false;
        }
    }
    if pad == 0 {
        // raw: any length except mod 4 == 1
        return bytes.len() % 4 != 1;
    }
    bytes.len() % 4 == 0
}

/// Validate an RFC 3339 timestamp (Z or numeric offset, optional fractional
/// seconds). Calendar fields are range-checked, including leap-year-aware day
/// counts.
fn is_valid_rfc3339(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() < 20 {
        return false;
    }
    if !bytes[0..4].iter().all(|b| b.is_ascii_digit())
        || bytes[4] != b'-'
        || !bytes[5..7].iter().all(|b| b.is_ascii_digit())
        || bytes[7] != b'-'
        || !bytes[8..10].iter().all(|b| b.is_ascii_digit())
        || (bytes[10] != b'T' && bytes[10] != b't')
        || !bytes[11..13].iter().all(|b| b.is_ascii_digit())
        || bytes[13] != b':'
        || !bytes[14..16].iter().all(|b| b.is_ascii_digit())
        || bytes[16] != b':'
        || !bytes[17..19].iter().all(|b| b.is_ascii_digit())
    {
        return false;
    }

    let year = parse_u32(&bytes[0..4]);
    let month = parse_u32(&bytes[5..7]);
    let day = parse_u32(&bytes[8..10]);
    let hour = parse_u32(&bytes[11..13]);
    let minute = parse_u32(&bytes[14..16]);
    let second = parse_u32(&bytes[17..19]);

    if !(1..=12).contains(&month) {
        return false;
    }
    let max_day = days_in_month(year, month);
    if !(1..=max_day).contains(&day) {
        return false;
    }
    if hour > 23 || minute > 59 || second > 60 {
        return false;
    }

    let mut i = 19;
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        let frac_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == frac_start {
            return false;
        }
    }

    if i >= bytes.len() {
        return false;
    }
    if bytes[i] == b'Z' || bytes[i] == b'z' {
        return i + 1 == bytes.len();
    }
    if bytes[i] == b'+' || bytes[i] == b'-' {
        if bytes.len() != i + 6 {
            return false;
        }
        if !bytes[i + 1..i + 3].iter().all(|b| b.is_ascii_digit())
            || bytes[i + 3] != b':'
            || !bytes[i + 4..i + 6].iter().all(|b| b.is_ascii_digit())
        {
            return false;
        }
        let off_h = parse_u32(&bytes[i + 1..i + 3]);
        let off_m = parse_u32(&bytes[i + 4..i + 6]);
        if off_h > 23 || off_m > 59 {
            return false;
        }
        return true;
    }
    false
}

fn parse_u32(bytes: &[u8]) -> u32 {
    let mut n: u32 = 0;
    for &b in bytes {
        n = n * 10 + (b - b'0') as u32;
    }
    n
}

fn is_leap_year(y: u32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: u32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(y) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Validate a Go-style duration string: optional leading sign, then one or
/// more `<digits>[.<digits>]<unit>` groups where unit ∈ {ns, us, ms, s, m, h}.
///
/// The lexer never emits non-ASCII bytes here, so `µs` (which Go's parser
/// accepts) is intentionally not handled — same as `lexer.go`.
fn is_valid_go_duration(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let mut i = 0;
    if bytes[0] == b'-' || bytes[0] == b'+' {
        i += 1;
    }
    if i >= bytes.len() {
        return false;
    }
    let mut groups = 0;
    while i < bytes.len() {
        let digit_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == digit_start {
            return false;
        }
        if i < bytes.len() && bytes[i] == b'.' {
            i += 1;
            let frac_start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i == frac_start {
                return false;
            }
        }
        if i >= bytes.len() {
            return false;
        }
        let next = if i + 1 < bytes.len() {
            Some(bytes[i + 1])
        } else {
            None
        };
        let unit_len = match (bytes[i], next) {
            (b'n', Some(b's')) => 2,
            (b'u', Some(b's')) => 2,
            (b'm', Some(b's')) => 2,
            (b's', _) | (b'm', _) | (b'h', _) => 1,
            _ => return false,
        };
        i += unit_len;
        groups += 1;
    }
    groups > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(input: &str) -> Vec<Token> {
        Lexer::new(input)
            .collect_tokens()
            .into_iter()
            .filter(|t| !matches!(t.kind, TokenKind::Eof))
            .collect()
    }

    fn kinds(input: &str) -> Vec<TokenKind> {
        tokens(input).into_iter().map(|t| t.kind).collect()
    }

    // ---------------- punctuation and whitespace ----------------

    #[test]
    fn punctuation_emits_braces_brackets_equals_colon_comma() {
        assert_eq!(
            kinds("{}[]=:,"),
            vec![
                TokenKind::LBrace,
                TokenKind::RBrace,
                TokenKind::LBracket,
                TokenKind::RBracket,
                TokenKind::Equals,
                TokenKind::Colon,
                TokenKind::Comma,
            ]
        );
    }

    #[test]
    fn whitespace_emits_newline_for_lf_skips_space_tab_cr() {
        assert_eq!(
            kinds("  \t\r{\n}"),
            vec![TokenKind::LBrace, TokenKind::Newline, TokenKind::RBrace]
        );
    }

    // ---------------- comments ----------------

    #[test]
    fn comment_hash_line() {
        let t = tokens("# hello world");
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].kind, TokenKind::Comment);
        assert_eq!(t[0].value, "# hello world");
    }

    #[test]
    fn comment_double_slash_line() {
        let t = tokens("// just a note");
        assert_eq!(t[0].kind, TokenKind::Comment);
        assert_eq!(t[0].value, "// just a note");
    }

    #[test]
    fn comment_block_inline() {
        let t = tokens("/* inline */");
        assert_eq!(t[0].kind, TokenKind::Comment);
        assert_eq!(t[0].value, "/* inline */");
    }

    #[test]
    fn comment_block_multi_line_tracks_position_past_newline() {
        let t = tokens("/* line1\nline2 */ x");
        assert_eq!(t[0].kind, TokenKind::Comment);
        assert_eq!(t[1].kind, TokenKind::Ident);
        assert_eq!(t[1].pos.line, 2);
    }

    #[test]
    fn comment_block_unterminated_is_illegal() {
        let t = tokens("/* no close");
        assert_eq!(t[0].kind, TokenKind::Illegal);
    }

    // ---------------- strings ----------------

    #[test]
    fn string_simple() {
        let t = tokens("\"hello\"");
        assert_eq!(t[0].kind, TokenKind::String);
        assert_eq!(t[0].value, "hello");
    }

    #[test]
    fn string_escape_sequences() {
        let t = tokens("\"a\\nb\\tc\\rd\\\\e\\\"f\"");
        assert_eq!(t[0].value, "a\nb\tc\rd\\e\"f");
    }

    #[test]
    fn string_unknown_escape_is_illegal() {
        // Unknown escapes used to silently pass through; they now produce
        // an ILLEGAL token to match the Go reference.
        let t = tokens("\"\\q\"");
        assert!(matches!(t[0].kind, TokenKind::Illegal));
        assert!(t[0].value.contains("unknown escape"));
    }

    #[test]
    fn string_utf8_passes_through() {
        let t = tokens("\"héllo, 世界\"");
        assert_eq!(t[0].value, "héllo, 世界");
    }

    #[test]
    fn string_unterminated_is_illegal() {
        let t = tokens("\"oops");
        assert_eq!(t[0].kind, TokenKind::Illegal);
    }

    #[test]
    fn string_triple_quoted_preserves_embedded_newlines() {
        let t = tokens("\"\"\"line1\nline2\"\"\"");
        assert_eq!(t[0].kind, TokenKind::String);
        assert_eq!(t[0].value, "line1\nline2");
    }

    #[test]
    fn string_triple_quoted_dedents_using_closing_indent() {
        let src = "\"\"\"\n  hello\n  world\n  \"\"\"";
        let t = tokens(src);
        assert_eq!(t[0].value, "hello\nworld");
    }

    #[test]
    fn string_triple_quoted_unterminated_is_illegal() {
        let t = tokens("\"\"\"no close");
        assert_eq!(t[0].kind, TokenKind::Illegal);
    }

    // ---------------- bytes ----------------

    #[test]
    fn bytes_standard_base64_decodes() {
        let t = tokens("b\"SGVsbG8=\"");
        assert_eq!(t[0].kind, TokenKind::Bytes);
        assert_eq!(t[0].value, "SGVsbG8=");
    }

    #[test]
    fn bytes_raw_unpadded_base64_accepted() {
        let t = tokens("b\"SGVsbG8\"");
        assert_eq!(t[0].kind, TokenKind::Bytes);
    }

    #[test]
    fn bytes_invalid_base64_is_illegal() {
        let t = tokens("b\"!!!\"");
        assert_eq!(t[0].kind, TokenKind::Illegal);
    }

    #[test]
    fn bytes_plain_b_followed_by_ident_is_just_ident() {
        let t = tokens("bool");
        assert_eq!(t[0].kind, TokenKind::Ident);
        assert_eq!(t[0].value, "bool");
    }

    // ---------------- numbers ----------------

    #[test]
    fn number_integer() {
        let t = tokens("123");
        assert_eq!(t[0].kind, TokenKind::Int);
        assert_eq!(t[0].value, "123");
    }

    #[test]
    fn number_negative_integer() {
        let t = tokens("-456");
        assert_eq!(t[0].kind, TokenKind::Int);
        assert_eq!(t[0].value, "-456");
    }

    #[test]
    fn number_float_with_decimal() {
        let t = tokens("1.23");
        assert_eq!(t[0].kind, TokenKind::Float);
        assert_eq!(t[0].value, "1.23");
    }

    #[test]
    fn number_float_with_exponent() {
        let t = tokens("6.022e23");
        assert_eq!(t[0].kind, TokenKind::Float);
        assert_eq!(t[0].value, "6.022e23");
    }

    #[test]
    fn number_negative_float_with_exponent() {
        let t = tokens("-1.5e-10");
        assert_eq!(t[0].kind, TokenKind::Float);
        assert_eq!(t[0].value, "-1.5e-10");
    }

    #[test]
    fn number_bare_minus_with_no_digits_is_illegal() {
        let t = tokens("-x");
        assert_eq!(t[0].kind, TokenKind::Illegal);
    }

    // ---------------- timestamps ----------------

    #[test]
    fn timestamp_z_suffix() {
        let t = tokens("2024-01-15T10:30:00Z");
        assert_eq!(t[0].kind, TokenKind::Timestamp);
        assert_eq!(t[0].value, "2024-01-15T10:30:00Z");
    }

    #[test]
    fn timestamp_with_offset() {
        let t = tokens("2024-01-15T10:30:00+05:30");
        assert_eq!(t[0].kind, TokenKind::Timestamp);
    }

    #[test]
    fn timestamp_with_fractional_seconds() {
        let t = tokens("2024-01-15T10:30:00.123456789Z");
        assert_eq!(t[0].kind, TokenKind::Timestamp);
    }

    #[test]
    fn timestamp_invalid_month_is_illegal() {
        let t = tokens("2024-13-01T00:00:00Z");
        assert_eq!(t[0].kind, TokenKind::Illegal);
    }

    #[test]
    fn timestamp_year_then_non_timestamp_falls_back_to_int() {
        let t = tokens("1234,");
        assert_eq!(t[0].kind, TokenKind::Int);
        assert_eq!(t[0].value, "1234");
    }

    // ---------------- durations ----------------

    #[test]
    fn duration_seconds() {
        let t = tokens("30s");
        assert_eq!(t[0].kind, TokenKind::Duration);
        assert_eq!(t[0].value, "30s");
    }

    #[test]
    fn duration_composite() {
        let t = tokens("1h30m45s");
        assert_eq!(t[0].kind, TokenKind::Duration);
    }

    #[test]
    fn duration_negative() {
        let t = tokens("-1h30m");
        assert_eq!(t[0].kind, TokenKind::Duration);
        assert_eq!(t[0].value, "-1h30m");
    }

    #[test]
    fn duration_subsecond_units() {
        assert_eq!(tokens("100ns")[0].kind, TokenKind::Duration);
        assert_eq!(tokens("250ms")[0].kind, TokenKind::Duration);
        assert_eq!(tokens("3us")[0].kind, TokenKind::Duration);
    }

    #[test]
    fn duration_no_day_unit_lexes_int_then_ident() {
        let ts = tokens("5d");
        assert_eq!(
            ts.iter().map(|t| t.kind).collect::<Vec<_>>(),
            vec![TokenKind::Int, TokenKind::Ident]
        );
    }

    #[test]
    fn duration_float_path_wins_over_duration() {
        let ts = tokens("1.5s");
        assert_eq!(
            ts.iter().map(|t| t.kind).collect::<Vec<_>>(),
            vec![TokenKind::Float, TokenKind::Ident]
        );
    }

    #[test]
    fn duration_malformed_is_illegal() {
        let t = tokens("5sx");
        assert_eq!(t[0].kind, TokenKind::Illegal);
    }

    // ---------------- identifiers and keywords ----------------

    #[test]
    fn ident_plain() {
        let t = tokens("name");
        assert_eq!(t[0].kind, TokenKind::Ident);
        assert_eq!(t[0].value, "name");
    }

    #[test]
    fn ident_dotted_package_type() {
        assert_eq!(tokens("infra.v1.ServerConfig")[0].value, "infra.v1.ServerConfig");
    }

    #[test]
    fn ident_true_false_become_bool() {
        assert_eq!(tokens("true")[0].kind, TokenKind::Bool);
        assert_eq!(tokens("false")[0].kind, TokenKind::Bool);
    }

    #[test]
    fn ident_null_becomes_null() {
        assert_eq!(tokens("null")[0].kind, TokenKind::Null);
    }

    #[test]
    fn ident_with_underscore_and_digits() {
        assert_eq!(tokens("_name123")[0].value, "_name123");
    }

    // ---------------- @type directive ----------------

    #[test]
    fn directive_at_type_recognized() {
        let t = tokens("@type");
        assert_eq!(t[0].kind, TokenKind::AtType);
        assert_eq!(t[0].value, "@type");
    }

    #[test]
    fn directive_unknown_is_illegal() {
        let t = tokens("@bogus");
        assert_eq!(t[0].kind, TokenKind::Illegal);
    }

    // ---------------- position tracking ----------------

    #[test]
    fn position_line_and_column_advance() {
        let t = tokens("\"a\"\n  name");
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].pos, Position::new(1, 1));
        assert_eq!(t[1].pos, Position::new(1, 4));
        assert_eq!(t[2].pos, Position::new(2, 3));
    }

    // ---------------- end-to-end ----------------

    #[test]
    fn end_to_end_pxf_readme_example_tokenizes_cleanly() {
        let src = "@type infra.v1.ServerConfig\n\
                   \n\
                   hostname = \"web-01.prod.example.com\"\n\
                   port     = 8443\n\
                   enabled  = true\n\
                   \n\
                   # Well-known type literals\n\
                   created_at = 2024-01-15T10:30:00Z\n\
                   timeout    = 30s\n\
                   \n\
                   # Nested messages use block syntax\n\
                   tls {\n\
                     cert_file = \"/etc/ssl/cert.pem\"\n\
                     key_file  = \"/etc/ssl/key.pem\"\n\
                     verify    = true\n\
                   }\n";
        let ts = tokens(src);
        assert!(
            ts.iter().find(|t| matches!(t.kind, TokenKind::Illegal)).is_none(),
            "no ILLEGAL tokens"
        );
        let meaningful: Vec<&Token> = ts
            .iter()
            .filter(|t| !matches!(t.kind, TokenKind::Newline | TokenKind::Comment))
            .collect();
        assert_eq!(meaningful[0].kind, TokenKind::AtType);
        assert_eq!(meaningful[1].value, "infra.v1.ServerConfig");
        assert_eq!(
            meaningful
                .iter()
                .find(|t| matches!(t.kind, TokenKind::Timestamp))
                .map(|t| t.value.as_str()),
            Some("2024-01-15T10:30:00Z")
        );
        assert_eq!(
            meaningful
                .iter()
                .find(|t| matches!(t.kind, TokenKind::Duration))
                .map(|t| t.value.as_str()),
            Some("30s")
        );
        assert_eq!(
            meaningful
                .iter()
                .filter(|t| matches!(t.kind, TokenKind::LBrace))
                .count(),
            1
        );
    }

    // --- Full Go-aligned escape set: \a \b \f \v \' \?, \xHH, \nnn,
    //     \uHHHH, \UHHHHHHHH. Mirrors protowire-go/encoding/pxf/lexer_test.go.

    /// Lex a single STRING token from `src`; returns Some(value) or None on
    /// ILLEGAL.
    fn lex_one(src: &str) -> Option<String> {
        let t = tokens(src);
        match t.first() {
            Some(tok) if matches!(tok.kind, TokenKind::String) => Some(tok.value.clone()),
            _ => None,
        }
    }

    #[test]
    fn escape_extended_simple_set() {
        assert_eq!(lex_one(r#""\a""#).as_deref(), Some("\u{07}"));
        assert_eq!(lex_one(r#""\b""#).as_deref(), Some("\u{08}"));
        assert_eq!(lex_one(r#""\f""#).as_deref(), Some("\u{0C}"));
        assert_eq!(lex_one(r#""\v""#).as_deref(), Some("\u{0B}"));
        assert_eq!(lex_one(r#""\'""#).as_deref(), Some("'"));
        assert_eq!(lex_one(r#""\?""#).as_deref(), Some("?"));
        assert_eq!(
            lex_one(r#""\a\b\f\n\r\t\v""#).as_deref(),
            Some("\u{07}\u{08}\u{0C}\n\r\t\u{0B}"),
        );
    }

    #[test]
    fn escape_hex_byte() {
        assert_eq!(lex_one(r#""\x41""#).as_deref(), Some("A"));
        assert_eq!(lex_one(r#""\x00""#).as_deref(), Some("\0"));
        // Two adjacent \x escapes encode a 2-byte UTF-8 sequence.
        assert_eq!(lex_one(r#""\xc3\xa9""#).as_deref(), Some("é"));
        // Three encode a 3-byte UTF-8 sequence.
        assert_eq!(
            lex_one(r#""\xe4\xb8\xad""#).as_deref(),
            Some("中"),
        );
    }

    #[test]
    fn escape_octal_byte() {
        assert_eq!(lex_one(r#""\101""#).as_deref(), Some("A"));
        assert_eq!(lex_one(r#""\000""#).as_deref(), Some("\0"));
        // \377 = 0xFF — a lone byte that's not valid UTF-8 on its own; the
        // Rust lexer rejects rather than store an invalid String.
        assert!(lex_one(r#""\377""#).is_none());
    }

    #[test]
    fn escape_unicode_4_hex() {
        assert_eq!(lex_one(r#""é""#).as_deref(), Some("é"));
        assert_eq!(lex_one(r#""中""#).as_deref(), Some("中"));
        assert_eq!(
            lex_one(r#""aéb""#).as_deref(),
            Some("aéb"),
        );
    }

    #[test]
    fn escape_unicode_8_hex() {
        assert_eq!(lex_one(r#""\U0001F600""#).as_deref(), Some("😀"));
        assert_eq!(lex_one(r#""\U0000004A""#).as_deref(), Some("J"));
    }

    #[test]
    fn escape_invalid_forms_rejected() {
        // Unknown escape.
        assert!(lex_one(r#""\z""#).is_none());
        // Truncated \u.
        assert!(lex_one(r#""\u12""#).is_none());
        // Non-hex in \u.
        assert!(lex_one(r#""\u12gh""#).is_none());
        // Surrogate halves rejected.
        assert!(lex_one(r#""\uD800""#).is_none());
        assert!(lex_one(r#""\uDFFF""#).is_none());
        // Out-of-range \U.
        assert!(lex_one(r#""\U00110000""#).is_none());
        // Truncated \U (only 7 hex digits).
        assert!(lex_one(r#""\U0001F60""#).is_none());
        // Truncated \x.
        assert!(lex_one(r#""\x""#).is_none());
        assert!(lex_one(r#""\x4""#).is_none());
        // Non-hex \x.
        assert!(lex_one(r#""\xZZ""#).is_none());
        // Truncated octal.
        assert!(lex_one(r#""\10""#).is_none());
        // Non-octal in octal escape.
        assert!(lex_one(r#""\18a""#).is_none());
    }

    #[test]
    fn bytes_literal_does_not_interpret_escapes() {
        // b"..." now reads the body raw (no escape interpretation). A
        // literal `\\` is invalid base64, so the lexer must produce an
        // ILLEGAL token rather than decoding the escape.
        let t = tokens(r#"b"hello\""#);
        assert!(matches!(t[0].kind, TokenKind::Illegal));
    }

    #[test]
    fn bytes_literal_accepts_valid_base64() {
        // "Hello" in base64 = "SGVsbG8="
        let t = tokens(r#"b"SGVsbG8=""#);
        assert!(matches!(t[0].kind, TokenKind::Bytes));
        assert_eq!(t[0].value, "SGVsbG8=");
    }
}
