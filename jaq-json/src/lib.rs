//! JSON superset with binary data and non-string object keys.
//!
//! This crate provides a few macros for formatting / writing;
//! this is done in order to function with both
//! [`core::fmt::Write`] and [`std::io::Write`].
#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

mod funs;
pub mod jv_parse;
mod num;
#[macro_use]
pub mod write;
pub mod read;

use alloc::{borrow::ToOwned, boxed::Box, string::String, vec::Vec};
use bstr::{BStr, ByteSlice};
use bytes::{BufMut, Bytes, BytesMut};
use core::cmp::Ordering;
use core::fmt;
use core::hash::{Hash, Hasher};
use jaq_core::box_iter::box_once;
use jaq_core::{load, path, val, Exn};

pub use funs::{bytes_valrs, funs};
pub use num::Num;

#[cfg(not(feature = "sync"))]
pub use alloc::rc::Rc;
#[cfg(feature = "sync")]
pub use alloc::sync::Arc as Rc;

#[cfg(feature = "serde")]
mod serde;

/// JSON superset with binary data and non-string object keys.
///
/// This is the default value type for jaq.
#[derive(Clone, Debug, Default)]
pub enum Val {
    #[default]
    /// Null
    Null,
    /// Boolean
    Bool(bool),
    /// Number
    Num(Num),
    /// Byte string
    BStr(Box<Bytes>),
    /// Text string (interpreted as UTF-8)
    ///
    /// Note that this does not require the actual bytes to be all valid UTF-8;
    /// this just means that the bytes are interpreted as UTF-8.
    /// An effort is made to preserve invalid UTF-8 as is, else
    /// replace invalid UTF-8 by the Unicode replacement character.
    TStr(Box<Bytes>),
    /// Array
    Arr(Rc<Array>),
    /// Object
    Obj(Rc<Object>),
}

#[cfg(feature = "sync")]
#[test]
fn val_send_sync() {
    fn send_sync<T: Send + Sync>(_: T) {}
    send_sync(Val::default())
}

#[cfg(target_arch = "x86_64")]
const _: () = {
    assert!(core::mem::size_of::<Val>() == 16);
};

/// Types and sets of types.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Type {
    /// `0 | fromjson` or `"a b c" | split(0)`
    Str,
    /// `0 | implode`
    Arr,
}

impl Type {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Str => "string",
            Self::Arr => "array",
        }
    }
}

/// A value's own jq type name (`type`'s own vocabulary: `null`/`boolean`/`number`/`string`/
/// `array`/`object`), as opposed to [`Type`] above, which names a *target* type a value failed
/// to convert to.
pub(crate) fn type_name(v: &Val) -> &'static str {
    match v {
        Val::Null => "null",
        Val::Bool(_) => "boolean",
        Val::Num(_) => "number",
        Val::BStr(_) | Val::TStr(_) => "string",
        Val::Arr(_) => "array",
        Val::Obj(_) => "object",
    }
}

/// A value in an error message, as jq's `jv_dump_string_trunc` gives it with the 30-byte
/// buffer its callers use: the compact JSON text, or when that is longer than 29 bytes, its first
/// 25 (26 without a closing delimiter) bytes, not splitting a character, then `...` and the
/// closing `"`, `]` or `}`.
pub fn dump_trunc(v: &Val) -> String {
    const BUFSIZE: usize = 30;
    let text = alloc::string::ToString::to_string(v);
    if text.len() < BUFSIZE {
        return text;
    }
    let delim = match text.as_bytes()[0] {
        b'"' => Some('"'),
        b'[' => Some(']'),
        b'{' => Some('}'),
        _ => None,
    };
    let mut len = BUFSIZE - if delim.is_some() { 5 } else { 4 };
    while !text.is_char_boundary(len) {
        len -= 1;
    }
    let mut out = String::from(&text[..len]);
    out.push_str("...");
    out.extend(delim);
    out
}

/// jq's `type_error`: `TYPE (VALUE) MESSAGE`.
pub(crate) fn type_error(v: &Val, message: &str) -> Error {
    Error::str(format_args!(
        "{} ({}) {message}",
        type_name(v),
        dump_trunc(v)
    ))
}

/// jq's `type_error2`: `KIND (VALUE) and KIND (VALUE) MESSAGE`.
pub(crate) fn type_error2(l: &Val, r: &Val, message: &str) -> Error {
    Error::str(format_args!(
        "{} ({}) and {} ({}) {message}",
        type_name(l),
        dump_trunc(l),
        type_name(r),
        dump_trunc(r)
    ))
}

/// jq's own wording for indexing a value by a key it cannot take — e.g. `{}[0]`, `[][{}]` or
/// `1 | .a` — is `Cannot index TYPE1 with TYPE2 (VALUE)`, the index's value cut short as
/// [`dump_trunc`] does. Verified against the oracle for every kind of value and index.
pub(crate) fn index_type_error(container: &Val, index: &Val) -> Error {
    Error::str(format_args!(
        "Cannot index {} with {} ({})",
        type_name(container),
        type_name(index),
        dump_trunc(index)
    ))
}

/// An array index as jq's `jv_get` takes it: truncated to an integer (`.[1.5]` is `.[1]`),
/// with NaN or a value past `int`'s range reading as `null`.
fn jq_array_index(i: &Num, len: usize) -> Option<usize> {
    if let Some(i) = i.as_pos_usize() {
        return abs_index(i, len);
    }
    let f = i.as_f64();
    if f.is_nan() || f.abs() >= 2147483648.0 {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    let i = f as isize;
    abs_index(num::PosUsize(i >= 0, i.unsigned_abs()), len)
}

/// Order-preserving map
pub type Map<K = Val, V = K> = indexmap::IndexMap<K, V, foldhash::fast::RandomState>;

/// The elements of an array.
///
/// This is a vector of values that is dropped without recursing into the arrays and objects it
/// holds (see [`drop_deep`]), so that a value nested arbitrarily deep, as a filter can build it,
/// can be dropped.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Array(Vec<Val>);

/// The entries of an object, dropped as [`Array`] is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Object(Map<Val, Val>);

/// Drop values without recursing: the elements of each array or object dropped last are moved
/// onto an explicit stack first.
fn drop_deep(mut stack: Vec<Val>) {
    while let Some(v) = stack.pop() {
        match v {
            Val::Arr(a) => {
                if let Ok(mut a) = Rc::try_unwrap(a) {
                    stack.append(&mut a.0);
                }
            }
            Val::Obj(o) => {
                if let Ok(mut o) = Rc::try_unwrap(o) {
                    stack.extend(
                        core::mem::take(&mut o.0)
                            .into_iter()
                            .flat_map(|(k, v)| [k, v]),
                    );
                }
            }
            _ => (),
        }
    }
}

const fn is_container(v: &Val) -> bool {
    matches!(v, Val::Arr(_) | Val::Obj(_))
}

impl Drop for Array {
    fn drop(&mut self) {
        if self.0.iter().any(is_container) {
            drop_deep(core::mem::take(&mut self.0));
        }
    }
}

impl Drop for Object {
    fn drop(&mut self) {
        if self.0.values().any(is_container) {
            let entries = core::mem::take(&mut self.0);
            drop_deep(entries.into_iter().flat_map(|(k, v)| [k, v]).collect());
        }
    }
}

impl core::ops::Deref for Array {
    type Target = Vec<Val>;
    fn deref(&self) -> &Vec<Val> {
        &self.0
    }
}

impl core::ops::DerefMut for Array {
    fn deref_mut(&mut self) -> &mut Vec<Val> {
        &mut self.0
    }
}

impl core::ops::Deref for Object {
    type Target = Map<Val, Val>;
    fn deref(&self) -> &Map<Val, Val> {
        &self.0
    }
}

impl core::ops::DerefMut for Object {
    fn deref_mut(&mut self) -> &mut Map<Val, Val> {
        &mut self.0
    }
}

impl From<Vec<Val>> for Array {
    fn from(v: Vec<Val>) -> Self {
        Self(v)
    }
}

impl From<Map<Val, Val>> for Object {
    fn from(m: Map<Val, Val>) -> Self {
        Self(m)
    }
}

impl FromIterator<Val> for Array {
    fn from_iter<I: IntoIterator<Item = Val>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl FromIterator<(Val, Val)> for Object {
    fn from_iter<I: IntoIterator<Item = (Val, Val)>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl IntoIterator for Array {
    type Item = Val;
    type IntoIter = alloc::vec::IntoIter<Val>;
    fn into_iter(mut self) -> Self::IntoIter {
        core::mem::take(&mut self.0).into_iter()
    }
}

impl IntoIterator for Object {
    type Item = (Val, Val);
    type IntoIter = indexmap::map::IntoIter<Val, Val>;
    fn into_iter(mut self) -> Self::IntoIter {
        core::mem::take(&mut self.0).into_iter()
    }
}

impl<'a> IntoIterator for &'a Array {
    type Item = &'a Val;
    type IntoIter = core::slice::Iter<'a, Val>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<'a> IntoIterator for &'a Object {
    type Item = (&'a Val, &'a Val);
    type IntoIter = indexmap::map::Iter<'a, Val, Val>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

/// Error that can occur during filter execution.
pub type Error = jaq_core::Error<Val>;
/// A value or an eRror.
pub type ValR = jaq_core::ValR<Val>;
/// A value or an eXception.
pub type ValX<'a> = jaq_core::ValX<'a, Val>;

// This is part of the Rust standard library since 1.76:
// <https://doc.rust-lang.org/std/rc/struct.Rc.html#method.unwrap_or_clone>.
// However, to keep MSRV low, we reimplement it here.
fn rc_unwrap_or_clone<T: Clone>(a: Rc<T>) -> T {
    Rc::try_unwrap(a).unwrap_or_else(|a| (*a).clone())
}

impl jaq_core::ValT for Val {
    fn from_num(n: &str) -> ValR {
        Ok(Self::Num(Num::from_str(n)))
    }

    fn from_map<I: IntoIterator<Item = (Self, Self)>>(iter: I) -> ValR {
        let mut map = Map::default();
        for (k, v) in iter {
            // jq builds objects with string keys only.
            if !matches!(k, Val::TStr(_) | Val::BStr(_)) {
                return Err(Error::str(format_args!(
                    "Cannot use {} ({}) as object key",
                    type_name(&k),
                    dump_trunc(&k)
                )));
            }
            map.insert(k, v);
        }
        Ok(Self::obj(map))
    }

    fn key_values(self) -> Box<dyn Iterator<Item = Result<(Val, Val), Error>>> {
        let arr_idx = |(i, x)| Ok((Self::from(i as isize), x));
        match self {
            Self::Arr(a) => Box::new(rc_unwrap_or_clone(a).into_iter().enumerate().map(arr_idx)),
            Self::Obj(o) => Box::new(rc_unwrap_or_clone(o).into_iter().map(Ok)),
            _ => box_once(Err(jaq_core::ValT::iterate_error(&self))),
        }
    }

    fn values(self) -> Box<dyn Iterator<Item = ValR>> {
        match self {
            Self::Arr(a) => Box::new(rc_unwrap_or_clone(a).into_iter().map(Ok)),
            Self::Obj(o) => Box::new(rc_unwrap_or_clone(o).into_iter().map(|(_k, v)| Ok(v))),
            _ => box_once(Err(jaq_core::ValT::iterate_error(&self))),
        }
    }

    fn index(self, index: &Self) -> ValR {
        self.index_opt(index).map(|o| o.unwrap_or(Val::Null))
    }

    fn range(self, range: val::Range<&Self>) -> ValR {
        // jq slices null to null.
        if let Val::Null = self {
            return Ok(Val::Null);
        }
        let fs = |b: Bytes, range, skip_take: SkipTakeFn| {
            skip_take(range, &b).map(|(skip, take)| b.slice(skip..skip + take))
        };
        match self {
            Val::BStr(b) => fs(*b, range, skip_take_bytes).map(Val::byte_str),
            Val::TStr(b) => fs(*b, range, skip_take_chars).map(Val::utf8_str),
            Val::Arr(a) => slice_bounds(range, a.len())
                .map(|(skip, take)| a.iter().skip(skip).take(take).cloned().collect()),
            _ => Err(index_type_error(&self, &slice_object(range))),
        }
    }

    fn map_values<'a, I: Iterator<Item = ValX<'a>>>(
        self,
        opt: path::Opt,
        f: impl Fn(Self) -> I,
    ) -> ValX<'a> {
        match self {
            Self::Arr(a) => {
                let iter = rc_unwrap_or_clone(a).into_iter().flat_map(f);
                Ok(iter.collect::<Result<_, _>>()?)
            }
            Self::Obj(o) => {
                let iter = rc_unwrap_or_clone(o).into_iter();
                let iter = iter.filter_map(|(k, v)| f(v).next().map(|v| Ok((k, v?))));
                Ok(Self::obj(iter.collect::<Result<_, Exn<_>>>()?))
            }
            v => opt.fail(v, |v| Exn::from(jaq_core::ValT::iterate_error(&v))),
        }
    }

    fn map_index<'a, I: Iterator<Item = ValX<'a>>>(
        mut self,
        index: &Self,
        opt: path::Opt,
        f: impl Fn(Self) -> I,
    ) -> ValX<'a> {
        if let (Val::BStr(_) | Val::TStr(_) | Val::Arr(_), Val::Obj(o)) = (&self, index) {
            let range = o.get(&Val::utf8_str("start"))..o.get(&Val::utf8_str("end"));
            return self.map_range(range, opt, f);
        };
        match self {
            // jq treats null as an empty object or array when updating it by key or index.
            Val::Null => match index {
                Val::TStr(_) | Val::BStr(_) => Val::obj(Map::default()).map_index(index, opt, f),
                Val::Num(_) => Val::Arr(Rc::default()).map_index(index, opt, f),
                _ => opt.fail(self, |v| Exn::from(index_type_error(&v, index))),
            },
            // jq reads the field first, and only a string names an object's field.
            Val::Obj(_) if !matches!(index, Val::TStr(_) | Val::BStr(_)) => {
                opt.fail(self, |v| Exn::from(index_type_error(&v, index)))
            }
            Val::Obj(ref mut o) => {
                use indexmap::map::Entry::{Occupied, Vacant};
                match Rc::make_mut(o).entry(index.clone()) {
                    Occupied(mut e) => {
                        let v = core::mem::take(e.get_mut());
                        match f(v).next().transpose()? {
                            Some(y) => e.insert(y),
                            // this runs in constant time, at the price of
                            // changing the order of the elements
                            None => e.swap_remove(),
                        };
                    }
                    Vacant(e) => {
                        if let Some(y) = f(Val::Null).next().transpose()? {
                            e.insert(y);
                        }
                    }
                }
                Ok(self)
            }
            Val::Arr(_) if !matches!(index, Val::Num(_)) => {
                opt.fail(self, |v| Exn::from(index_type_error(&v, index)))
            }
            Val::Arr(ref mut a) => {
                let oob = || Error::str("Out of bounds negative array index");
                let len = a.len();
                // As jq's `jv_set`: the index truncated to an integer (NaN refused), and a
                // non-negative index past the end extends the array with nulls.
                let i = match index {
                    Val::Num(n) if n.as_f64().is_nan() => {
                        let e = Error::str("Cannot set array element at NaN index");
                        return opt.fail(self, |_| Exn::from(e));
                    }
                    Val::Num(n) => {
                        let d = n.as_f64().clamp(f64::from(i32::MIN), f64::from(i32::MAX));
                        #[allow(clippy::cast_possible_truncation)]
                        let i = d as isize;
                        num::PosUsize(i >= 0, i.unsigned_abs())
                    }
                    _ => num::PosUsize(true, 0),
                };
                let abs_or = |i: num::PosUsize| match i {
                    num::PosUsize(true, i) => Ok(i),
                    i => abs_index(i, len).ok_or_else(oob),
                };
                let i = match abs_or(i) {
                    Ok(i) => i,
                    Err(e) => return opt.fail(self, |_| Exn::from(e)),
                };

                // jq refuses to grow an array past `INT_MAX >> 2` elements rather than allocate.
                if i > (i32::MAX >> 2) as usize {
                    let e = Error::str("Array index too large");
                    return opt.fail(self, |_| Exn::from(e));
                }
                // Deleting past the end leaves the array as it is.
                if i >= a.len() {
                    match f(Val::Null).next().transpose()? {
                        None => return Ok(self),
                        Some(y) => {
                            let a = Rc::make_mut(a);
                            a.resize(i, Val::Null);
                            a.push(y);
                            return Ok(self);
                        }
                    }
                }
                let a = Rc::make_mut(a);
                let x = core::mem::take(&mut a[i]);
                if let Some(y) = f(x).next().transpose()? {
                    a[i] = y;
                } else {
                    a.remove(i);
                }
                Ok(self)
            }
            _ => opt.fail(self, |v| Exn::from(index_type_error(&v, index))),
        }
    }

    fn map_range<'a, I: Iterator<Item = ValX<'a>>>(
        mut self,
        range: val::Range<&Self>,
        opt: path::Opt,
        f: impl Fn(Self) -> I,
    ) -> ValX<'a> {
        let fs = |b: Bytes, range, skip_take: SkipTakeFn, from: ValBytesFn, into: BytesValFn| {
            let (skip, take) = match skip_take(range, &b) {
                Ok(bounds) => bounds,
                Err(e) => return opt.fail(into(b), |_| Exn::from(e)),
            };
            let str = into(b.slice(skip..skip + take));
            let y = f(str).map(|y| from(y?).map_err(Exn::from)).next();
            let y = y.transpose()?.unwrap_or_default();
            let mut b = BytesMut::from(b);
            bytes_splice(&mut b, skip, take, &y);
            Ok(into(b.freeze()))
        };
        let stb = skip_take_bytes;
        let stc = skip_take_chars;
        match self {
            Val::Arr(ref mut a) => {
                let (skip, take) = match slice_bounds(range, a.len()) {
                    Ok(bounds) => bounds,
                    Err(e) => return opt.fail(self, |_| Exn::from(e)),
                };
                let arr = a.iter().skip(skip).take(take).cloned().collect();
                let y = f(arr).map(|y| y?.into_arr().map_err(Exn::from)).next();
                let y = y.transpose()?.unwrap_or_default();
                Rc::make_mut(a).splice(skip..skip + take, (*y).clone());
                Ok(self)
            }
            Val::BStr(b) => fs(*b, range, stb, Val::into_byte_str, Val::byte_str),
            Val::TStr(b) => fs(*b, range, stc, Val::into_utf8_str, Val::utf8_str),
            _ => {
                let slice = slice_object(range);
                opt.fail(self, |v| Exn::from(index_type_error(&v, &slice)))
            }
        }
    }

    /// True if the value is neither null nor false.
    fn as_bool(&self) -> bool {
        !matches!(self, Self::Null | Self::Bool(false))
    }

    fn into_string(self) -> Self {
        match self {
            Self::BStr(b) | Self::TStr(b) => Self::TStr(b),
            _ => Self::utf8_str(self.to_json()),
        }
    }

    fn kind_name(&self) -> &'static str {
        type_name(self)
    }

    fn dump_trunc(&self) -> String {
        dump_trunc(self)
    }
}

impl jaq_std::ValT for Val {
    fn into_seq<S: FromIterator<Self>>(self) -> Result<S, Self> {
        match self {
            Self::Arr(a) => match Rc::try_unwrap(a) {
                Ok(a) => Ok(a.into_iter().collect()),
                Err(a) => Ok(a.iter().cloned().collect()),
            },
            _ => Err(self),
        }
    }

    fn is_int(&self) -> bool {
        self.as_num().is_some_and(Num::is_int)
    }

    fn as_isize(&self) -> Option<isize> {
        self.as_num().and_then(Num::as_isize)
    }

    fn as_f64(&self) -> Option<f64> {
        self.as_num().map(Num::as_f64)
    }

    fn is_utf8_str(&self) -> bool {
        matches!(self, Self::TStr(_))
    }

    fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::BStr(b) | Self::TStr(b) => Some(b),
            _ => None,
        }
    }

    fn as_sub_str(&self, sub: &[u8]) -> Self {
        match self {
            Self::BStr(b) => Self::byte_str(b.slice_ref(sub)),
            Self::TStr(b) => Self::utf8_str(b.slice_ref(sub)),
            _ => panic!(),
        }
    }

    fn from_utf8_bytes(b: impl AsRef<[u8]> + Send + 'static) -> Self {
        Self::utf8_str(Bytes::from_owner(b))
    }
}

/// Definitions of the standard library.
pub fn defs() -> impl Iterator<Item = load::parse::Def<&'static str>> {
    load::parse(include_str!("defs.jq"), |p| p.defs())
        .unwrap()
        .into_iter()
}

type ValBytesFn = fn(Val) -> Result<Bytes, Error>;
type BytesValFn = fn(Bytes) -> Val;
type SkipTakeFn = fn(val::Range<&Val>, &[u8]) -> Result<(usize, usize), Error>;

/// jq's `parse_slice`: where a slice of `len` elements starts and how many it takes. The bounds
/// are numbers (or null for the ends); a negative one counts from the end, the start is rounded
/// down and the end up, and both are clamped to the elements.
fn slice_bounds(range: val::Range<&Val>, len: usize) -> Result<(usize, usize), Error> {
    let bound = |bound: Option<&Val>, default: f64| match bound {
        None | Some(Val::Null) => Ok(default),
        Some(Val::Num(n)) => Ok(n.as_f64()),
        Some(_) => Err(Error::str("Array/string slice indices must be integers")),
    };
    #[allow(clippy::cast_precision_loss)]
    let flen = len as f64;
    let mut dstart = bound(range.start, 0.0)?;
    let mut dend = bound(range.end, flen)?;
    if dstart.is_nan() {
        dstart = 0.0;
    }
    if dstart < 0.0 {
        dstart += flen;
    }
    let dstart = dstart.clamp(0.0, flen);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let start = dstart as usize;
    if dend.is_nan() {
        dend = flen;
    }
    if dend < 0.0 {
        dend += flen;
    }
    if dend < 0.0 {
        #[allow(clippy::cast_precision_loss)]
        let start = start as f64;
        dend = start;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let mut end = (dend.min(f64::from(i32::MAX)) as usize).min(len);
    #[allow(clippy::cast_precision_loss)]
    if end < len && (end as f64) < dend {
        end += 1;
    }
    let end = end.max(start);
    Ok((start, end - start))
}

/// The object jq indexes by for a slice: `{"start": ..., "end": ...}`.
fn slice_object(range: val::Range<&Val>) -> Val {
    let bound = |b: Option<&Val>| b.cloned().unwrap_or(Val::Null);
    Val::obj(Map::from_iter([
        (Val::utf8_str("start"), bound(range.start)),
        (Val::utf8_str("end"), bound(range.end)),
    ]))
}

fn skip_take_bytes(range: val::Range<&Val>, b: &[u8]) -> Result<(usize, usize), Error> {
    slice_bounds(range, b.len())
}

fn skip_take_chars(range: val::Range<&Val>, b: &[u8]) -> Result<(usize, usize), Error> {
    let (skip, take) = slice_bounds(range, b.chars().count())?;
    let byte_index = |c: usize| b.char_indices().nth(c).map_or(b.len(), |(i, ..)| i);
    let from = byte_index(skip);
    let upto = byte_index(skip + take);
    Ok((from, upto - from))
}

fn bytes_splice(b: &mut BytesMut, skip: usize, take: usize, replace: &[u8]) {
    let final_len = b.len() - take + replace.len();
    let post_take = skip + take..b.len();

    if replace.len() > take {
        b.resize(final_len, 0);
    }
    b.copy_within(post_take, skip + replace.len());
    b[skip..skip + replace.len()].copy_from_slice(replace);
    if replace.len() < take {
        b.truncate(final_len);
    }
}

/// If a range bound is given, absolutise and clip it between 0 and `len`,
/// else return `default`.
/// Absolutise an index and return result if it is inside [0, len).
fn abs_index(i: num::PosUsize, len: usize) -> Option<usize> {
    i.wrap(len).filter(|i| *i < len)
}

impl Val {
    /// Construct an object value.
    pub fn obj(m: Map) -> Self {
        Self::Obj(Rc::new(Object(m)))
    }

    /// Construct an array value.
    pub fn arr(v: Vec<Self>) -> Self {
        Self::Arr(Rc::new(Array(v)))
    }

    /// Construct a string that is interpreted as UTF-8.
    pub fn utf8_str(s: impl Into<Bytes>) -> Self {
        Self::TStr(s.into().into())
    }

    /// Construct a string that is interpreted as bytes.
    pub fn byte_str(s: impl Into<Bytes>) -> Self {
        Self::BStr(s.into().into())
    }

    fn as_num(&self) -> Option<&Num> {
        match self {
            Self::Num(n) => Some(n),
            _ => None,
        }
    }

    fn into_byte_str(self) -> Result<Bytes, Error> {
        match self {
            Self::BStr(b) => Ok(*b),
            _ => Err(Error::typ(self, Type::Str.as_str())),
        }
    }

    fn into_utf8_str(self) -> Result<Bytes, Error> {
        match self {
            Self::TStr(b) => Ok(*b),
            _ => Err(Error::typ(self, Type::Str.as_str())),
        }
    }

    /// If the value is an array, return it, else fail.
    fn into_arr(self) -> Result<Rc<Array>, Error> {
        match self {
            Self::Arr(a) => Ok(a),
            _ => Err(Error::typ(self, Type::Arr.as_str())),
        }
    }

    fn as_arr(&self) -> Result<&Rc<Array>, Error> {
        match self {
            Self::Arr(a) => Ok(a),
            _ => Err(Error::typ(self.clone(), Type::Arr.as_str())),
        }
    }

    fn to_json(&self) -> Vec<u8> {
        write::to_json(self)
    }

    fn index_opt(self, index: &Self) -> Result<Option<Val>, Error> {
        Ok(match (self, index) {
            (Val::Null, _) => None,
            (Val::BStr(a), Val::Num(i)) => {
                jq_array_index(i, a.len()).map(|i| usize::from(a[i]).into())
            }
            (Val::Arr(a), Val::Num(i)) => jq_array_index(i, a.len()).map(|i| a[i].clone()),
            (Val::Arr(_), Val::Arr(y)) if y.is_empty() => Some(Val::Arr(Default::default())),
            (Val::Arr(x), Val::Arr(y)) => {
                // adapted from the implementation of the `indices` filter
                let iw = x.windows(y.len()).enumerate();
                let indices = iw.filter_map(|(i, w)| (w == &y[..]).then_some(i));
                Some(indices.map(Val::from).collect())
            }
            // jq only ever indexes an object by a string key; a non-string index (`{}[0]`) is a
            // type error there, not a (guaranteed) miss that quietly reads as `null` — even
            // though jaq objects can technically hold non-string keys (from `{(0): 1}`-style
            // construction). This build follows jq's behavior rather than jaq's own, since jq's
            // behavior is the bar the tool is advertised against; see `index_type_error`'s doc
            // comment for the exact wording, verified against the oracle for every index type.
            (Val::Obj(o), i @ (Val::TStr(_) | Val::BStr(_))) => o.get(i).cloned(),
            (s @ Val::Obj(_), i) => return Err(index_type_error(&s, i)),
            (v @ (Val::BStr(_) | Val::TStr(_) | Val::Arr(_)), Val::Obj(o)) => {
                use jaq_core::ValT;
                let start = o.get(&Val::utf8_str("start"));
                let end = o.get(&Val::utf8_str("end"));
                return v.range(start..end).map(Some);
            }
            (s, _) => return Err(index_type_error(&s, index)),
        })
    }
}

impl From<bool> for Val {
    fn from(b: bool) -> Self {
        Self::Bool(b)
    }
}

impl From<isize> for Val {
    fn from(i: isize) -> Self {
        Self::Num(Num::Int(i))
    }
}

impl From<usize> for Val {
    fn from(i: usize) -> Self {
        Self::Num(Num::from_integral(i))
    }
}

impl From<f64> for Val {
    fn from(f: f64) -> Self {
        Self::Num(Num::Float(f))
    }
}

impl From<String> for Val {
    fn from(s: String) -> Self {
        Self::utf8_str(Bytes::from_owner(s))
    }
}

impl From<val::Range<Val>> for Val {
    fn from(r: val::Range<Val>) -> Self {
        let kv = |(k, v): (&str, Option<_>)| v.map(|v| (k.to_owned().into(), v));
        let kvs = [("start", r.start), ("end", r.end)];
        Val::obj(kvs.into_iter().flat_map(kv).collect())
    }
}

impl FromIterator<Self> for Val {
    fn from_iter<T: IntoIterator<Item = Self>>(iter: T) -> Self {
        Self::Arr(Rc::new(iter.into_iter().collect()))
    }
}

impl core::ops::Add for Val {
    type Output = ValR;
    fn add(self, rhs: Self) -> Self::Output {
        let concat_bytes = |l, r| {
            let mut buf = BytesMut::from(l);
            buf.put(r);
            buf
        };
        use Val::*;
        match (self, rhs) {
            // `null` is a neutral element for addition
            (Null, x) | (x, Null) => Ok(x),
            (Num(x), Num(y)) => Ok(Num(x + y)),
            (BStr(l), BStr(r)) => Ok(Val::byte_str(concat_bytes(*l, r))),
            (TStr(l), TStr(r)) => Ok(Val::utf8_str(concat_bytes(*l, r))),
            (Arr(mut l), Arr(r)) => {
                //std::dbg!(Rc::strong_count(&l));
                Rc::make_mut(&mut l).extend(r.iter().cloned());
                Ok(Arr(l))
            }
            (Obj(mut l), Obj(r)) => {
                Rc::make_mut(&mut l).extend(r.iter().map(|(k, v)| (k.clone(), v.clone())));
                Ok(Obj(l))
            }
            (l, r) => Err(type_error2(&l, &r, "cannot be added")),
        }
    }
}

impl core::ops::Sub for Val {
    type Output = ValR;
    fn sub(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Self::Num(x), Self::Num(y)) => Ok(Self::Num(x - y)),
            (Self::Arr(mut l), Self::Arr(r)) => {
                let r = r.iter().collect::<alloc::collections::BTreeSet<_>>();
                Rc::make_mut(&mut l).retain(|x| !r.contains(x));
                Ok(Self::Arr(l))
            }
            (l, r) => Err(type_error2(&l, &r, "cannot be subtracted")),
        }
    }
}

/// Merge `r` into `l` recursively (`l * r`), without recursing once per level of nesting: an
/// object being merged into waits on a stack, its merged entry taken out (left `null`) until its
/// merge is done.
fn obj_merge(l: &mut Rc<Object>, r: Rc<Object>) {
    struct Frame {
        l: Object,
        r: <Object as IntoIterator>::IntoIter,
        key: Option<Val>,
    }
    let mut stack = Vec::from([Frame {
        l: core::mem::take(Rc::make_mut(l)),
        r: rc_unwrap_or_clone(r).into_iter(),
        key: None,
    }]);
    loop {
        let Some(top) = stack.last_mut() else {
            return;
        };
        match top.r.next() {
            Some((k, Val::Obj(r))) if matches!(top.l.get(&k), Some(Val::Obj(_))) => {
                let Some(Val::Obj(l)) = top.l.get_mut(&k).map(core::mem::take) else {
                    unreachable!()
                };
                stack.push(Frame {
                    l: rc_unwrap_or_clone(l),
                    r: rc_unwrap_or_clone(r).into_iter(),
                    key: Some(k),
                });
            }
            Some((k, v)) => {
                top.l.insert(k, v);
            }
            None => {
                let Some(mut done) = stack.pop() else {
                    return;
                };
                let merged = core::mem::take(&mut done.l);
                match (stack.last_mut(), done.key.take()) {
                    (Some(parent), Some(key)) => {
                        if let Some(slot) = parent.l.get_mut(&key) {
                            *slot = Val::Obj(Rc::new(merged));
                        }
                    }
                    _ => {
                        *Rc::make_mut(l) = merged;
                        return;
                    }
                }
            }
        }
    }
}

/// jq caps a repeated string's result length at `INT_MAX` (its `jv` string length field is a
/// signed 32-bit int internally, even on a 64-bit host) and refuses to even attempt building a
/// longer one, rather than let the allocation run. A real process can often still satisfy an
/// allocation that large (jq's own oracle-verified boundary: `"a" * 1500000000` succeeds,
/// `"a" * 2147483647` — `INT_MAX` exactly — does not); the sandbox this tool actually runs in
/// cannot — attempting even the `INT_MAX`-byte case (still under jq's own cap) aborts the whole
/// component with an allocation failure, unlike jq's own recoverable error. Capped well below
/// that instead, at the same order of magnitude this tool already uses elsewhere for a
/// platform-imposed (not jq-imposed) ceiling — e.g. grep's `-f -`/stdin read limit — since jq's
/// own `INT_MAX` bound is not actually reachable here.
const MAX_REPEATED_STRING_LEN: usize = 16 * 1024 * 1024;

fn checked_repeat_len(part_len: usize, count: usize) -> Result<usize, Error> {
    part_len
        .checked_mul(count)
        .filter(|&len| len <= MAX_REPEATED_STRING_LEN)
        .ok_or_else(|| Error::str("Repeat string result too long"))
}

/// How many times jq 1.8 repeats a string multiplied by `n`: `None` (giving `null`) for a
/// negative count or NaN, else the count truncated to an integer, `0` giving `""`.
fn repeat_count(n: &Num) -> Option<usize> {
    let d = n.as_f64();
    if d < 0.0 || d.is_nan() {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(if d > f64::from(i32::MAX) {
        i32::MAX as usize
    } else {
        d as usize
    })
}

impl core::ops::Mul for Val {
    type Output = ValR;
    fn mul(self, rhs: Self) -> Self::Output {
        use Val::*;
        match (self, rhs) {
            (Num(x), Num(y)) => Ok(Num(x * y)),
            // As jq 1.8 repeats: `"ab" * 1.5` is `"ab"`, `"ab" * 0` is `""` and a negative
            // count gives `null`.
            (BStr(s), Num(n)) | (Num(n), BStr(s)) => Ok(match repeat_count(&n) {
                None => Null,
                Some(count) => {
                    checked_repeat_len(s.len(), count)?;
                    Self::byte_str(s.repeat(count))
                }
            }),
            (TStr(s), Num(n)) | (Num(n), TStr(s)) => Ok(match repeat_count(&n) {
                None => Null,
                Some(count) => {
                    checked_repeat_len(s.len(), count)?;
                    Self::utf8_str(s.repeat(count))
                }
            }),
            (Obj(mut l), Obj(r)) => {
                obj_merge(&mut l, r);
                Ok(Obj(l))
            }
            (l, r) => Err(type_error2(&l, &r, "cannot be multiplied")),
        }
    }
}

/// Split a string by a given separator string.
fn split<'a>(s: &'a [u8], sep: &'a [u8]) -> Box<dyn Iterator<Item = &'a [u8]> + 'a> {
    if s.is_empty() {
        Box::new(core::iter::empty())
    } else if sep.is_empty() {
        // Rust's `split` function with an empty separator ("")
        // yields an empty string as first and last result
        // to prevent this, we are using `chars` instead
        Box::new(s.char_indices().map(|(start, end, _)| &s[start..end]))
    } else {
        Box::new(s.split_str(sep))
    }
}

/// jq's own wording for `X / 0` or `X % 0` (of any number type, not just integer zero — a float
/// `0.0` divisor is just as refused): `number (X) and number (Y) cannot be divided[ (remainder)]
/// because the divisor is zero`. Verified against the oracle for `/` and `%`, integer and float
/// zero divisors alike.
fn zero_divisor_error(x: Num, y: Num, remainder: bool) -> Error {
    let what = if remainder { " (remainder)" } else { "" };
    Error::str(format_args!(
        "number ({x}) and number ({y}) cannot be divided{what} because the divisor is zero"
    ))
}

impl core::ops::Div for Val {
    type Output = ValR;
    fn div(self, rhs: Self) -> Self::Output {
        let fs = |x: Bytes, y: Bytes, into: BytesValFn| {
            split(&x, &y).map(|s| into(x.slice_ref(s))).collect()
        };
        match (self, rhs) {
            // jq refuses division by an exact-zero divisor (integer or float) rather than let it
            // through to the underlying IEEE result — matching jq's own behavior is the bar this
            // build follows, even where jaq's own upstream deliberately lets it through (jaq-std's
            // `nan`/`infinite` no longer rely on this; see jaq-std/src/lib.rs).
            (Self::Num(x), Self::Num(y)) if y == Num::Int(0) => {
                Err(zero_divisor_error(x, y, false))
            }
            (Self::Num(x), Self::Num(y)) => Ok(Self::Num(x / y)),
            (Self::TStr(x), Self::TStr(y)) => Ok(fs(*x, *y, Val::utf8_str)),
            (Self::BStr(x), Self::BStr(y)) => Ok(fs(*x, *y, Val::byte_str)),
            (l, r) => Err(type_error2(&l, &r, "cannot be divided")),
        }
    }
}

impl core::ops::Rem for Val {
    type Output = ValR;
    fn rem(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Self::Num(x), Self::Num(y)) if y.is_zero_divisor() && !x.as_f64().is_nan() => {
                Err(zero_divisor_error(x, y, true))
            }
            (Self::Num(x), Self::Num(y)) => Ok(Self::Num(x % y)),
            (l, r) => Err(type_error2(&l, &r, "cannot be divided (remainder)")),
        }
    }
}

impl core::ops::Neg for Val {
    type Output = ValR;
    fn neg(self) -> Self::Output {
        match self {
            Self::Num(n) => Ok(Self::Num(-n)),
            x => Err(type_error(&x, "cannot be negated")),
        }
    }
}

impl PartialOrd for Val {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// What comparing two values shallowly tells: their order, or that their elements (an array's
/// in order, an object's values in the order of its sorted keys) decide it, lexicographically.
enum Shallow<'a> {
    Decided(Ordering),
    Elements(alloc::vec::IntoIter<&'a Val>, alloc::vec::IntoIter<&'a Val>),
}

impl Val {
    fn cmp_shallow<'a>(&'a self, other: &'a Self) -> Shallow<'a> {
        use Ordering::{Equal, Greater, Less};
        Shallow::Decided(match (self, other) {
            (Self::Arr(x), Self::Arr(y)) => {
                let l: Vec<_> = x.iter().collect();
                let r: Vec<_> = y.iter().collect();
                return Shallow::Elements(l.into_iter(), r.into_iter());
            }
            (Self::Obj(x), Self::Obj(y)) => match (x.len(), y.len()) {
                (0, 0) => Equal,
                (0, _) => Less,
                (_, 0) => Greater,
                _ => {
                    let mut l: Vec<_> = x.iter().collect();
                    let mut r: Vec<_> = y.iter().collect();
                    l.sort_by_key(|(k, _v)| *k);
                    r.sort_by_key(|(k, _v)| *k);
                    let kl = l.iter().map(|(k, _v)| k);
                    let kr = r.iter().map(|(k, _v)| k);
                    match kl.cmp(kr) {
                        Equal => {
                            let vl: Vec<_> = l.iter().map(|(_k, v)| *v).collect();
                            let vr: Vec<_> = r.iter().map(|(_k, v)| *v).collect();
                            return Shallow::Elements(vl.into_iter(), vr.into_iter());
                        }
                        ord => ord,
                    }
                }
            },
            (x, y) => x.cmp_scalar(y),
        })
    }
}

impl Ord for Val {
    /// Compare values, arrays and objects element by element, without recursing once per level
    /// of nesting.
    fn cmp(&self, other: &Self) -> Ordering {
        use Ordering::{Equal, Greater, Less};
        let mut stack: Vec<(alloc::vec::IntoIter<&Val>, alloc::vec::IntoIter<&Val>)> = Vec::new();
        let mut pair = Some((self, other));
        loop {
            if let Some((x, y)) = pair.take() {
                match x.cmp_shallow(y) {
                    Shallow::Decided(Equal) => (),
                    Shallow::Decided(ord) => return ord,
                    Shallow::Elements(l, r) => stack.push((l, r)),
                }
            }
            let Some((l, r)) = stack.last_mut() else {
                return Equal;
            };
            match (l.next(), r.next()) {
                (Some(x), Some(y)) => pair = Some((x, y)),
                (None, None) => {
                    stack.pop();
                }
                (None, Some(_)) => return Less,
                (Some(_), None) => return Greater,
            }
        }
    }
}

impl Val {
    /// Compare values that are not both arrays or both objects.
    fn cmp_scalar(&self, other: &Self) -> Ordering {
        use Ordering::{Equal, Greater, Less};
        match (self, other) {
            (Self::Null, Self::Null) => Equal,
            (Self::Bool(x), Self::Bool(y)) => x.cmp(y),
            (Self::Num(x), Self::Num(y)) => x.cmp(y),
            (Self::BStr(x) | Self::TStr(x), Self::BStr(y) | Self::TStr(y)) => x.cmp(y),
            (Self::Arr(_), Self::Arr(_)) | (Self::Obj(_), Self::Obj(_)) => Equal,

            // nulls are smaller than anything else
            (Self::Null, _) => Less,
            (_, Self::Null) => Greater,
            // bools are smaller than anything else, except for nulls
            (Self::Bool(_), _) => Less,
            (_, Self::Bool(_)) => Greater,
            // numbers are smaller than anything else, except for nulls and bools
            (Self::Num(_), _) => Less,
            (_, Self::Num(_)) => Greater,
            // etc.
            (Self::BStr(_) | Self::TStr(_), _) => Less,
            (_, Self::BStr(_) | Self::TStr(_)) => Greater,
            (Self::Arr(_), _) => Less,
            (_, Self::Arr(_)) => Greater,
        }
    }
}

impl PartialEq for Val {
    /// Compare values for equality, arrays and objects element by element, without recursing
    /// once per level of nesting.
    fn eq(&self, other: &Self) -> bool {
        let mut pairs = Vec::from([(self, other)]);
        while let Some(pair) = pairs.pop() {
            match pair {
                (Self::Null, Self::Null) => (),
                (Self::Bool(x), Self::Bool(y)) if x == y => (),
                (Self::Num(x), Self::Num(y)) if x == y => (),
                (Self::BStr(x) | Self::TStr(x), Self::BStr(y) | Self::TStr(y)) if x == y => (),
                (Self::Arr(x), Self::Arr(y)) if Rc::ptr_eq(x, y) => (),
                (Self::Arr(x), Self::Arr(y)) if x.len() == y.len() => {
                    pairs.extend(x.iter().zip(y.iter()));
                }
                (Self::Obj(x), Self::Obj(y)) if Rc::ptr_eq(x, y) => (),
                (Self::Obj(x), Self::Obj(y)) if x.len() == y.len() => {
                    for (k, v) in x.iter() {
                        match y.get(k) {
                            Some(w) => pairs.push((v, w)),
                            None => return false,
                        }
                    }
                }
                _ => return false,
            }
        }
        true
    }
}

impl Eq for Val {}

impl Hash for Val {
    fn hash<H: Hasher>(&self, state: &mut H) {
        fn hash_with(u: u8, x: impl Hash, state: &mut impl Hasher) {
            state.write_u8(u);
            x.hash(state)
        }
        match self {
            Self::Num(n) => n.hash(state),
            // Num::hash() starts its hash with a 0 or 1, so we start with 2 here
            Self::Null => state.write_u8(2),
            Self::Bool(b) => state.write_u8(if *b { 3 } else { 4 }),
            Self::BStr(b) | Self::TStr(b) => hash_with(5, b, state),
            Self::Arr(a) => hash_with(6, a, state),
            Self::Obj(o) => {
                state.write_u8(7);
                // this is similar to what happens in `Val::cmp`
                let mut kvs: Vec<_> = o.iter().collect();
                kvs.sort_by_key(|(k, _v)| *k);
                kvs.iter().for_each(|(k, v)| (k, v).hash(state));
            }
        }
    }
}

/// Display bytes as valid UTF-8 string.
///
/// This maps invalid UTF-8 to the Unicode replacement character.
pub fn bstr(s: &(impl core::convert::AsRef<[u8]> + ?Sized)) -> impl fmt::Display + '_ {
    BStr::new(s)
}

impl fmt::Display for Val {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write::format(f, &write::Pp::default(), 0, self)
    }
}
