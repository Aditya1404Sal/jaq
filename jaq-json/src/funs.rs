use crate::{jv_parse, Error, Val, ValR};
use alloc::{boxed::Box, vec::Vec};
use bstr::ByteSlice;
use bytes::{BufMut, Bytes, BytesMut};
use jaq_core::box_iter::BoxIter;
use jaq_core::native::{bome, run, unary, v, Filter, Fun};
use jaq_core::{DataT, RunPtr};
use jaq_std::ValT as _;

/// jq's value kinds, where `true` and `false` are two.
fn jq_kind(v: &Val) -> u8 {
    match v {
        Val::Null => 0,
        Val::Bool(false) => 1,
        Val::Bool(true) => 2,
        Val::Num(_) => 3,
        Val::TStr(_) | Val::BStr(_) => 4,
        Val::Arr(_) => 5,
        Val::Obj(_) => 6,
    }
}

impl Val {
    /// Return 0 for null, the absolute value for numbers, and
    /// the length for strings, arrays, and objects.
    ///
    /// Fail on booleans.
    fn length(&self) -> ValR {
        match self {
            Val::Null => Ok(Val::from(0usize)),
            Val::Num(n) => Ok(Val::Num(n.length())),
            Val::TStr(s) => Ok(Val::from(s.chars().count())),
            Val::BStr(b) => Ok(Val::from(b.len())),
            Val::Arr(a) => Ok(Val::from(a.len())),
            Val::Obj(o) => Ok(Val::from(o.len())),
            Val::Bool(_) => Err(crate::type_error(self, "has no length")),
        }
    }

    /// Return the indices of `y` in `self`.
    fn indices<'a>(&'a self, y: &'a Val) -> Result<Box<dyn Iterator<Item = usize> + 'a>, Error> {
        match (self, y) {
            (Val::BStr(_), Val::BStr(y)) | (Val::TStr(_), Val::TStr(y)) if y.is_empty() => {
                Ok(Box::new(core::iter::empty()))
            }
            (Val::TStr(x), Val::TStr(y)) => {
                let index = |(i, _, _)| x.get(i..i + y.len());
                let iw = x.char_indices().map_while(index).enumerate();
                Ok(Box::new(iw.filter_map(|(i, w)| (w == **y).then_some(i))))
            }
            (Val::BStr(x), Val::BStr(y)) => {
                let iw = x.windows(y.len()).enumerate();
                Ok(Box::new(iw.filter_map(|(i, w)| (w == **y).then_some(i))))
            }
            (Val::Arr(_), Val::Arr(y)) if y.is_empty() => Ok(Box::new(core::iter::empty())),
            (Val::Arr(x), Val::Arr(y)) => {
                let iw = x.windows(y.len()).enumerate();
                Ok(Box::new(iw.filter_map(|(i, w)| (w == **y).then_some(i))))
            }
            (Val::Arr(x), y) => {
                let ix = x.iter().enumerate();
                Ok(Box::new(ix.filter_map(move |(i, x)| (x == y).then_some(i))))
            }
            (x, y) => Err(crate::index_type_error(x, y)),
        }
    }

    /// `a` contains `b` iff either
    /// * the string `b` is a substring of `a`,
    /// * every element in the array `b` is contained in some element of the array `a`,
    /// * for every key-value pair `k, v` in `b`,
    ///   there is a key-value pair `k, v'` in `a` such that `v'` contains `v`, or
    /// * `a` equals `b`.
    fn contains(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::BStr(l), Self::BStr(r)) | (Self::TStr(l), Self::TStr(r)) => l.contains_str(&**r),
            (Self::Arr(l), Self::Arr(r)) => r.iter().all(|r| l.iter().any(|l| l.contains(r))),
            (Self::Obj(l), Self::Obj(r)) => r
                .iter()
                .all(|(k, r)| l.get(k).is_some_and(|l| l.contains(r))),
            _ => self == other,
        }
    }

    fn to_bytes(&self) -> Result<Bytes, Self> {
        match self {
            Val::Num(n) => n
                .as_isize()
                .and_then(|i| u8::try_from(i).ok())
                .map(|u| Bytes::from(Vec::from([u])))
                .ok_or_else(|| self.clone()),
            Val::BStr(b) | Val::TStr(b) => Ok(*b.clone()),
            Val::Arr(a) => {
                let mut buf = BytesMut::new();
                for x in a.iter() {
                    buf.put(Val::to_bytes(x)?);
                }
                Ok(buf.into())
            }
            _ => Err(self.clone()),
        }
    }

    fn as_bytes_owned(&self) -> Option<Bytes> {
        match self {
            Self::BStr(b) | Self::TStr(b) => Some(*b.clone()),
            _ => None,
        }
    }

    fn as_utf8_bytes_owned(&self) -> Option<Bytes> {
        self.is_utf8_str().then(|| self.as_bytes_owned()).flatten()
    }

    /// Return bytes if the value is a (byte or text) string.
    pub fn try_as_bytes_owned(&self) -> Result<Bytes, Error> {
        self.as_bytes_owned()
            .ok_or_else(|| Error::typ(self.clone(), "string"))
    }

    /// Return bytes if the value is a text string.
    pub fn try_as_utf8_bytes_owned(&self) -> Result<Bytes, Error> {
        self.as_utf8_bytes_owned()
            .ok_or_else(|| Error::typ(self.clone(), "string"))
    }
}

self_cell::self_cell!(
    struct BytesValRs {
        owner: Bytes,

        #[not_covariant]
        dependent: ValRs,
    }
);

impl Iterator for BytesValRs {
    type Item = ValR;
    fn next(&mut self) -> Option<Self::Item> {
        self.with_dependent_mut(|_owner, iter| iter.next())
    }
}

type ValRs<'a> = BoxIter<'a, ValR>;

/// Apply a function to bytes and yield the resulting value results.
pub fn bytes_valrs(b: Bytes, f: impl FnOnce(&[u8]) -> ValRs) -> ValRs<'static> {
    Box::new(BytesValRs::new(b, |b| f(b)))
}

/// Functions of the standard library.
pub fn funs<D: for<'a> DataT<V<'a> = Val>>() -> impl Iterator<Item = Fun<D>> {
    base().into_vec().into_iter().map(run)
}

fn base<D: for<'a> DataT<V<'a> = Val>>() -> Box<[Filter<RunPtr<D>>]> {
    Box::new([
        // jq 1.8's `fromjson`, `tonumber` and `toboolean`, with its messages.
        ("fromjson", v(0), |cv| {
            bome(match &cv.1 {
                Val::TStr(s) => jv_parse::parse_sized(s).map_err(Error::str),
                v => Err(crate::type_error(v, "only strings can be parsed")),
            })
        }),
        ("tonumber", v(0), |cv| {
            let message = "cannot be parsed as a number";
            bome(match &cv.1 {
                Val::Num(_) => Ok(cv.1.clone()),
                // jq reads the string as a C string: a NUL in it fails.
                Val::TStr(s) if !s.contains(&0) => jv_parse::number_literal(s)
                    .map(Val::Num)
                    .ok_or_else(|| crate::type_error(&cv.1, message)),
                v => Err(crate::type_error(v, message)),
            })
        }),
        ("toboolean", v(0), |cv| {
            bome(match &cv.1 {
                Val::Bool(_) => Ok(cv.1.clone()),
                Val::TStr(s) if &***s == b"true" => Ok(Val::Bool(true)),
                Val::TStr(s) if &***s == b"false" => Ok(Val::Bool(false)),
                v => Err(crate::type_error(v, "cannot be parsed as a boolean")),
            })
        }),
        ("tojson", v(0), |cv| bome(Ok(Val::utf8_str(cv.1.to_json())))),
        ("tobytes", v(0), |cv| {
            let fail = |v| Error::str(format_args!("cannot convert {v} to bytes"));
            bome(cv.1.to_bytes().map(Val::byte_str).map_err(fail))
        }),
        ("length", v(0), |cv| bome(cv.1.length())),
        // jq checks containment only between values of one kind (`true` and `false` are two).
        ("contains", v(1), |cv| {
            unary(cv, |x, y| {
                if jq_kind(&x) != jq_kind(&y) {
                    let message = "cannot have their containment checked";
                    return Err(crate::type_error2(&x, &y, message));
                }
                Ok(Val::from(x.contains(&y)))
            })
        }),
        ("has", v(1), |cv| {
            unary(cv, |v, k| match (&v, &k) {
                (Val::Obj(o), Val::TStr(_) | Val::BStr(_)) => Ok(o.contains_key(&k).into()),
                (Val::Arr(a), Val::Num(n)) => {
                    // jq truncates the index; NaN and negative indices are never present.
                    let d = n.as_f64();
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let present = !d.is_nan() && d > -1.0 && (d as usize) < a.len();
                    Ok(present.into())
                }
                _ => Err(Error::str(format_args!(
                    "Cannot check whether {} has a {} key",
                    crate::type_name(&v),
                    crate::type_name(&k)
                ))),
            })
        }),
        ("indices", v(1), |cv| {
            let to_int = |i: usize| Val::from(i as isize);
            unary(cv, move |x, v| {
                x.indices(&v).map(|idxs| idxs.map(to_int).collect())
            })
        }),
        ("bsearch", v(1), |cv| {
            let to_idx = |r: Result<_, _>| r.map_or_else(|i| -1 - i as isize, |i| i as isize);
            unary(cv, move |a, x| {
                a.as_arr().map(|a| Val::from(to_idx(a.binary_search(&x))))
            })
        }),
    ])
}
