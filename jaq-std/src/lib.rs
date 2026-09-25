//! Standard library for the jq language.
//!
//! The standard library provides a set of filters.
//! These filters are either implemented as definitions or as functions.
//! For example, the standard library provides the `map(f)` filter,
//! which is defined using the more elementary filter `[.[] | f]`.
//!
//! If you want to use the standard library in jaq, then
//! you'll likely only need [`funs`] and [`defs`].
//! Most other functions are relevant if you
//! want to implement your own native filters.
#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod input;
#[cfg(feature = "math")]
mod math;
#[cfg(feature = "regex")]
mod regex;
#[cfg(feature = "time")]
mod time;

#[cfg(feature = "std")]
use alloc::string::String;
#[cfg(feature = "format")]
use alloc::string::ToString;
use alloc::{boxed::Box, vec::Vec};
#[cfg(feature = "log")]
use bstr::BStr;
use bstr::ByteSlice;
use jaq_core::box_iter::{box_once, BoxIter};
use jaq_core::native::{bome, run, unary, v, Filter, Fun};
#[cfg(feature = "regex")]
use jaq_core::Cv;
#[cfg(any(feature = "regex", feature = "std"))]
use jaq_core::ValT as _;
use jaq_core::{load, Bind, DataT, Error, Exn, RunPtr, ValR, ValX, ValXs};

/// Definitions of the standard library.
pub fn defs() -> impl Iterator<Item = load::parse::Def<&'static str>> {
    load::parse(include_str!("defs.jq"), |p| p.defs())
        .unwrap()
        .into_iter()
}

/// Named filters available by default in jaq
/// which are implemented as native filters, such as `length`, `keys`, ...,
/// but also `now`, `debug`, `fromdateiso8601`, ...
///
/// This is the combination of [`base_funs`] and [`extra_funs`].
/// It does not include filters implemented by definition, such as `map`.
#[cfg(all(
    feature = "std",
    feature = "format",
    feature = "log",
    feature = "math",
    feature = "regex",
    feature = "time",
))]
pub fn funs<D: DataT>() -> impl Iterator<Item = Fun<D>>
where
    for<'a> D::V<'a>: ValT,
{
    base_funs().chain(extra_funs())
}

/// Minimal set of filters that are generic over the value type.
/// Return the minimal set of named filters available in jaq
/// which are implemented as native filters, such as `length`, `keys`, ...,
/// but not `now`, `debug`, `fromdateiso8601`, ...
///
/// Does not return filters from the standard library, such as `map`.
pub fn base_funs<D: DataT>() -> impl Iterator<Item = Fun<D>>
where
    for<'a> D::V<'a>: ValT,
{
    base_run().into_vec().into_iter().map(run)
}

/// Supplementary set of filters that are generic over the value type.
#[cfg(all(
    feature = "std",
    feature = "format",
    feature = "log",
    feature = "math",
    feature = "regex",
    feature = "time",
))]
pub fn extra_funs<D: DataT>() -> impl Iterator<Item = Fun<D>>
where
    for<'a> D::V<'a>: ValT,
{
    [std(), format(), math(), regex(), time(), log()]
        .into_iter()
        .flat_map(|fs| fs.into_vec().into_iter().map(run))
}

/// Values that the standard library can operate on.
pub trait ValT: jaq_core::ValT + Ord + From<f64> + From<usize> {
    /// Convert an array into a sequence.
    ///
    /// This returns the original value as `Err` if it is not an array.
    fn into_seq<S: FromIterator<Self>>(self) -> Result<S, Self>;

    /// True if the value is integer.
    fn is_int(&self) -> bool;

    /// Use the value as machine-sized integer.
    ///
    /// If this function returns `Some(_)`, then [`Self::is_int`] must return true.
    /// However, the other direction must not necessarily be the case, because
    /// there may be integer values that are not representable by `isize`.
    fn as_isize(&self) -> Option<isize>;

    /// Use the value as floating-point number.
    ///
    /// This succeeds for all numeric values,
    /// rounding too large/small ones to +/- Infinity.
    fn as_f64(&self) -> Option<f64>;

    /// True if the value is interpreted as UTF-8 string.
    fn is_utf8_str(&self) -> bool;

    /// If the value is a string (whatever its interpretation), return its bytes.
    fn as_bytes(&self) -> Option<&[u8]>;

    /// If the value is interpreted as UTF-8 string, return its bytes.
    fn as_utf8_bytes(&self) -> Option<&[u8]> {
        self.is_utf8_str().then(|| self.as_bytes()).flatten()
    }

    /// If the value is a string (whatever its interpretation), return its bytes, else fail.
    fn try_as_bytes(&self) -> Result<&[u8], Error<Self>> {
        self.as_bytes().ok_or_else(|| self.fail_str())
    }

    /// If the value is interpreted as UTF-8 string, return its bytes, else fail.
    fn try_as_utf8_bytes(&self) -> Result<&[u8], Error<Self>> {
        self.as_utf8_bytes().ok_or_else(|| self.fail_str())
    }

    /// If the value is a string and `sub` points to a slice of the string,
    /// shorten the string to `sub`, else panic.
    fn as_sub_str(&self, sub: &[u8]) -> Self;

    /// Interpret bytes as UTF-8 string value.
    fn from_utf8_bytes(b: impl AsRef<[u8]> + Send + 'static) -> Self;
}

/// Convenience trait for implementing the core functions.
trait ValTx: ValT + Sized {
    fn into_vec(self) -> Result<Vec<Self>, Error<Self>> {
        self.into_seq().map_err(|v| Error::typ(v, "array"))
    }

    fn try_as_isize(&self) -> Result<isize, Error<Self>> {
        self.as_isize()
            .ok_or_else(|| Error::typ(self.clone(), "integer"))
    }

    fn try_as_i32(&self) -> Result<i32, Error<Self>> {
        self.try_as_isize()?.try_into().map_err(Error::str)
    }

    /// The value as a number; jq's math functions refuse anything else as below.
    fn try_as_f64(&self) -> Result<f64, Error<Self>> {
        self.as_f64()
            .ok_or_else(|| self.type_error("number required"))
    }

    /// jq's `type_error2`: `KIND (VALUE) and KIND (VALUE) MESSAGE`.
    fn type_error2(&self, other: &Self, message: &str) -> Error<Self> {
        Error::str(format_args!(
            "{} ({}) and {} ({}) {message}",
            self.kind_name(),
            self.dump_trunc(),
            other.kind_name(),
            other.dump_trunc()
        ))
    }

    /// The string bytes of this value, or the error `message`.
    fn str_or(&self, message: &str) -> Result<&[u8], Error<Self>> {
        self.as_utf8_bytes().ok_or_else(|| Error::str(message))
    }

    /// The elements of an array to sort or pick from by `f`, as jq gets them with `map([f])`:
    /// anything but an array or object cannot be iterated over, and an object's mapped values
    /// are no array to go with it (`MESSAGE`).
    fn by_elements<'a>(
        self,
        f: &impl Fn(Self) -> ValXs<'a, Self>,
        message: &str,
    ) -> Result<Vec<Self>, Exn<'a, Self>> {
        match self.into_seq() {
            Ok(xs) => Ok(xs),
            Err(v) if v.kind_name() == "object" => {
                let keys = v
                    .clone()
                    .values()
                    .map(|x| f(x?).collect::<Result<Self, _>>());
                let keys = keys.collect::<Result<Self, _>>()?;
                Err(Exn::from(v.type_error2(&keys, message)))
            }
            Err(v) => Err(Exn::from(v.iterate_error())),
        }
    }

    /// Apply a function to an array.
    fn mutate_arr(self, f: impl FnOnce(&mut Vec<Self>)) -> ValR<Self> {
        let mut a = self.into_vec()?;
        f(&mut a);
        Ok(Self::from_iter(a))
    }

    /// Round as jq does, on doubles: the result is an integer while a double holds it exactly
    /// (2^53) and it fits `isize`; past that it stays a double (`1e30 | floor` is `1e+30`), as
    /// does a negative zero (`-0.4 | round` is `-0`).
    fn round(self, f: impl FnOnce(f64) -> f64) -> ValR<Self> {
        const EXACT: f64 = 9007199254740992.0;
        let f = f(self.try_as_f64()?);
        let int = f.abs() <= EXACT
            && isize::MIN as f64 <= f
            && f <= isize::MAX as f64
            && !(f == 0.0 && f.is_sign_negative());
        Ok(if int {
            Self::from(f as isize)
        } else {
            Self::from(f)
        })
    }

    /// If the value is interpreted as UTF-8 string,
    /// return its `str` representation.
    #[cfg(any(feature = "regex", feature = "time"))]
    fn try_as_str(&self) -> Result<&str, Error<Self>> {
        self.try_as_utf8_bytes()
            .and_then(|s| core::str::from_utf8(s).map_err(Error::str))
    }

    fn map_utf8_str<B>(self, f: impl FnOnce(&[u8]) -> B) -> ValR<Self>
    where
        B: AsRef<[u8]> + Send + 'static,
    {
        Ok(Self::from_utf8_bytes(f(self.try_as_utf8_bytes()?)))
    }

    /// Helper function to strip away the prefix or suffix of a string.
    fn strip_fix<F>(self, fix: &Self, f: F) -> Result<Self, Error<Self>>
    where
        F: for<'a> FnOnce(&'a [u8], &[u8]) -> Option<&'a [u8]>,
    {
        Ok(match f(self.try_as_bytes()?, fix.try_as_bytes()?) {
            Some(sub) => self.as_sub_str(sub),
            None => self,
        })
    }

    fn fail_str(&self) -> Error<Self> {
        Error::typ(self.clone(), "string")
    }
}
impl<T: ValT> ValTx for T {}

/// Sort array by the given function.
fn sort_by<'a, V: ValT>(xs: &mut [V], f: impl Fn(V) -> ValXs<'a, V>) -> Result<(), Exn<'a, V>> {
    // Some(e) iff an error has previously occurred
    let mut err = None;
    xs.sort_by_cached_key(|x| {
        if err.is_some() {
            return Vec::new();
        };
        match f(x.clone()).collect() {
            Ok(y) => y,
            Err(e) => {
                err = Some(e);
                Vec::new()
            }
        }
    });
    err.map_or(Ok(()), Err)
}

/// Group an array by the given function.
fn group_by<'a, V: ValT>(xs: Vec<V>, f: impl Fn(V) -> ValXs<'a, V>) -> ValX<'a, V> {
    let mut yx: Vec<(Vec<V>, V)> = xs
        .into_iter()
        .map(|x| Ok((f(x.clone()).collect::<Result<_, _>>()?, x)))
        .collect::<Result<_, Exn<_>>>()?;

    yx.sort_by(|(y1, _), (y2, _)| y1.cmp(y2));

    let mut grouped = Vec::new();
    let mut yx = yx.into_iter();
    if let Some((mut group_y, first_x)) = yx.next() {
        let mut group = Vec::from([first_x]);
        for (y, x) in yx {
            if group_y != y {
                grouped.push(V::from_iter(core::mem::take(&mut group)));
                group_y = y;
            }
            group.push(x);
        }
        if !group.is_empty() {
            grouped.push(V::from_iter(group));
        }
    }

    Ok(V::from_iter(grouped))
}

/// Get the minimum or maximum element from an array according to the given function.
fn cmp_by<'a, V: Clone, F, R>(xs: Vec<V>, f: F, replace: R) -> Result<Option<V>, Exn<'a, V>>
where
    F: Fn(V) -> ValXs<'a, V>,
    R: Fn(&[V], &[V]) -> bool,
{
    let iter = xs.into_iter();
    let mut iter = iter.map(|x| (x.clone(), f(x).collect::<Result<Vec<_>, _>>()));
    let (mut mx, mut my) = if let Some((x, y)) = iter.next() {
        (x, y?)
    } else {
        return Ok(None);
    };
    for (x, y) in iter {
        let y = y?;
        if replace(&my, &y) {
            (mx, my) = (x, y);
        }
    }
    Ok(Some(mx))
}

/// Convert a string into an array of its Unicode codepoints (with negative integers representing UTF-8 errors).
fn explode<V: ValT>(s: &[u8]) -> impl Iterator<Item = ValR<V>> + '_ {
    let invalid = [].iter();
    Explode { s, invalid }.map(|r| match r {
        Err(b) => Ok((-(b as isize)).into()),
        // conversion from u32 to isize may fail on 32-bit systems for high values of c
        Ok(c) => Ok(isize::try_from(c as u32).map_err(Error::str)?.into()),
    })
}

struct Explode<'a> {
    s: &'a [u8],
    invalid: core::slice::Iter<'a, u8>,
}
impl Iterator for Explode<'_> {
    type Item = Result<char, u8>;
    fn next(&mut self) -> Option<Self::Item> {
        self.invalid.next().map(|next| Err(*next)).or_else(|| {
            let (c, size) = bstr::decode_utf8(self.s);
            let (consumed, rest) = self.s.split_at(size);
            self.s = rest;
            c.map(Ok).or_else(|| {
                // invalid UTF-8 sequence, emit all invalid bytes
                self.invalid = consumed.iter();
                self.invalid.next().map(|next| Err(*next))
            })
        })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let max = self.s.len();
        let min = self.s.len() / 4;
        let inv = self.invalid.as_slice().len();
        (min + inv, Some(max + inv))
    }
}

/// Convert an array of Unicode codepoints into a string, as jq 1.8 does: each number is
/// truncated to an integer (C's `(int)`), and one that is not a Unicode scalar value becomes
/// U+FFFD.
fn implode<V: ValT>(v: V) -> Result<Vec<u8>, Error<V>> {
    let xs: Vec<V> = v
        .into_seq()
        .map_err(|_| Error::str("implode input must be an array"))?;
    let mut s = String::with_capacity(xs.len());
    for x in xs {
        let message = "can't be imploded, unicode codepoint needs to be numeric";
        let f = x.as_f64().filter(|f| !f.is_nan());
        let f = f.ok_or_else(|| x.type_error(message))?;
        // C leaves a conversion out of `int`'s range undefined; x86-64 yields INT_MIN and
        // AArch64 saturates, and jq replaces either.
        #[allow(clippy::cast_possible_truncation)]
        let i = if (-2147483648.0..2147483648.0).contains(&f) {
            f as i64
        } else {
            -1
        };
        let c = u32::try_from(i).ok().and_then(char::from_u32);
        s.push(c.unwrap_or('\u{FFFD}'));
    }
    Ok(s.into_bytes())
}

/// jq's `trim`, `ltrim` and `rtrim`.
fn trim_with<V: ValT>(v: &V, f: impl FnOnce(&[u8]) -> &[u8]) -> ValR<V> {
    let s = v.str_or("trim input must be a string")?;
    Ok(v.as_sub_str(f(s)))
}

/// jq's `min` and `max` (`minmax_by` with each element its own key), without the `null` that
/// an empty array gives (see `defs.jq`).
fn min_max<'a, V: ValT + 'a>(v: V, replace: fn(&[V], &[V]) -> bool) -> ValXs<'a, V> {
    let xs: Vec<V> = match v.into_seq() {
        Ok(xs) => xs,
        Err(v) => return box_once(Err(Exn::from(v.type_error2(&v, "cannot be iterated over")))),
    };
    once_or_empty(cmp_by(xs, |x| box_once(Ok(x)), replace))
}

fn once_or_empty<'a, T: 'a, E: 'a>(r: Result<Option<T>, E>) -> BoxIter<'a, Result<T, E>> {
    Box::new(r.transpose().into_iter())
}

// Primitive float rounding methods are unavailable without `std`.
// These can be dropped after `core_float_math` lands: https://github.com/rust-lang/rust/issues/137578
fn floor(x: f64) -> f64 {
    #[cfg(feature = "std")]
    return x.floor();
    #[cfg(not(feature = "std"))]
    return no_std_float::floor(x);
}

fn round(x: f64) -> f64 {
    #[cfg(feature = "std")]
    return x.round();
    #[cfg(not(feature = "std"))]
    return no_std_float::round(x);
}

fn ceil(x: f64) -> f64 {
    #[cfg(feature = "std")]
    return x.ceil();
    #[cfg(not(feature = "std"))]
    return no_std_float::ceil(x);
}

#[cfg(any(not(feature = "std"), test))]
mod no_std_float {
    const SIGN_MASK: u64 = 1 << 63;
    const SIG_BITS: i32 = 52;
    const SIG_MASK: u64 = (1 << SIG_BITS) - 1;
    const EXPONENT_BIAS: i32 = 1023;

    // Adapted from Rust's MIT-licensed `libm` implementation:
    // https://github.com/rust-lang/compiler-builtins/blob/5c5f07851b1878013ac81e129b0517feaaf8661d/libm/src/math/generic/trunc.rs
    fn trunc(x: f64) -> f64 {
        let xi = x.to_bits();
        let e = ((xi >> SIG_BITS) & 0x7ff) as i32 - EXPONENT_BIAS;
        if e >= SIG_BITS {
            return x;
        }
        let clear_mask = if e < 0 { !SIGN_MASK } else { SIG_MASK >> e };
        let cleared = xi & clear_mask;
        f64::from_bits(xi ^ cleared)
    }

    pub fn floor(x: f64) -> f64 {
        let trunc = trunc(x);
        if x < trunc {
            trunc - 1.0
        } else {
            trunc
        }
    }

    pub fn round(x: f64) -> f64 {
        let trunc = trunc(x);
        let fract = x - trunc;
        if fract >= 0.5 {
            trunc + 1.0
        } else if fract <= -0.5 {
            trunc - 1.0
        } else {
            trunc
        }
    }

    pub fn ceil(x: f64) -> f64 {
        let trunc = trunc(x);
        if x > trunc {
            trunc + 1.0
        } else {
            trunc
        }
    }

    #[cfg(all(test, feature = "std"))]
    mod tests {
        #[track_caller]
        fn assert_same(actual: f64, expected: f64) {
            if expected.is_nan() {
                assert!(actual.is_nan());
            } else {
                assert_eq!(actual.to_bits(), expected.to_bits());
            }
        }

        #[test]
        fn matches_std() {
            let values = [
                f64::NEG_INFINITY,
                -((1_u64 << 53) as f64),
                -1.5,
                -0.5,
                -f64::from_bits(1),
                -0.0,
                0.0,
                f64::from_bits(1),
                0.5,
                1.5,
                (1_u64 << 53) as f64,
                f64::INFINITY,
                f64::NAN,
            ];
            for x in values {
                assert_same(super::trunc(x), x.trunc());
                assert_same(super::floor(x), x.floor());
                assert_same(super::round(x), x.round());
                assert_same(super::ceil(x), x.ceil());
            }
        }
    }
}

#[allow(clippy::unit_arg)]
fn base_run<D: DataT>() -> Box<[Filter<RunPtr<D>>]>
where
    for<'a> D::V<'a>: ValT,
{
    let f = || [Bind::Fun(())].into();
    Box::new([
        ("floor", v(0), |cv| bome(cv.1.round(floor))),
        ("round", v(0), |cv| bome(cv.1.round(round))),
        ("ceil", v(0), |cv| bome(cv.1.round(ceil))),
        ("utf8bytelength", v(0), |cv| {
            let message = "only strings have UTF-8 byte length";
            let len = cv.1.as_utf8_bytes().ok_or_else(|| cv.1.type_error(message));
            bome(len.map(|s| (s.len() as isize).into()))
        }),
        ("explode", v(0), |cv| {
            let s = cv.1.str_or("explode input must be a string");
            bome(s.and_then(|s| explode(s).collect()))
        }),
        ("implode", v(0), |cv| {
            bome(implode(cv.1).map(D::V::from_utf8_bytes))
        }),
        ("ascii_downcase", v(0), |cv| {
            let s = cv.1.str_or("explode input must be a string");
            bome(s.map(|s| D::V::from_utf8_bytes(s.to_ascii_lowercase())))
        }),
        ("ascii_upcase", v(0), |cv| {
            let s = cv.1.str_or("explode input must be a string");
            bome(s.map(|s| D::V::from_utf8_bytes(s.to_ascii_uppercase())))
        }),
        ("reverse", v(0), |cv| bome(cv.1.mutate_arr(|a| a.reverse()))),
        ("sort", v(0), |cv| {
            bome(match cv.1.into_seq::<Vec<_>>() {
                Ok(mut a) => {
                    a.sort();
                    Ok(D::V::from_iter(a))
                }
                Err(v) => Err(v.type_error("cannot be sorted, as it is not an array")),
            })
        }),
        ("sort_by", f(), |mut cv| {
            let (f, fc) = cv.0.pop_fun();
            let f = move |v| f.run((fc.clone(), v));
            let message = "cannot be sorted, as they are not both arrays";
            box_once(cv.1.by_elements(&f, message).and_then(|mut a| {
                sort_by(&mut a, f)?;
                Ok(D::V::from_iter(a))
            }))
        }),
        ("group_by", f(), |mut cv| {
            let (f, fc) = cv.0.pop_fun();
            let f = move |v| f.run((fc.clone(), v));
            let message = "cannot be sorted, as they are not both arrays";
            box_once(cv.1.by_elements(&f, message).and_then(|a| group_by(a, f)))
        }),
        ("min_by_or_empty", f(), |mut cv| {
            let (f, fc) = cv.0.pop_fun();
            let f = move |v| f.run((fc.clone(), v));
            let a = cv.1.by_elements(&f, "cannot be iterated over");
            once_or_empty(a.and_then(|a| cmp_by(a, f, |my, y| y < my)))
        }),
        ("max_by_or_empty", f(), |mut cv| {
            let (f, fc) = cv.0.pop_fun();
            let f = move |v| f.run((fc.clone(), v));
            let a = cv.1.by_elements(&f, "cannot be iterated over");
            once_or_empty(a.and_then(|a| cmp_by(a, f, |my, y| y >= my)))
        }),
        // jq's C `min` and `max`.
        ("min_or_empty", v(0), |cv| min_max(cv.1, |my, y| y < my)),
        ("max_or_empty", v(0), |cv| min_max(cv.1, |my, y| y >= my)),
        ("startswith", v(1), |cv| {
            unary(cv, |v, s| match (v.as_utf8_bytes(), s.as_utf8_bytes()) {
                (Some(v), Some(s)) => Ok(v.starts_with(s).into()),
                _ => Err(Error::str("startswith() requires string inputs")),
            })
        }),
        ("endswith", v(1), |cv| {
            unary(cv, |v, s| match (v.as_utf8_bytes(), s.as_utf8_bytes()) {
                (Some(v), Some(s)) => Ok(v.ends_with(s).into()),
                _ => Err(Error::str("endswith() requires string inputs")),
            })
        }),
        // jq 1.8 trims through `startswith`/`endswith`, so only strings trim.
        ("ltrimstr", v(1), |cv| {
            unary(cv, |v, pre| {
                if v.as_utf8_bytes().is_none() || pre.as_utf8_bytes().is_none() {
                    return Err(Error::str("startswith() requires string inputs"));
                }
                v.strip_fix(&pre, <[u8]>::strip_prefix)
            })
        }),
        ("rtrimstr", v(1), |cv| {
            unary(cv, |v, suf| {
                if v.as_utf8_bytes().is_none() || suf.as_utf8_bytes().is_none() {
                    return Err(Error::str("endswith() requires string inputs"));
                }
                v.strip_fix(&suf, <[u8]>::strip_suffix)
            })
        }),
        ("trim", v(0), |cv| bome(trim_with(&cv.1, ByteSlice::trim))),
        ("ltrim", v(0), |cv| {
            bome(trim_with(&cv.1, ByteSlice::trim_start))
        }),
        ("rtrim", v(0), |cv| {
            bome(trim_with(&cv.1, ByteSlice::trim_end))
        }),
        ("escape_sh", v(0), |cv| {
            let message = "can not be escaped for shell";
            let s = cv.1.as_utf8_bytes().ok_or_else(|| cv.1.type_error(message));
            bome(s.map(|s| ValT::from_utf8_bytes(s.replace(b"'", b"'\\''"))))
        }),
        ("halt", v(1), |mut cv| {
            let exit_code = cv.0.pop_var().try_as_i32().map_err(Exn::from);
            box_once(exit_code.and_then(|exit_code| Err(Exn::halt(exit_code))))
        }),
        // `nan`/`infinite` used to be defined in defs.jq as `0/0`/`1/0`, relying on `/` letting
        // an exact-zero divisor through to the underlying IEEE result. Now that `/` (and `%`)
        // raise jq's own "divisor is zero" error instead (matching real jq, see jaq-json's `Val`
        // `Div`/`Rem` impls), that construction no longer works — these are native constants
        // instead, exactly as jq's own `nan`/`infinite` ultimately bottom out at literal IEEE
        // values (jq's builtin.jq reaches them via `1e1000`, which is itself just a spelling of
        // `f64::INFINITY`). `isnan`/`isinfinite`/`isfinite`/`isnormal` and everything built on
        // them keep working unchanged, since only how `nan`/`infinite` are *produced* changed.
        ("nan", v(0), |_| bome(Ok(D::V::from(f64::NAN)))),
        ("infinite", v(0), |_| bome(Ok(D::V::from(f64::INFINITY)))),
    ])
}

#[cfg(feature = "std")]
fn now<V: From<String>>() -> Result<f64, Error<V>> {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|x| x.as_secs_f64())
        .map_err(Error::str)
}

#[cfg(feature = "std")]
fn std<D: DataT>() -> Box<[Filter<RunPtr<D>>]>
where
    for<'a> D::V<'a>: ValT,
{
    use std::env::vars;
    Box::new([
        ("env", v(0), |_| {
            bome(D::V::from_map(
                vars().map(|(k, v)| (D::V::from(k), D::V::from(v))),
            ))
        }),
        ("now", v(0), |_| bome(now().map(D::V::from))),
    ])
}

#[cfg(feature = "format")]
fn replace(s: &[u8], patterns: &[&str], replacements: &[&str]) -> Vec<u8> {
    let ac = aho_corasick::AhoCorasick::new(patterns).unwrap();
    ac.replace_all_bytes(s, replacements)
}

#[cfg(feature = "format")]
fn format<D: DataT>() -> Box<[Filter<RunPtr<D>>]>
where
    for<'a> D::V<'a>: ValT,
{
    const HTML_PATS: [&str; 5] = ["<", ">", "&", "\'", "\""];
    const HTML_REPS: [&str; 5] = ["&lt;", "&gt;", "&amp;", "&apos;", "&quot;"];
    Box::new([
        ("escape_html", v(0), |cv| {
            bome(cv.1.map_utf8_str(|s| replace(s, &HTML_PATS, &HTML_REPS)))
        }),
        ("unescape_html", v(0), |cv| {
            bome(cv.1.map_utf8_str(|s| replace(s, &HTML_REPS, &HTML_PATS)))
        }),
        ("encode_uri", v(0), |cv| {
            bome(cv.1.map_utf8_str(|s| urlencoding::encode_binary(s).to_string()))
        }),
        ("decode_uri", v(0), |cv| {
            bome(cv.1.map_utf8_str(|s| urlencoding::decode_binary(s).to_vec()))
        }),
        ("encode_base64", v(0), |cv| {
            use base64::{engine::general_purpose::STANDARD, Engine};
            bome(cv.1.map_utf8_str(|s| STANDARD.encode(s)))
        }),
        ("decode_base64", v(0), |cv| bome(decode_base64(&cv.1))),
    ])
}

/// jq's `@base64d`: decoding stops at the first `=`, missing padding is fine, one byte left
/// over is an error, and the bytes become text as jq makes it (U+FFFD for invalid UTF-8).
#[cfg(feature = "format")]
fn decode_base64<V: ValT>(v: &V) -> ValR<V> {
    let s = v.try_as_utf8_bytes()?;
    let digit = |b: u8| match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let mut out = Vec::with_capacity(s.len() / 4 * 3 + 2);
    let (mut code, mut read) = (0u32, 0);
    for &b in s.iter().take_while(|b| **b != b'=') {
        let d = digit(b).ok_or_else(|| v.type_error("is not valid base64 data"))?;
        code = (code << 6) | u32::from(d);
        read += 1;
        if read == 4 {
            out.extend(code.to_be_bytes()[1..].iter().copied());
            (code, read) = (0, 0);
        }
    }
    match read {
        3 => out.extend((code >> 2).to_be_bytes()[2..].iter().copied()),
        2 => out.push(((code >> 4) & 0xFF) as u8),
        1 => return Err(v.type_error("trailing base64 byte found")),
        _ => (),
    }
    Ok(V::from_utf8_bytes(jaq_core::jq_utf8::lossy(out)))
}

#[cfg(feature = "math")]
fn math<D: DataT>() -> Box<[Filter<RunPtr<D>>]>
where
    for<'a> D::V<'a>: ValT,
{
    let rename = |name, (_name, arity, f): Filter<RunPtr<D>>| (name, arity, f);
    Box::new([
        math::f_f!(acos),
        math::f_f!(acosh),
        math::f_f!(asin),
        math::f_f!(asinh),
        math::f_f!(atan),
        math::f_f!(atanh),
        math::f_f!(cbrt),
        math::f_f!(cos),
        math::f_f!(cosh),
        math::f_f!(erf),
        math::f_f!(erfc),
        math::f_f!(exp),
        math::f_f!(exp10),
        math::f_f!(exp2),
        math::f_f!(expm1),
        math::f_f!(fabs),
        math::f_fi!(frexp),
        math::f_fi!(lgamma_r),
        math::f_i!(ilogb),
        math::f_f!(j0),
        math::f_f!(j1),
        math::f_f!(lgamma),
        math::f_f!(log),
        math::f_f!(log10),
        math::f_f!(log1p),
        math::f_f!(log2),
        // logb is implemented in jaq-std
        math::f_ff!(modf),
        rename("nearbyint", math::f_f!(round)),
        // pow10 is implemented in jaq-std
        math::f_f!(rint),
        // significand is implemented in jaq-std
        math::f_f!(sin),
        math::f_f!(sinh),
        math::f_f!(sqrt),
        math::f_f!(tan),
        math::f_f!(tanh),
        math::f_f!(tgamma),
        math::f_f!(trunc),
        math::f_f!(y0),
        math::f_f!(y1),
        math::ff_f!(atan2),
        math::ff_f!(copysign),
        // drem is implemented in jaq-std
        math::ff_f!(fdim),
        math::ff_f!(fmax),
        math::ff_f!(fmin),
        math::ff_f!(fmod),
        math::ff_f!(hypot),
        math::if_f!(jn),
        math::fi_f!(ldexp),
        math::ff_f!(nextafter),
        // nexttoward is implemented in jaq-std
        math::ff_f!(pow),
        math::ff_f!(remainder),
        // scalb is implemented in jaq-std
        rename("scalbln", math::fi_f!(scalbn)),
        math::if_f!(yn),
        math::fff_f!(fma),
    ])
}

#[cfg(feature = "regex")]
fn re<'a, D: DataT>(s: bool, m: bool, mut cv: Cv<'a, D>) -> ValR<D::V<'a>>
where
    D::V<'a>: ValT,
{
    let flags = cv.0.pop_var();
    let re = cv.0.pop_var();

    use crate::regex::Part::{Matches, Mismatch};
    let fail_re = |e| Error::str(format_args!("invalid regex: {e}"));

    // jq's `_match_impl`: the input first, then the regex, then the flags (null for none).
    let message = "cannot be matched, as it is not a string";
    let input =
        cv.1.as_utf8_bytes()
            .ok_or_else(|| cv.1.type_error(message))?;
    let re = match re.as_utf8_bytes().map(core::str::from_utf8) {
        Some(Ok(re)) => re,
        _ => return Err(re.type_error("is not a string")),
    };
    let flags = match flags.as_utf8_bytes().map(core::str::from_utf8) {
        Some(Ok(flags)) => flags,
        None if flags.kind_name() == "null" => "",
        _ => return Err(flags.type_error("is not a string")),
    };
    let fail_flag = |_| Error::str(format_args!("{flags} is not a valid modifier string"));
    let flags = regex::Flags::new(flags).map_err(fail_flag)?;
    let re = flags.regex(re).map_err(fail_re)?;
    let out = regex::regex(input, &re, flags, (s, m));
    let sub = |s| cv.1.as_sub_str(s);
    let out = out.into_iter().map(|out| match out {
        Matches(ms) => ms
            .into_iter()
            .map(|m| D::V::from_map(m.fields(sub)))
            .collect(),
        Mismatch(s) => Ok(sub(s)),
    });
    out.collect()
}

#[cfg(feature = "regex")]
fn regex<D: DataT>() -> Box<[Filter<RunPtr<D>>]>
where
    for<'a> D::V<'a>: ValT,
{
    let vv = || [Bind::Var(()), Bind::Var(())].into();
    Box::new([
        ("matches", vv(), |cv| bome(re(false, true, cv))),
        ("split_matches", vv(), |cv| bome(re(true, true, cv))),
        ("split_", vv(), |cv| bome(re(true, false, cv))),
    ])
}

#[cfg(feature = "time")]
fn time<D: DataT>() -> Box<[Filter<RunPtr<D>>]>
where
    for<'a> D::V<'a>: ValT,
{
    use jiff::tz::TimeZone;
    Box::new([
        ("fromdateiso8601", v(0), |cv| {
            bome(cv.1.try_as_str().and_then(time::from_iso8601))
        }),
        ("todateiso8601", v(0), |cv| {
            bome(time::to_iso8601(&cv.1).map(D::V::from))
        }),
        ("strftime", v(1), |cv| {
            unary(cv, |v, fmt| {
                time::strftime(&v, &fmt, TimeZone::UTC, "strftime")
            })
        }),
        ("strflocaltime", v(1), |cv| {
            unary(cv, |v, fmt| {
                time::strftime(&v, &fmt, TimeZone::system(), "strflocaltime")
            })
        }),
        ("gmtime", v(0), |cv| {
            if cv.1.kind_name() != "number" {
                return bome(Err(Error::str("gmtime() requires numeric inputs")));
            }
            bome(time::gmtime(&cv.1, TimeZone::UTC))
        }),
        ("localtime", v(0), |cv| {
            if cv.1.kind_name() != "number" {
                return bome(Err(Error::str("localtime() requires numeric inputs")));
            }
            bome(time::gmtime(&cv.1, TimeZone::system()))
        }),
        ("strptime", v(1), |cv| {
            unary(cv, |v, fmt| {
                match (v.as_utf8_bytes(), fmt.as_utf8_bytes()) {
                    (Some(s), Some(f)) => {
                        match (core::str::from_utf8(s), core::str::from_utf8(f)) {
                            (Ok(s), Ok(f)) => time::strptime(s, f),
                            _ => Err(Error::str(
                                "strptime/1 requires string inputs and arguments",
                            )),
                        }
                    }
                    _ => Err(Error::str(
                        "strptime/1 requires string inputs and arguments",
                    )),
                }
            })
        }),
        ("mktime", v(0), |cv| bome(time::mktime(&cv.1))),
    ])
}

#[cfg(feature = "log")]
fn log<D: DataT>() -> Box<[Filter<RunPtr<D>>]>
where
    for<'a> D::V<'a>: ValT,
{
    fn eprint_raw<V: ValT>(v: &V) {
        if let Some(s) = v.as_utf8_bytes() {
            log::error!("{}", BStr::new(s))
        } else {
            log::error!("{v}")
        }
    }
    /// Construct a filter that applies an effect function before returning nothing.
    macro_rules! empty_with {
        ( $eff:expr ) => {
            |cv| {
                $eff(&cv.1);
                Box::new(core::iter::empty())
            }
        };
    }
    Box::new([
        ("debug_empty", v(0), empty_with!(|x| log::debug!("{x}"))),
        ("stderr_empty", v(0), empty_with!(eprint_raw)),
    ])
}
