use crate::{Error, ValR, ValT, ValTx};
use alloc::string::{String, ToString};
use jiff::{civil::DateTime, fmt::strtime, tz, Timestamp};

/// Convert a UNIX epoch timestamp with optional fractions.
fn epoch_to_timestamp<V: ValT>(v: &V) -> Result<Timestamp, Error<V>> {
    let val = match v.as_isize() {
        Some(i) => i as i64 * 1000000,
        None => (v.try_as_f64()? * 1000000.0) as i64,
    };
    Timestamp::from_microsecond(val).map_err(Error::str)
}

/// Convert a date-time pair to a UNIX epoch timestamp.
fn timestamp_to_epoch<V: ValT>(ts: Timestamp, frac: bool) -> ValR<V> {
    if frac {
        Ok((ts.as_microsecond() as f64 / 1e6).into())
    } else {
        let seconds = ts.as_second();
        isize::try_from(seconds)
            .map(V::from)
            .or_else(|_| V::from_num(&seconds.to_string()))
    }
}

/// Convert a `DateTime` to a "broken down time" array
fn datetime_to_array<V: ValT>(dt: DateTime) -> [V; 8] {
    [
        V::from(dt.year() as isize),
        V::from(dt.month() as isize - 1),
        V::from(dt.day() as isize),
        V::from(dt.hour() as isize),
        V::from(dt.minute() as isize),
        if dt.subsec_nanosecond() > 0 {
            V::from(dt.second() as f64 + dt.subsec_nanosecond() as f64 / 1e9)
        } else {
            V::from(dt.second() as isize)
        },
        V::from(dt.weekday().to_sunday_zero_offset() as isize),
        V::from(dt.day_of_year() as isize - 1),
    ]
}

/// Parse an ISO 8601 timestamp string to a number holding the equivalent UNIX timestamp
/// (seconds elapsed since 1970/01/01).
///
/// Actually, this parses RFC 3339; see
/// <https://ijmacd.github.io/rfc3339-iso8601/> for differences.
/// jq also only parses a very restricted subset of ISO 8601.
pub fn from_iso8601<V: ValT>(s: &str) -> ValR<V> {
    timestamp_to_epoch(s.parse().map_err(Error::str)?, s.contains('.'))
}

/// Format a number as an ISO 8601 timestamp string.
pub fn to_iso8601<V: ValT>(v: &V) -> Result<String, Error<V>> {
    let ts = if let Some(i) = v.as_isize() {
        Timestamp::from_second(i as i64)
    } else {
        Timestamp::from_microsecond((v.try_as_f64()? * 1e6) as i64)
    };
    Ok(ts.map_err(Error::str)?.to_string())
}

/// Format a date (either number or array) in a given timezone.
///
/// When the input is a "broken down time" array,
/// then it is assumed to be in the given timezone.
/// When the input is an integer, i.e. a Unix epoch,
/// then it is *converted* to the given timezone.
pub fn strftime<V: ValT>(v: &V, fmt: &V, tz: tz::TimeZone, name: &str) -> ValR<V> {
    let inputs = || Error::str(format_args!("{name}/1 requires parsed datetime inputs"));
    let zoned = match v.clone().into_seq::<alloc::vec::Vec<V>>() {
        // jq normalizes a broken-down time as `timegm` does, and shows its wall clock.
        Ok(bdt) => {
            let fmt_ok = fmt.as_utf8_bytes().is_some();
            if !fmt_ok {
                return Err(Error::str(format_args!(
                    "{name}/1 requires a string format"
                )));
            }
            let seconds = broken_down_to_epoch(&bdt).ok_or_else(inputs)?;
            Timestamp::from_second(seconds)
                .map_err(Error::str)?
                .to_zoned(tz::TimeZone::UTC)
        }
        Err(v) if v.kind_name() == "number" => epoch_to_timestamp(&v)?.to_zoned(tz),
        Err(_) => return Err(inputs()),
    };
    let fmt = match fmt.as_utf8_bytes().map(core::str::from_utf8) {
        Some(Ok(fmt)) => fmt,
        _ => {
            return Err(Error::str(format_args!(
                "{name}/1 requires a string format"
            )))
        }
    };
    strtime::format(fmt, &zoned)
        .map(V::from)
        .map_err(Error::str)
}

/// Convert an epoch timestamp to a "broken down time" array.
pub fn gmtime<V: ValT>(v: &V, tz: tz::TimeZone) -> ValR<V> {
    let dt = epoch_to_timestamp(v)?.to_zoned(tz).into();
    datetime_to_array(dt).into_iter().map(Ok).collect()
}

/// Parse a string into a "broken down time" array.
pub fn strptime<V: ValT>(s: &str, fmt: &str) -> ValR<V> {
    let nomatch = |_| Error::str(format_args!("date \"{s}\" does not match format \"{fmt}\""));
    let mut bdt = strtime::BrokenDownTime::parse(fmt, s).map_err(nomatch)?;
    if (bdt.offset(), bdt.iana_time_zone()) == (None, None) {
        bdt.set_offset(Some(tz::Offset::UTC));
    }
    let dt = bdt.to_zoned().map_err(Error::str)?.into();
    datetime_to_array(dt).into_iter().map(Ok).collect()
}

/// Parse an array into a UNIX epoch timestamp, as jq's `mktime` (`jv2tm`, then `timegm`).
pub fn mktime<V: ValT>(v: &V) -> ValR<V> {
    let bdt: alloc::vec::Vec<V> = v
        .clone()
        .into_seq()
        .map_err(|_| Error::str("mktime requires array inputs"))?;
    let seconds = broken_down_to_epoch(&bdt)
        .ok_or_else(|| Error::str("mktime requires parsed datetime inputs"))?;
    isize::try_from(seconds)
        .map(V::from)
        .or_else(|_| V::from_num(&seconds.to_string()))
}

/// jq's `jv2tm` and `timegm`: the leading numbers of a broken-down time (missing ones 0), each
/// truncated to an integer, normalized into seconds since the epoch. `None` for an element that
/// is not a number.
fn broken_down_to_epoch<V: ValT>(bdt: &[V]) -> Option<i64> {
    let mut tm = [0i64; 6];
    for (i, v) in bdt.iter().take(8).enumerate() {
        let d = v.as_f64().filter(|d| !d.is_nan())?;
        let d = if i == 0 { d - 1900.0 } else { d };
        #[allow(clippy::cast_possible_truncation)]
        let n = d.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i64;
        if let Some(field) = tm.get_mut(i) {
            *field = n;
        }
    }
    let [year, month, mday, hour, min, sec] = tm;
    let year = year + 1900 + month.div_euclid(12);
    let month = month.rem_euclid(12) + 1;
    // Howard Hinnant's `days_from_civil`.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468 + (mday - 1);
    Some(days * 86_400 + hour * 3_600 + min * 60 + sec)
}
