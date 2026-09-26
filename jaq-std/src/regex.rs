//! Helpers to interface with the `regex` crate.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use bstr::ByteSlice;
use regex_bites::bytes::{self as regex, Error, Regex, RegexBuilder};

#[derive(Copy, Clone, Default)]
pub struct Flags {
    // global search
    g: bool,
    // ignore empty matches
    n: bool,
    // case-insensitive
    i: bool,
    // multi-line mode: ^ and $ match begin/end of line
    m: bool,
    // single-line mode: allow . to match \n
    s: bool,
    // greedy
    l: bool,
    // extended mode: ignore whitespace and allow line comments (starting with `#`)
    x: bool,
}

impl Flags {
    pub fn new(flags: &str) -> Result<Self, char> {
        let mut out = Self::default();
        for flag in flags.chars() {
            match flag {
                'g' => out.g = true,
                'n' => out.n = true,
                'i' => out.i = true,
                'm' => out.m = true,
                's' => out.s = true,
                'l' => out.l = true,
                'x' => out.x = true,
                'p' => {
                    out.m = true;
                    out.s = true;
                }
                c => return Err(c),
            }
        }
        Ok(out)
    }

    pub fn ignore_empty(self) -> bool {
        self.n
    }

    pub fn global(self) -> bool {
        self.g
    }

    fn impact(self, builder: &mut RegexBuilder) -> &mut RegexBuilder {
        builder
            .case_insensitive(self.i)
            .multi_line(self.m)
            .dot_matches_new_line(self.s)
            .swap_greed(self.l)
            .ignore_whitespace(self.x)
    }

    pub fn regex(self, re: &str) -> Result<Regex, Error> {
        // `regex_bites` ignores the case of ASCII letters only; jq (Oniguruma) of any letter.
        let unicode;
        let re = if self.i && !re.is_ascii() {
            unicode = unicode_case_insensitive(re);
            unicode.as_str()
        } else {
            re
        };
        let mut builder = RegexBuilder::new(re);
        self.impact(&mut builder).build()
    }
}

/// The other cases of `c` that are one character each, in the simple case mapping.
fn other_cases(c: char) -> Vec<char> {
    let mut cases = Vec::new();
    for case in [
        c.to_lowercase().collect::<Vec<_>>(),
        c.to_uppercase().collect::<Vec<_>>(),
    ] {
        if let [other] = case[..] {
            if other != c && !cases.contains(&other) {
                cases.push(other);
            }
        }
    }
    cases
}

/// `re` with each letter beyond ASCII matching its other cases too: a literal one becomes a
/// class of its cases, and one in a class (not an end of a range) is joined by its cases.
fn unicode_case_insensitive(re: &str) -> String {
    let mut out = String::with_capacity(re.len());
    let mut chars = re.chars().peekable();
    // How deep in classes, and whether the class just opened (where `]` is a member).
    let mut depth = 0_usize;
    let mut class_start = false;
    let mut previous = None;
    while let Some(c) = chars.next() {
        let starting = core::mem::replace(&mut class_start, false);
        match c {
            '\\' => {
                out.push(c);
                if let Some(next) = chars.next() {
                    out.push(next);
                    // `\p{...}`, `\x{...}`, `\u{...}`: the braces are part of the escape.
                    if chars.peek() == Some(&'{') {
                        for braced in chars.by_ref() {
                            out.push(braced);
                            if braced == '}' {
                                break;
                            }
                        }
                    }
                }
                previous = None;
                continue;
            }
            '[' if depth > 0 && chars.peek() == Some(&':') => {
                // A POSIX class, `[:alpha:]`, is copied whole.
                out.push(c);
                let mut prev = c;
                for next in chars.by_ref() {
                    out.push(next);
                    if prev == ':' && next == ']' {
                        break;
                    }
                    prev = next;
                }
            }
            '[' => {
                out.push(c);
                depth += 1;
                if chars.peek() == Some(&'^') {
                    out.push('^');
                    chars.next();
                }
                class_start = true;
            }
            ']' if depth > 0 && !starting => {
                out.push(c);
                depth -= 1;
            }
            c if c.is_ascii() => out.push(c),
            c => {
                let cases = other_cases(c);
                let in_range = previous == Some('-') || chars.peek() == Some(&'-');
                if cases.is_empty() || (depth > 0 && in_range) {
                    out.push(c);
                } else if depth > 0 {
                    out.push(c);
                    out.extend(cases);
                } else {
                    out.push('[');
                    out.push(c);
                    out.extend(cases);
                    out.push(']');
                }
            }
        }
        previous = Some(c);
    }
    out
}

type CharIndices<'a> =
    core::iter::Chain<bstr::CharIndices<'a>, core::iter::Once<(usize, usize, char)>>;

/// Mapping between byte and character indices.
pub struct ByteChar<'a>(core::iter::Peekable<core::iter::Enumerate<CharIndices<'a>>>);

impl<'a> ByteChar<'a> {
    pub fn new(s: &'a [u8]) -> Self {
        let last = core::iter::once((s.len(), 0, '\0'));
        Self(s.char_indices().chain(last).enumerate().peekable())
    }

    /// Convert byte offset to UTF-8 character offset.
    ///
    /// This needs to be called with monotonically increasing values of `byte_offset`.
    fn char_of_byte(&mut self, byte_offset: usize) -> Option<usize> {
        loop {
            let (char_i, (byte_i, _, _char)) = self.0.peek()?;
            if byte_offset == *byte_i {
                return Some(*char_i);
            } else {
                self.0.next();
            }
        }
    }
}

pub struct Match<B, S> {
    pub offset: usize,
    pub length: usize,
    pub string: B,
    pub name: Option<S>,
}

impl<'a> Match<&'a [u8], &'a str> {
    pub fn new(bc: &mut ByteChar, m: regex::Match<'a>, name: Option<&'a str>) -> Self {
        Self {
            offset: bc.char_of_byte(m.start()).unwrap(),
            length: m.as_bytes().chars().count(),
            string: m.as_bytes(),
            name,
        }
    }

    pub fn fields<T: From<isize> + From<String> + 'a>(
        &self,
        f: impl Fn(&'a [u8]) -> T,
    ) -> impl Iterator<Item = (T, T)> + '_ {
        [
            ("offset", (self.offset as isize).into()),
            ("length", (self.length as isize).into()),
            ("string", f(self.string)),
        ]
        .into_iter()
        .chain(self.name.iter().map(|n| ("name", (*n).to_string().into())))
        .map(|(k, v)| (k.to_string().into(), v))
    }
}

pub enum Part<B, S> {
    Matches(Vec<Match<B, S>>),
    Mismatch(B),
}

/// Apply a regular expression to the given input value.
///
/// `sm` indicates whether to
/// 1. output strings that do *not* match the regex, and
/// 2. output the matches.
pub fn regex<'a>(
    s: &'a [u8],
    re: &'a Regex,
    flags: Flags,
    sm: (bool, bool),
) -> Vec<Part<&'a [u8], &'a str>> {
    // mismatches & matches
    let (mi, ma) = sm;

    let mut last_byte = 0;
    let mut bc = ByteChar::new(s);
    let mut out = Vec::new();

    for c in re.captures_iter(s) {
        let whole = c.get(0).unwrap();
        if flags.ignore_empty() && whole.as_bytes().is_empty() {
            continue;
        }
        let match_names = c.iter().zip(re.capture_names());
        let matches = match_names.filter_map(|(m, n)| Some(Match::new(&mut bc, m?, n)));
        if mi {
            out.push(Part::Mismatch(&s[last_byte..whole.start()]));
            last_byte = whole.end();
        }
        if ma {
            out.push(Part::Matches(matches.collect()));
        }
        if !flags.global() {
            break;
        }
    }
    if mi {
        out.push(Part::Mismatch(&s[last_byte..]));
    }
    out
}
