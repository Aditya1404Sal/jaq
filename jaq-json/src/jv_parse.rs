//! jq 1.8's JSON reader (`src/jv_parse.c`), ported for byte-for-byte fidelity: the same
//! incremental parser fed buffer by buffer, the same diagnostics and positions
//! (`Unfinished JSON term at EOF at line 2, column 0`), a byte order mark stripped only at the
//! very start, invalid UTF-8 replaced as jq replaces it, and number literals checked with
//! decNumber's syntax and kept as written.
//!
//! Like jq's, the parser holds its open arrays and objects on an explicit stack, so input depth
//! never recurses natively.
use crate::{Map, Num, Val};
use alloc::string::String;
use alloc::vec::Vec;
use jaq_core::jq_utf8;

/// How many arrays, objects and pending keys may be open at once. jq allows 10,000; other code
/// (printing, dropping) walks a value recursively, so this reader stops sooner, with jq's
/// message.
pub const MAX_PARSING_DEPTH: usize = 3000;

const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
/// `bom` once the stream has begun without a byte order mark or after a whole one.
const BOM_DONE: u8 = 3;
/// `bom` after a partial byte order mark.
const BOM_MALFORMED: u8 = 0xFF;

enum Frame {
    Arr(Vec<Val>),
    Obj(Map),
    /// An object key read, waiting for its value.
    Key(Val),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Normal,
    Str,
    StrEscape,
    /// `--seq` input before its first record separator, or waiting to resync after an error.
    WaitingForRs,
}

/// ASCII RS, which starts each text of a JSON text sequence (`--seq`).
const RS: u8 = 0x1E;

/// What one scanned byte leaves the parser with.
enum Step {
    Continue,
    Value(Val),
    /// Stop scanning for now with nothing to show (an RS that ends nothing).
    Stop,
}

/// What the parser has for its caller next (`jv_parser_next`'s three outcomes).
#[derive(Debug)]
pub enum Next {
    /// A complete top-level value.
    Value(Val),
    /// A syntax error, worded and positioned as jq words it. The parser drops the rest of the
    /// current buffer.
    Error(String),
    /// Nothing yet: the buffer ran out (give it another), or the input is over.
    More,
}

/// jq's `struct jv_parser`, with its `--seq` mode but not `--stream`.
pub struct Parser {
    /// Reading a JSON text sequence (`--seq`).
    seq: bool,
    /// Whether the byte scanned last was whitespace.
    last_ch_was_ws: bool,
    stack: Vec<Frame>,
    next: Option<Val>,
    token: Vec<u8>,
    state: State,
    line: usize,
    column: usize,
    bom: u8,
    buf: Vec<u8>,
    pos: usize,
    /// Whether more buffers follow this one.
    partial: bool,
    /// Whether there is a buffer at all: jq drops its buffer after a syntax error.
    has_buf: bool,
    eof: bool,
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    /// A parser at the start of a stream.
    #[must_use]
    pub const fn new() -> Self {
        Self::with_seq(false)
    }

    /// A parser at the start of a stream, reading a JSON text sequence (`--seq`) if `seq`.
    #[must_use]
    pub const fn with_seq(seq: bool) -> Self {
        Self {
            seq,
            last_ch_was_ws: false,
            stack: Vec::new(),
            next: None,
            token: Vec::new(),
            state: if seq {
                State::WaitingForRs
            } else {
                State::Normal
            },
            line: 1,
            column: 0,
            bom: 0,
            buf: Vec::new(),
            pos: 0,
            partial: false,
            has_buf: false,
            eof: false,
        }
    }

    /// Bytes of the current buffer not scanned yet (`jv_parser_remaining`).
    #[must_use]
    pub fn remaining(&self) -> usize {
        if self.has_buf {
            self.buf.len() - self.pos
        } else {
            0
        }
    }

    /// Give the parser its next buffer (`jv_parser_set_buf`); `partial` says more follow. A
    /// byte order mark is stripped from the start of the stream, however the buffers split it.
    pub fn set_buf(&mut self, mut buf: &[u8], partial: bool) {
        while let Some((&first, rest)) = buf.split_first() {
            if self.bom >= BOM_DONE {
                break;
            }
            if first == BOM[usize::from(self.bom)] {
                buf = rest;
                self.bom += 1;
            } else if self.bom == 0 {
                self.bom = BOM_DONE;
            } else {
                self.bom = BOM_MALFORMED;
            }
        }
        self.buf.clear();
        self.buf.extend_from_slice(buf);
        self.pos = 0;
        self.partial = partial;
        self.has_buf = true;
    }

    /// Scan until a value is complete, an error occurs, or the buffer runs out
    /// (`jv_parser_next`).
    pub fn next_value(&mut self) -> Next {
        if self.eof || !self.has_buf {
            return Next::More;
        }
        if self.bom == BOM_MALFORMED {
            if !self.seq {
                return Next::Error("Malformed BOM".into());
            }
            // jq waits for an RS, though its reset takes it straight back to scanning.
            self.state = State::WaitingForRs;
            self.reset();
        }
        while self.pos < self.buf.len() {
            let ch = self.buf[self.pos];
            self.pos += 1;
            if self.state == State::WaitingForRs {
                if ch == b'\n' {
                    self.line += 1;
                    self.column = 0;
                } else {
                    self.column += 1;
                }
                if ch == RS {
                    self.state = State::Normal;
                }
                continue;
            }
            match self.scan(ch) {
                Ok(Step::Continue) => (),
                Ok(Step::Value(value)) => return Next::Value(value),
                Ok(Step::Stop) => return Next::More,
                Err(message) => {
                    let (line, column) = (self.line, self.column);
                    if ch != RS && self.seq {
                        // jq means to skip to the next RS, but its reset returns it to scanning.
                        self.state = State::WaitingForRs;
                        self.reset();
                        return Next::Error(alloc::format!(
                            "{message} at line {line}, column {column} (need RS to resync)"
                        ));
                    }
                    self.reset();
                    if !self.seq {
                        // jq throws the rest of this buffer away.
                        self.has_buf = false;
                        self.buf.clear();
                        self.pos = 0;
                    }
                    return Next::Error(alloc::format!(
                        "{message} at line {line}, column {column}"
                    ));
                }
            }
        }
        if self.partial {
            return Next::More;
        }
        self.eof = true;
        let at_eof = |parser: &mut Self, message: &str| {
            let error = alloc::format!(
                "{message} at EOF at line {}, column {}",
                parser.line,
                parser.column
            );
            parser.reset();
            parser.state = State::WaitingForRs;
            Next::Error(error)
        };
        if self.state == State::WaitingForRs {
            let (line, column) = (self.line, self.column);
            return Next::Error(alloc::format!(
                "Unfinished abandoned text at EOF at line {line}, column {column}"
            ));
        }
        if self.state != State::Normal {
            return at_eof(self, "Unfinished string");
        }
        if let Err(message) = self.check_literal() {
            return at_eof(self, message);
        }
        if !self.stack.is_empty() {
            return at_eof(self, "Unfinished JSON term");
        }
        let value = self.next.take();
        if self.seq && !self.last_ch_was_ws && matches!(value, Some(Val::Num(_))) {
            let (line, column) = (self.line, self.column);
            return Next::Error(alloc::format!(
                "Potentially truncated top-level numeric value at EOF at line {line}, column {column}"
            ));
        }
        value.map_or(Next::More, Next::Value)
    }

    fn reset(&mut self) {
        self.stack.clear();
        self.next = None;
        self.token.clear();
        self.state = State::Normal;
    }

    /// `scan`: one byte.
    fn scan(&mut self, ch: u8) -> Result<Step, &'static str> {
        self.column += 1;
        if ch == b'\n' {
            self.line += 1;
            self.column = 0;
        }
        if self.seq && ch == RS {
            // An RS ends the text before it, which must be complete.
            let truncated = !self.last_ch_was_ws
                && (!self.stack.is_empty()
                    || !self.token.is_empty()
                    || matches!(self.next, Some(Val::Num(_))));
            if truncated {
                let literal = self.check_literal();
                if literal.is_ok()
                    && self.stack.is_empty()
                    && matches!(self.next, Some(Val::Num(_)))
                {
                    return Err("Potentially truncated top-level numeric value");
                }
                return Err("Truncated value");
            }
            self.check_literal()?;
            if self.state == State::Normal {
                if let Some(value) = self.check_done() {
                    return Ok(Step::Value(value));
                }
            }
            self.reset();
            return Ok(Step::Stop);
        }
        self.last_ch_was_ws = false;
        if self.state != State::Normal {
            if ch == b'"' && self.state == State::Str {
                self.found_string()?;
                self.state = State::Normal;
                return Ok(self.check_done().map_or(Step::Continue, Step::Value));
            }
            self.token.push(ch);
            self.state = if ch == b'\\' && self.state == State::Str {
                State::StrEscape
            } else {
                State::Str
            };
            return Ok(Step::Continue);
        }
        let literal = !matches!(
            ch,
            b' ' | b'\t' | b'\r' | b'\n' | b'"' | b'[' | b',' | b']' | b'{' | b':' | b'}'
        );
        if literal {
            self.token.push(ch);
            return Ok(Step::Continue);
        }
        self.last_ch_was_ws = matches!(ch, b' ' | b'\t' | b'\r' | b'\n');
        self.check_literal()?;
        // jq takes a value finished here, but a failing structural character still wins.
        let mut done = self.check_done();
        match ch {
            b'"' => self.state = State::Str,
            b' ' | b'\t' | b'\r' | b'\n' => (),
            _ => self.token(ch)?,
        }
        if let Some(value) = self.check_done() {
            done = Some(value);
        }
        Ok(done.map_or(Step::Continue, Step::Value))
    }

    /// `parse_check_done`: a complete top-level value.
    fn check_done(&mut self) -> Option<Val> {
        if self.stack.is_empty() {
            self.next.take()
        } else {
            None
        }
    }

    /// `value`: a scalar or a closed container, which nothing may precede unseparated.
    fn value(&mut self, value: Val) -> Result<(), &'static str> {
        if self.next.is_some() {
            return Err("Expected separator between values");
        }
        self.next = Some(value);
        Ok(())
    }

    /// `parse_token`: a structural character.
    fn token(&mut self, ch: u8) -> Result<(), &'static str> {
        match ch {
            b'[' | b'{' => {
                if self.stack.len() >= MAX_PARSING_DEPTH {
                    return Err("Exceeds depth limit for parsing");
                }
                if self.next.is_some() {
                    return Err("Expected separator between values");
                }
                self.stack.push(if ch == b'[' {
                    Frame::Arr(Vec::new())
                } else {
                    Frame::Obj(Map::default())
                });
            }
            b':' => {
                let Some(key) = self.next.take() else {
                    return Err("Expected string key before ':'");
                };
                if !matches!(self.stack.last(), Some(Frame::Obj(_))) {
                    self.next = Some(key);
                    return Err("':' not as part of an object");
                }
                if !matches!(key, Val::TStr(_)) {
                    self.next = Some(key);
                    return Err("Object keys must be strings");
                }
                self.stack.push(Frame::Key(key));
            }
            b',' => {
                if self.next.is_none() {
                    return Err("Expected value before ','");
                }
                match self.stack.last_mut() {
                    None => return Err("',' not as part of an object or array"),
                    Some(Frame::Arr(items)) => items.extend(self.next.take()),
                    Some(Frame::Key(_)) => self.set_member(),
                    Some(Frame::Obj(_)) => return Err("Objects must consist of key:value pairs"),
                }
            }
            b']' => {
                let Some(Frame::Arr(items)) = self.stack.last_mut() else {
                    return Err("Unmatched ']'");
                };
                match self.next.take() {
                    Some(value) => items.push(value),
                    None if !items.is_empty() => return Err("Expected another array element"),
                    None => (),
                }
                if let Some(Frame::Arr(items)) = self.stack.pop() {
                    self.next = Some(Val::Arr(items.into()));
                }
            }
            b'}' => {
                if self.stack.is_empty() {
                    return Err("Unmatched '}'");
                }
                if self.next.is_some() {
                    if !matches!(self.stack.last(), Some(Frame::Key(_))) {
                        return Err("Objects must consist of key:value pairs");
                    }
                    self.set_member();
                } else {
                    match self.stack.last() {
                        Some(Frame::Obj(members)) if !members.is_empty() => {
                            return Err("Expected another key-value pair");
                        }
                        Some(Frame::Obj(_)) => (),
                        _ => return Err("Unmatched '}'"),
                    }
                }
                if let Some(Frame::Obj(members)) = self.stack.pop() {
                    self.next = Some(Val::obj(members));
                }
            }
            _ => (),
        }
        Ok(())
    }

    /// Store the pending key and value in the object under them.
    fn set_member(&mut self) {
        if let (Some(Frame::Key(key)), Some(value)) = (self.stack.pop(), self.next.take()) {
            if let Some(Frame::Obj(members)) = self.stack.last_mut() {
                members.insert(key, value);
            }
        }
    }

    /// `found_string`: the text between quotes, with its escapes decoded.
    fn found_string(&mut self) -> Result<(), &'static str> {
        let text = core::mem::take(&mut self.token);
        let value = decode_string(&text)?;
        self.value(value)
    }

    /// `check_literal`: the pending unquoted token, if any.
    fn check_literal(&mut self) -> Result<(), &'static str> {
        if self.token.is_empty() {
            return Ok(());
        }
        let pattern: Option<(&[u8], Val)> = match self.token[0] {
            b't' => Some((b"true", Val::Bool(true))),
            b'f' => Some((b"false", Val::Bool(false))),
            b'\'' => return Err("Invalid string literal; expected \", but got '"),
            b'n' if self.token.get(1) == Some(&b'u') => Some((b"null", Val::Null)),
            _ => None,
        };
        let value = match pattern {
            Some((pattern, value)) if self.token == pattern => value,
            Some(_) => return Err("Invalid literal"),
            // jq hands the token to decNumber as a C string, ending at a NUL.
            None => match self
                .token
                .split(|b| *b == 0)
                .next()
                .and_then(number_literal)
            {
                Some(n) => Val::Num(n),
                None => return Err("Invalid numeric literal"),
            },
        };
        self.value(value)?;
        self.token.clear();
        Ok(())
    }
}

/// Decode a string's text between its quotes as `found_string` does.
fn decode_string(text: &[u8]) -> Result<Val, &'static str> {
    let hex4 = |hex: &[u8]| -> Option<u32> {
        hex.iter().try_fold(0, |n, b| {
            char::from(*b).to_digit(16).map(|digit| (n << 4) | digit)
        })
    };
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        let c = text[i];
        i += 1;
        if c != b'\\' {
            if c < 0x20 {
                return Err(
                    "Invalid string: control characters from U+0000 through U+001F must be escaped",
                );
            }
            out.push(c);
            continue;
        }
        let Some(&c) = text.get(i) else {
            return Err("Expected escape character at end of string");
        };
        i += 1;
        match c {
            b'\\' | b'"' | b'/' => out.push(c),
            b'b' => out.push(0x08),
            b'f' => out.push(0x0C),
            b't' => out.push(b'\t'),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b'u' => {
                let Some(hex) = text.get(i..i + 4) else {
                    return Err("Invalid \\uXXXX escape");
                };
                let Some(mut codepoint) = hex4(hex) else {
                    return Err("Invalid characters in \\uXXXX escape");
                };
                i += 4;
                if (0xD800..=0xDBFF).contains(&codepoint) {
                    let low = match text.get(i..i + 6) {
                        Some([b'\\', b'u', hex @ ..]) => hex4(hex),
                        _ => None,
                    };
                    let Some(low) = low.filter(|low| (0xDC00..=0xDFFF).contains(low)) else {
                        return Err("Invalid \\uXXXX\\uXXXX surrogate pair escape");
                    };
                    i += 6;
                    codepoint = 0x10000 + (((codepoint - 0xD800) << 10) | (low - 0xDC00));
                }
                jq_utf8::encode(codepoint, &mut out);
            }
            _ => return Err("Invalid escape"),
        }
    }
    Ok(Val::from(jq_utf8::lossy(out)))
}

/// A number from its text, as jq's `jv_number_with_literal` makes one: decNumber's syntax
/// (an optional sign; digits with at most one `.`; an optional exponent; or `Infinity`, `Inf`
/// or a `NaN`/`sNaN` without a payload, in any case). The number keeps its literal text, so
/// `1.50` prints as `1.50` and `-0` as `-0`.
#[must_use]
pub fn number_literal(text: &[u8]) -> Option<Num> {
    let (negative, rest) = match text.split_first() {
        Some((b'-', rest)) => (true, rest),
        Some((b'+', rest)) => (false, rest),
        _ => (false, text),
    };
    let mut digits = 0;
    let mut dot = false;
    let mut end = 0;
    for &b in rest {
        match b {
            b'0'..=b'9' => digits += 1,
            b'.' if !dot => dot = true,
            _ => break,
        }
        end += 1;
    }
    if digits == 0 {
        if dot {
            return None;
        }
        if rest.eq_ignore_ascii_case(b"infinity") || rest.eq_ignore_ascii_case(b"inf") {
            let inf = if negative {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            };
            return Some(Num::Float(inf));
        }
        let nan = rest.strip_prefix(b"s").or_else(|| rest.strip_prefix(b"S"));
        let nan = nan.unwrap_or(rest);
        let payload = nan
            .get(3..)
            .filter(|_| nan[..3].eq_ignore_ascii_case(b"nan"))?;
        // jq refuses a NaN with a payload, and takes one without as NaN.
        return payload
            .iter()
            .all(|b| *b == b'0')
            .then_some(Num::Float(f64::NAN));
    }
    if let Some((&e, exponent)) = rest[end..].split_first() {
        if !matches!(e, b'e' | b'E') {
            return None;
        }
        let exponent = match exponent.split_first() {
            Some((b'+' | b'-', digits)) => digits,
            _ => exponent,
        };
        if exponent.is_empty() || !exponent.iter().all(u8::is_ascii_digit) {
            return None;
        }
    }
    // The text is ASCII now.
    let text = core::str::from_utf8(text).ok()?;
    // A zero stays a decimal literal, keeping a negative zero's sign as decNumber does.
    Some(Num::from_str(text))
}

/// Parse one JSON text as `jv_parse_sized` does, for `fromjson` and `--jsonargs`: exactly one
/// value, else jq's message with `(while parsing '...')` after it.
pub fn parse_sized(text: &[u8]) -> Result<Val, String> {
    let mut parser = Parser::new();
    parser.set_buf(text, false);
    let result = match parser.next_value() {
        Next::Value(value) => match parser.next_value() {
            Next::Value(_) => Err("Unexpected extra JSON values".into()),
            Next::Error(error) => Err(error),
            Next::More => Ok(value),
        },
        Next::Error(error) => Err(error),
        Next::More => Err("Expected JSON value".into()),
    };
    result.map_err(|error| {
        let text = String::from_utf8_lossy(text);
        alloc::format!("{error} (while parsing '{text}')")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn parse_all(chunks: &[&[u8]]) -> Vec<Result<String, String>> {
        let mut parser = Parser::new();
        let mut out = Vec::new();
        for (i, chunk) in chunks.iter().enumerate() {
            parser.set_buf(chunk, i + 1 < chunks.len());
            loop {
                match parser.next_value() {
                    Next::Value(value) => out.push(Ok(value.to_string())),
                    Next::Error(error) => {
                        out.push(Err(error));
                        return out;
                    }
                    Next::More => break,
                }
            }
        }
        out
    }

    #[test]
    fn values_and_positions_match_jq() {
        assert_eq!(
            parse_all(&[b"1 2 x\n"]),
            vec![
                Ok("1".into()),
                Ok("2".into()),
                Err("Invalid numeric literal at line 2, column 0".into())
            ]
        );
        assert_eq!(
            parse_all(&[b"[1,\n", b"2,\n", b"}"]),
            vec![Err("Unmatched '}' at line 3, column 1".into())]
        );
        assert_eq!(
            parse_all(&[b"{\"a\":1"]),
            vec![Err("Unfinished JSON term at EOF at line 1, column 6".into())]
        );
        assert_eq!(
            parse_all(&[b"{\"a\" 1}"]),
            vec![Err(
                "Expected separator between values at line 1, column 7".into()
            )]
        );
        assert_eq!(
            parse_all(&[b"[1,", b"2]\n\"z\"\n"]),
            vec![Ok("[1,2]".into()), Ok("\"z\"".into())]
        );
        assert_eq!(
            parse_all(&[b"{\"b\":1,\"a\":2,\"b\":3}"]),
            vec![Ok("{\"b\":3,\"a\":2}".into())]
        );
    }

    #[test]
    fn bom_and_utf8_match_jq() {
        assert_eq!(parse_all(&[b"\xef\xbb", b"\xbf1"]), vec![Ok("1".into())]);
        assert_eq!(
            parse_all(&[b"\xef\xbb1"]),
            vec![Err("Malformed BOM".into())]
        );
        assert_eq!(
            parse_all(&[b"\"a\xffb\" \"\xed\xa0\x80\""]),
            vec![Ok("\"a\u{FFFD}b\"".into()), Ok("\"\u{FFFD}\"".into())]
        );
    }

    #[test]
    fn numbers_match_decnumber() {
        let show = |text: &str| number_literal(text.as_bytes()).map(|n| n.to_string());
        assert_eq!(show("-0").as_deref(), Some("-0"));
        assert_eq!(show("1.50").as_deref(), Some("1.50"));
        assert_eq!(show("+1").as_deref(), Some("1"));
        assert_eq!(show(".5").as_deref(), Some("0.5"));
        assert_eq!(show("5.").as_deref(), Some("5"));
        assert_eq!(show("1e3").as_deref(), Some("1E+3"));
        assert_eq!(show("nan").as_deref(), Some("null"));
        assert_eq!(show("sNaN0").as_deref(), Some("null"));
        assert_eq!(
            show("-Infinity").as_deref(),
            Some("-1.7976931348623157e+308")
        );
        for bad in [
            "nan5", "", "-", ".", "1.5.6", "0x10", " 1", "1 ", "1e", "1e+", "infinit",
        ] {
            assert_eq!(show(bad), None, "{bad}");
        }
    }

    #[test]
    fn seq_matches_jq() {
        let seq = |input: &[u8]| {
            let mut parser = Parser::with_seq(true);
            parser.set_buf(input, false);
            let mut out = Vec::new();
            loop {
                match parser.next_value() {
                    Next::Value(value) => out.push(Ok(value.to_string())),
                    Next::Error(error) => out.push(Err(error)),
                    Next::More if parser.remaining() > 0 => (),
                    Next::More => break,
                }
            }
            out
        };
        assert_eq!(
            seq(b"\x1e1\n\x1e[2]\n"),
            vec![Ok("1".into()), Ok("[2]".into())]
        );
        assert_eq!(
            seq(b"1\n"),
            vec![Err(
                "Unfinished abandoned text at EOF at line 2, column 0".into()
            )]
        );
        assert_eq!(
            seq(b"\x1e1\x1e2\n"),
            vec![
                Err("Potentially truncated top-level numeric value at line 1, column 3".into()),
                Ok("2".into())
            ]
        );
        assert_eq!(
            seq(b"\x1e[1,}\n\x1e3\n"),
            vec![
                Err("Unmatched '}' at line 1, column 5 (need RS to resync)".into()),
                Ok("3".into())
            ]
        );
    }

    #[test]
    fn parse_sized_matches_jq() {
        assert_eq!(
            parse_sized(b"1 2").unwrap_err(),
            "Unexpected extra JSON values (while parsing '1 2')"
        );
        assert_eq!(
            parse_sized(b"{").unwrap_err(),
            "Unfinished JSON term at EOF at line 1, column 1 (while parsing '{')"
        );
        assert_eq!(
            parse_sized(b"").unwrap_err(),
            "Expected JSON value (while parsing '')"
        );
        assert_eq!(parse_sized(b"nan").unwrap().to_string(), "null");
    }
}
