//! Integer / decimal numbers.
use super::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt;
use core::hash::{Hash, Hasher};
use num_bigint::{BigInt, BigUint, Sign};
use num_traits::cast::ToPrimitive;

/// Integer / decimal number.
///
/// The speciality of this type is that numbers are distinguished into
/// integers and 64-bit floating-point numbers.
/// This allows using integers to index arrays,
/// while using floating-point numbers to do general math.
///
/// Operations on numbers follow a few principles:
/// * The sum, difference, product, and remainder of two integers is integer.
/// * Any other operation between two numbers yields a float.
#[derive(Clone, Debug)]
pub enum Num {
    /// Machine-size integer
    Int(isize),
    /// Arbitrarily large integer
    BigInt(Rc<BigInt>),
    /// Floating-point number
    Float(f64),
    /// Decimal number
    Dec(Rc<String>),
}

impl Num {
    /// Create a big integer.
    pub fn big_int(i: BigInt) -> Self {
        Self::BigInt(i.into())
    }

    /// A number literal (program text or JSON input). A zero stays a decimal literal: jq
    /// negates a literal zero to `0` (decNumber's `0 - x`), but any computed zero, an `Int`
    /// here, to `-0`.
    pub(crate) fn from_str(s: &str) -> Self {
        match Self::from_str_radix(s, 10) {
            Some(Self::Int(0)) | None => Self::Dec(Rc::new(s.to_string())),
            Some(n) => n,
        }
    }

    /// Convert from an integral type to a machine-sized or big integer.
    pub fn from_integral<T: Copy + TryInto<isize> + Into<BigInt>>(x: T) -> Self {
        x.try_into()
            .map_or_else(|_| Num::big_int(x.into()), Num::Int)
    }

    /// Try to parse an integer from a string with given radix.
    pub fn from_str_radix(i: &str, radix: u32) -> Option<Self> {
        let int = || isize::from_str_radix(i, radix).ok().map(Num::Int);
        let big = || bigint_from_str_radix(i, radix).map(Self::big_int);
        int().or_else(big)
    }

    /// Try to parse a decimal string to a [`Self::Float`], else return NaN.
    pub fn from_dec_str(n: &str) -> Self {
        // TODO: changed to NaN!
        n.parse().map_or(Self::Float(f64::NAN), Self::Float)
    }

    pub(crate) fn is_int(&self) -> bool {
        matches!(self, Self::Int(_) | Self::BigInt(_)) || self.integral_float().is_some()
    }

    /// A double (or decimal) with no fractional part that fits a machine-sized integer, which
    /// jq uses as that integer (`-1 * 0` is jq's `-0`, and still indexes like `0`).
    fn integral_float(&self) -> Option<isize> {
        let f = match self {
            Self::Float(f) => *f,
            Self::Dec(n) => n.parse().ok()?,
            _ => return None,
        };
        let range = isize::MIN as f64..isize::MAX as f64;
        (f.fract() == 0.0 && range.contains(&f)).then_some(f as isize)
    }

    /// If the value is a machine-sized integer, return it, else fail.
    pub fn as_isize(&self) -> Option<isize> {
        match self {
            Self::Int(i) => Some(*i),
            Self::BigInt(i) => i.to_isize(),
            _ => self.integral_float(),
        }
    }

    pub(crate) fn as_pos_usize(&self) -> Option<PosUsize> {
        match self {
            Self::Int(i) => Some(PosUsize(*i >= 0, i.unsigned_abs())),
            Self::BigInt(i) => i
                .magnitude()
                .to_usize()
                .map(|u| PosUsize(i.sign() != Sign::Minus, u)),
            _ => self
                .integral_float()
                .map(|i| PosUsize(i >= 0, i.unsigned_abs())),
        }
    }

    /// If the value is or can be converted to float, return it, else fail.
    pub(crate) fn as_f64(&self) -> f64 {
        match self {
            Self::Int(n) => *n as f64,
            Self::BigInt(n) => n.to_f64().unwrap(),
            Self::Float(n) => *n,
            Self::Dec(n) => n.parse().unwrap(),
        }
    }

    pub(crate) fn length(&self) -> Self {
        match self {
            Self::Int(i) => Self::Int(i.abs()),
            Self::BigInt(i) => match i.sign() {
                Sign::Plus | Sign::NoSign => Self::BigInt(i.clone()),
                Sign::Minus => Self::BigInt(BigInt::from(i.magnitude().clone()).into()),
            },
            Self::Dec(n) => Self::from_dec_str(n).length(),
            Self::Float(f) => Self::Float(f.abs()),
        }
    }
}

#[derive(Copy, Clone)]
pub(crate) struct PosUsize(pub(crate) bool, pub(crate) usize);

impl PosUsize {
    pub fn wrap(&self, len: usize) -> Option<usize> {
        self.0.then_some(self.1).or_else(|| len.checked_sub(self.1))
    }
}

#[test]
fn wrap_test() {
    let len = 4;
    let pos = |i| PosUsize(true, i);
    let neg = |i| PosUsize(false, i);
    assert_eq!(pos(0).wrap(len), Some(0));
    assert_eq!(pos(8).wrap(len), Some(8));
    assert_eq!(neg(1).wrap(len), Some(3));
    assert_eq!(neg(4).wrap(len), Some(0));
    assert_eq!(neg(8).wrap(len), None);
}

fn int_or_big<const N: usize>(
    i: Option<isize>,
    x: [isize; N],
    f: fn([BigInt; N]) -> BigInt,
) -> Num {
    i.map_or_else(|| Num::big_int(f(x.map(BigInt::from))), Num::Int)
}

// Do not use `BigInt::parse_bytes`, because it accepts arbitrary underscores between digits.
// https://github.com/rust-num/num-bigint/issues/340
fn bigint_from_str_radix(s: &str, radix: u32) -> Option<BigInt> {
    use num_bigint::Sign::{Minus, Plus};
    let f = |c, sign| s.strip_prefix(c).map(|s| (sign, s));
    let (sign, num) = f('-', Minus).or_else(|| f('+', Plus)).unwrap_or((Plus, s));
    biguint_from_str_radix(num, radix).map(|bu| BigInt::from_biguint(sign, bu))
}

fn biguint_from_str_radix(s: &str, radix: u32) -> Option<BigUint> {
    assert!((2..=36).contains(&radix));
    if s.is_empty() {
        return None;
    }

    // normalize all characters to plain digit values
    let digits = s.bytes().map(|b| {
        Some(match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'z' => b - b'a' + 10,
            b'A'..=b'Z' => b - b'A' + 10,
            _ => return None,
        })
        .filter(|d| *d < radix as u8)
    });
    BigUint::from_radix_be(&digits.collect::<Option<Vec<_>>>()?, radix)
}

/// Integers of at most this magnitude are exact as doubles. jq computes on doubles, so an
/// integer operand or result past it becomes the double jq would have.
const EXACT_INT: i128 = 1 << 53;

/// `+`, `-` or `*` of two integers as jq computes them: exactly while operands and result are
/// within [`EXACT_INT`], else in doubles.
fn int_arith(x: isize, y: isize, exact: fn(i128, i128) -> i128, float: fn(f64, f64) -> f64) -> Num {
    let (a, b) = (x as i128, y as i128);
    if a.abs() > EXACT_INT || b.abs() > EXACT_INT {
        return Num::Float(float(x as f64, y as f64));
    }
    let r = exact(a, b);
    if r.abs() > EXACT_INT {
        Num::Float(r as f64)
    } else if r == 0 && float(a as f64, b as f64).is_sign_negative() {
        // `0 * -1` is jq's `-0`.
        Num::Float(-0.0)
    } else {
        exact_int(r)
    }
}

/// An integer within [`EXACT_INT`], as a machine-sized integer where it fits (`isize` may be
/// 32 bits wide).
fn exact_int(r: i128) -> Num {
    isize::try_from(r).map_or_else(|_| Num::big_int(BigInt::from(r)), Num::Int)
}

fn big_f64(i: &BigInt) -> f64 {
    // `BigInt::to_f64` always yields `Some`, rounding large values to infinity.
    i.to_f64().unwrap_or(f64::NAN)
}

/// jq's `dtoi`: a double to `intmax_t`, saturating.
pub(crate) fn dtoi(n: f64) -> i64 {
    // Rust's `as` saturates too; NaN has been ruled out by the caller.
    n as i64
}

impl Num {
    /// Whether jq's `%` would refuse this divisor: one that truncates to 0.
    pub(crate) fn is_zero_divisor(&self) -> bool {
        let f = self.as_f64();
        !f.is_nan() && dtoi(f) == 0
    }
}

impl core::ops::Add for Num {
    type Output = Num;
    fn add(self, rhs: Self) -> Self::Output {
        use Num::*;
        match (self, rhs) {
            (Int(x), Int(y)) => int_arith(x, y, |x, y| x + y, |x, y| x + y),
            (Int(i), BigInt(b)) | (BigInt(b), Int(i)) => Float(i as f64 + big_f64(&b)),
            (Int(i), Float(f)) | (Float(f), Int(i)) => Float(f + i as f64),
            (BigInt(x), BigInt(y)) => Float(big_f64(&x) + big_f64(&y)),
            (BigInt(i), Float(f)) | (Float(f), BigInt(i)) => Float(f + big_f64(&i)),
            (Float(x), Float(y)) => Float(x + y),
            (Dec(n), r) => Self::from_dec_str(&n) + r,
            (l, Dec(n)) => l + Self::from_dec_str(&n),
        }
    }
}

impl core::ops::Sub for Num {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self::Output {
        use Num::*;
        match (self, rhs) {
            (Int(x), Int(y)) => int_arith(x, y, |x, y| x - y, |x, y| x - y),
            (Int(i), BigInt(b)) => Float(i as f64 - big_f64(&b)),
            (BigInt(b), Int(i)) => Float(big_f64(&b) - i as f64),
            (BigInt(x), BigInt(y)) => Float(big_f64(&x) - big_f64(&y)),
            (Float(f), Int(i)) => Float(f - i as f64),
            (Int(i), Float(f)) => Float(i as f64 - f),
            (Float(f), BigInt(i)) => Float(f - big_f64(&i)),
            (BigInt(i), Float(f)) => Float(big_f64(&i) - f),
            (Float(x), Float(y)) => Float(x - y),
            (Dec(n), r) => Self::from_dec_str(&n) - r,
            (l, Dec(n)) => l - Self::from_dec_str(&n),
        }
    }
}

impl core::ops::Mul for Num {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self::Output {
        use Num::*;
        match (self, rhs) {
            (Int(x), Int(y)) => int_arith(x, y, |x, y| x * y, |x, y| x * y),
            (Int(i), BigInt(b)) | (BigInt(b), Int(i)) => Float(i as f64 * big_f64(&b)),
            (BigInt(x), BigInt(y)) => Float(big_f64(&x) * big_f64(&y)),
            (BigInt(i), Float(f)) | (Float(f), BigInt(i)) => Float(f * big_f64(&i)),
            (Float(f), Int(i)) | (Int(i), Float(f)) => Float(f * i as f64),
            (Float(x), Float(y)) => Float(x * y),
            (Dec(n), r) => Self::from_dec_str(&n) * r,
            (l, Dec(n)) => l * Self::from_dec_str(&n),
        }
    }
}

impl core::ops::Div for Num {
    type Output = Self;
    fn div(self, rhs: Self) -> Self::Output {
        use Num::{BigInt, Dec, Float, Int};
        match (self, rhs) {
            (Int(l), r) => Float(l as f64) / r,
            (l, Int(r)) => l / Float(r as f64),
            (BigInt(l), r) => Float(big_f64(&l)) / r,
            (l, BigInt(r)) => l / Float(big_f64(&r)),
            (Float(x), Float(y)) => Float(x / y),
            (Dec(n), r) => Self::from_dec_str(&n) / r,
            (l, Dec(n)) => l / Self::from_dec_str(&n),
        }
    }
}

impl core::ops::Rem for Num {
    type Output = Self;
    /// jq's `%`: NaN if either side is NaN, else both sides truncated to integers (`dtoi`) and
    /// the C remainder of those. A divisor that truncates to 0 is refused by `Val`.
    fn rem(self, rhs: Self) -> Self::Output {
        use Num::Int;
        match (self, rhs) {
            (Int(x), Int(y))
                if (x as i128).abs() <= EXACT_INT && (y as i128).abs() <= EXACT_INT =>
            {
                // `x % -1` is 0 (jq checks for it, as `INTMAX_MIN % -1` overflows).
                Int(if y == -1 {
                    0
                } else {
                    x.checked_rem(y).unwrap_or(0)
                })
            }
            (l, r) => {
                let (x, y) = (l.as_f64(), r.as_f64());
                if x.is_nan() || y.is_nan() {
                    return Num::Float(f64::NAN);
                }
                let (x, y) = (dtoi(x), dtoi(y));
                let r = if y == -1 {
                    0
                } else {
                    x.checked_rem(y).unwrap_or(0)
                };
                let r = i128::from(r);
                if r.abs() > EXACT_INT {
                    Num::Float(r as f64)
                } else {
                    exact_int(r)
                }
            }
        }
    }
}

impl core::ops::Neg for Num {
    type Output = Self;
    fn neg(self) -> Self::Output {
        match self {
            // Every `Int` zero is computed (see `from_str`): jq's double, negated to `-0`.
            Self::Int(0) => Self::Float(-0.0),
            Self::Int(x) => int_or_big(x.checked_neg(), [x], |[x]| -x),
            Self::BigInt(x) => Self::big_int(-&*x),
            Self::Float(x) => Self::Float(-x),
            // decNumber negates as `0 - x`, so a zero literal comes out positive: `-0.0` is `0.0`.
            Self::Dec(n) if dec_is_zero(&n) => {
                let pos = n.strip_prefix(['-', '+']).unwrap_or(&n);
                Self::Dec(pos.to_string().into())
            }
            Self::Dec(n) => match n.strip_prefix('-') {
                Some(pos) => Self::Dec(pos.to_string().into()),
                None => Self::Dec(alloc::format!("-{}", n.strip_prefix('+').unwrap_or(&n)).into()),
            },
        }
    }
}

/// Whether a decimal literal's value is zero.
fn dec_is_zero(n: &str) -> bool {
    let mantissa = n.split(['e', 'E']).next().unwrap_or(n);
    !mantissa.bytes().any(|b| matches!(b, b'1'..=b'9'))
}

impl Hash for Num {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            // hash machine-sized integers like floating-point numbers,
            // because they are also compared for equality that way
            Self::Int(i) => Self::Float(*i as f64).hash(state),
            // hash all non-finite floats, like NaN and infinity, to the same
            // note that `Val::hash` assumes that `Num::hash` always starts
            // with `state.write_u8(n)`, where `n < 2`
            Self::Float(f) => {
                state.write_u8(0);
                if f.is_finite() {
                    f.to_ne_bytes().hash(state);
                }
            }
            Self::Dec(d) => Self::from_dec_str(d).hash(state),
            Self::BigInt(i) => {
                let f = i.to_f64().unwrap();
                if f.is_finite() {
                    Self::Float(f).hash(state)
                } else {
                    state.write_u8(1);
                    i.hash(state)
                }
            }
        }
    }
}

#[test]
fn hash_nums() {
    use core::f64::consts::PI;
    use core::hash::BuildHasher;
    let h = |n| foldhash::fast::FixedState::with_seed(42).hash_one(n);

    assert_eq!(h(Num::Int(4096)), h(Num::big_int(4096.into())));
    assert_eq!(h(Num::Float(0.0)), h(Num::Int(0)));
    assert_eq!(h(Num::Float(3.0)), h(Num::big_int(3.into())));

    assert_ne!(h(Num::Float(0.2)), h(Num::Float(0.4)));
    assert_ne!(h(Num::Float(PI)), h(Num::big_int(3.into())));
    assert_ne!(h(Num::Float(0.2)), h(Num::Int(1)));
    assert_ne!(h(Num::Int(1)), h(Num::Int(2)));
}

impl PartialEq for Num {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Int(x), Self::Int(y)) => x == y,
            (Self::Int(i), Self::Float(f)) | (Self::Float(f), Self::Int(i)) => {
                f.is_finite() && float_eq(*i as f64, *f)
            }
            (Self::BigInt(x), Self::BigInt(y)) => x == y,
            (Self::Int(i), Self::BigInt(b)) | (Self::BigInt(b), Self::Int(i)) => {
                **b == BigInt::from(*i)
            }
            (Self::BigInt(i), Self::Float(f)) | (Self::Float(f), Self::BigInt(i)) => {
                f.is_finite() && float_eq(i.to_f64().unwrap(), *f)
            }
            (Self::Float(x), Self::Float(y)) => float_eq(*x, *y),
            (Self::Dec(x), Self::Dec(y)) if Rc::ptr_eq(x, y) => true,
            (Self::Dec(n), y) => &Self::from_dec_str(n) == y,
            (x, Self::Dec(n)) => x == &Self::from_dec_str(n),
        }
    }
}

impl Eq for Num {}

impl PartialOrd for Num {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Num {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Int(x), Self::Int(y)) => x.cmp(y),
            (Self::Int(x), Self::BigInt(y)) => BigInt::from(*x).cmp(y),
            (Self::Int(i), Self::Float(f)) => float_cmp(*i as f64, *f),
            (Self::BigInt(x), Self::Int(y)) => (**x).cmp(&BigInt::from(*y)),
            (Self::BigInt(x), Self::BigInt(y)) => x.cmp(y),
            // BigInt::to_f64 always yields Some, large values become f64::INFINITY
            (Self::BigInt(x), Self::Float(y)) => float_cmp(x.to_f64().unwrap(), *y),
            (Self::Float(f), Self::Int(i)) => float_cmp(*f, *i as f64),
            (Self::Float(x), Self::BigInt(y)) => float_cmp(*x, y.to_f64().unwrap()),
            (Self::Float(x), Self::Float(y)) => float_cmp(*x, *y),
            (Self::Dec(x), Self::Dec(y)) if Rc::ptr_eq(x, y) => Ordering::Equal,
            (Self::Dec(n), y) => Self::from_dec_str(n).cmp(y),
            (x, Self::Dec(n)) => x.cmp(&Self::from_dec_str(n)),
        }
    }
}

fn float_eq(left: f64, right: f64) -> bool {
    float_cmp(left, right) == Ordering::Equal
}

fn float_cmp(left: f64, right: f64) -> Ordering {
    if left == 0. && right == 0. {
        // consider negative and positive 0 as equal
        Ordering::Equal
    } else if left.is_nan() {
        // there are more than 50 shades of NaN, and which of these
        // you strike when you perform a calculation is not deterministic (!),
        // therefore `total_cmp` may yield different results for the same calculation
        // so we bite the bullet and handle this like in jq
        Ordering::Less
    } else if right.is_nan() {
        Ordering::Greater
    } else {
        f64::total_cmp(&left, &right)
    }
}

impl fmt::Display for Num {
    /// Print numbers as jq 1.8 does: NaN as `null`, infinities as the largest finite doubles,
    /// computed doubles in jq's shortest `%g`-like form, and decimal literals canonically.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Int(i) => write!(f, "{i}"),
            Self::BigInt(i) => write!(f, "{i}"),
            Self::Float(x) if x.is_nan() => write!(f, "null"),
            Self::Float(x) if x.is_infinite() => fmt_jq_float(f, x.signum() * f64::MAX),
            Self::Float(x) => fmt_jq_float(f, *x),
            Self::Dec(n) => fmt_jq_dec(f, n),
        }
    }
}

/// jq's `jvp_dtoa_fmt`: shortest round-trip digits; exponential with a signed, at least
/// two-digit exponent when the decimal point is 4+ places left of the digits or more than 15
/// places right of them.
fn fmt_jq_float(f: &mut fmt::Formatter, x: f64) -> fmt::Result {
    if x == 0.0 {
        return write!(f, "{}", if x.is_sign_negative() { "-0" } else { "0" });
    }
    let scientific = alloc::format!("{:e}", x.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let digits: alloc::string::String = mantissa.chars().filter(|c| *c != '.').collect();
    let count = digits.len() as i32;
    let point = exponent + 1;
    if x < 0.0 {
        write!(f, "-")?;
    }
    if point <= -4 || point > count + 15 {
        write!(f, "{}", &digits[..1])?;
        if count > 1 {
            write!(f, ".{}", &digits[1..])?;
        }
        let power = point - 1;
        let sign = if power < 0 { '-' } else { '+' };
        write!(f, "e{sign}{:02}", power.abs())
    } else if point <= 0 {
        write!(f, "0.{}{digits}", "0".repeat(point.unsigned_abs() as usize))
    } else if point >= count {
        write!(f, "{digits}{}", "0".repeat((point - count) as usize))
    } else {
        let point = point as usize;
        write!(f, "{}.{}", &digits[..point], &digits[point..])
    }
}

/// decNumber's to-scientific-string, which jq 1.8 uses for number literals it preserves.
fn fmt_jq_dec(f: &mut fmt::Formatter, literal: &str) -> fmt::Result {
    let (negative, rest) = match literal.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, literal.strip_prefix('+').unwrap_or(literal)),
    };
    let (mantissa, exponent) = match rest.find(['e', 'E']) {
        Some(index) => (&rest[..index], rest[index + 1..].parse::<i64>()),
        None => (rest, Ok(0)),
    };
    let Ok(exponent) = exponent else {
        return write!(f, "{literal}");
    };
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let joined = alloc::format!("{integer}{fraction}");
    let trimmed = joined.trim_start_matches('0');
    let digits = if trimmed.is_empty() { "0" } else { trimmed };
    let count = digits.len() as i64;
    let exponent = exponent - fraction.len() as i64;
    let adjusted = exponent + count - 1;
    if negative {
        write!(f, "-")?;
    }
    if exponent <= 0 && adjusted >= -6 {
        let point = count + exponent;
        if exponent == 0 {
            write!(f, "{digits}")
        } else if point > 0 {
            let point = point as usize;
            write!(f, "{}.{}", &digits[..point], &digits[point..])
        } else {
            write!(f, "0.{}{digits}", "0".repeat(point.unsigned_abs() as usize))
        }
    } else {
        write!(f, "{}", &digits[..1])?;
        if count > 1 {
            write!(f, ".{}", &digits[1..])?;
        }
        let sign = if adjusted < 0 { '-' } else { '+' };
        write!(f, "E{sign}{}", adjusted.abs())
    }
}
