//! jq 1.8's UTF-8 rules (`src/jv_unicode.c`), for text that jq decodes itself: its JSON reader,
//! its lexer's string escapes, and raw input lines.
//!
//! jq keeps every string valid UTF-8. Where bytes are not, it substitutes U+FFFD per
//! `jvp_utf8_next`: one replacement for each bad lead or stray continuation byte, and one for a
//! whole sequence that is overlong, a surrogate, above U+10FFFF, or cut short. This is not
//! Unicode's "maximal subpart" rule that [`String::from_utf8_lossy`] follows (`ED A0 80` is one
//! U+FFFD here, three there).
use alloc::string::String;
use alloc::vec::Vec;

/// The length of the sequence a lead byte starts: 0 for a byte no sequence starts with, and
/// `None` for a continuation byte.
const fn coding_length(byte: u8) -> Option<usize> {
    match byte {
        0x00..=0x7F => Some(1),
        0x80..=0xBF => None,
        0xC0 | 0xC1 | 0xF5..=0xFF => Some(0),
        0xC2..=0xDF => Some(2),
        0xE0..=0xEF => Some(3),
        0xF0..=0xF4 => Some(4),
    }
}

/// Decode the next code point, as `jvp_utf8_next`: the number of bytes it takes and the code
/// point, or `None` where jq substitutes U+FFFD. `bytes` must not be empty.
#[must_use]
pub fn next(bytes: &[u8]) -> (usize, Option<u32>) {
    let first = bytes[0];
    let length = match coding_length(first) {
        Some(1) => return (1, Some(u32::from(first))),
        Some(0) | None => return (1, None),
        Some(length) if length > bytes.len() => return (bytes.len(), None),
        Some(length) => length,
    };
    let bits = match length {
        2 => 0x1F,
        3 => 0x0F,
        _ => 0x07,
    };
    let mut codepoint = u32::from(first & bits);
    for (i, &byte) in bytes.iter().enumerate().take(length).skip(1) {
        if coding_length(byte).is_some() {
            return (i, None);
        }
        codepoint = (codepoint << 6) | u32::from(byte & 0x3F);
    }
    let first_codepoint = match length {
        2 => 0x80,
        3 => 0x800,
        _ => 0x10000,
    };
    let valid = codepoint >= first_codepoint
        && !(0xD800..=0xDFFF).contains(&codepoint)
        && codepoint <= 0x10FFFF;
    (length, valid.then_some(codepoint))
}

/// How many bytes the character that `bytes` ends in still lacks, as `jvp_utf8_backtrack` counts
/// them (jq reads that many more so an input chunk does not split a character), or `None` when
/// the end is not part of a well-started sequence.
#[must_use]
pub fn missing(bytes: &[u8]) -> Option<usize> {
    let mut start = bytes.len().checked_sub(1)?;
    let mut seen = 1;
    while coding_length(bytes[start]).is_none() && start > 0 {
        start -= 1;
        seen += 1;
    }
    match coding_length(bytes[start]) {
        Some(length) if length > 0 => length.checked_sub(seen),
        _ => None,
    }
}

/// Append a code point's UTF-8 bytes as `jvp_utf8_encode` does: surrogates too, which
/// [`lossy`] later turns into U+FFFD.
pub fn encode(codepoint: u32, out: &mut Vec<u8>) {
    // The casts keep the low bits of each six-bit group, as jq's encoder does.
    #[allow(clippy::cast_possible_truncation)]
    match codepoint {
        0..=0x7F => out.push(codepoint as u8),
        0x80..=0x7FF => out.extend([
            0xC0 | (codepoint >> 6) as u8,
            0x80 | (codepoint & 0x3F) as u8,
        ]),
        0x800..=0xFFFF => out.extend([
            0xE0 | (codepoint >> 12) as u8,
            0x80 | ((codepoint >> 6) & 0x3F) as u8,
            0x80 | (codepoint & 0x3F) as u8,
        ]),
        _ => out.extend([
            0xF0 | (codepoint >> 18) as u8,
            0x80 | ((codepoint >> 12) & 0x3F) as u8,
            0x80 | ((codepoint >> 6) & 0x3F) as u8,
            0x80 | (codepoint & 0x3F) as u8,
        ]),
    }
}

/// The text jq makes of `bytes` (`jv_string_sized`): unchanged when they are valid UTF-8, else
/// with U+FFFD substituted as [`next`] decides.
#[must_use]
pub fn lossy(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        // Rust's UTF-8 is jq's: both reject overlong forms, surrogates and code points past
        // U+10FFFF, so only invalid input needs jq's own walk.
        Ok(text) => text,
        Err(error) => {
            let bytes = error.into_bytes();
            let mut text = String::with_capacity(bytes.len());
            let mut rest = &bytes[..];
            while !rest.is_empty() {
                let (length, codepoint) = next(rest);
                text.push(codepoint.and_then(char::from_u32).unwrap_or('\u{FFFD}'));
                rest = &rest[length..];
            }
            text
        }
    }
}

#[test]
fn lossy_matches_jq() {
    let lossy = |bytes: &[u8]| lossy(bytes.to_vec());
    assert_eq!(lossy(b"a\xffb"), "a\u{FFFD}b");
    assert_eq!(lossy(b"\xc3"), "\u{FFFD}");
    assert_eq!(lossy(b"\xe2\x82"), "\u{FFFD}");
    assert_eq!(lossy(b"\xf0\x9f\x98"), "\u{FFFD}");
    assert_eq!(lossy(b"\xc3("), "\u{FFFD}(");
    assert_eq!(lossy(b"\xed\xa0\x80"), "\u{FFFD}");
    assert_eq!(lossy(b"\xc0\xaf"), "\u{FFFD}\u{FFFD}");
    assert_eq!(lossy(b"\xf4\x90\x80\x80"), "\u{FFFD}");
    assert_eq!(lossy("é😀".as_bytes()), "é😀");
    let mut surrogate = Vec::new();
    encode(0xDC00, &mut surrogate);
    assert_eq!(lossy(&surrogate), "\u{FFFD}");
}

#[test]
fn missing_matches_jq() {
    assert_eq!(missing(b"ab"), Some(0));
    assert_eq!(missing(b"a\xe2\x82"), Some(1));
    assert_eq!(missing(b"a\xf0"), Some(3));
    assert_eq!(missing("é".as_bytes()), Some(0));
    assert_eq!(missing(b"\x82\x82"), None);
    assert_eq!(missing(b"a\xc3\xa9\xa9"), None);
}
