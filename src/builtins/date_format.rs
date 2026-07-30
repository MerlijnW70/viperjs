//! §21.4.4.41's text forms and §21.4.1.32's grammar — a Date written out, and read back.
//!
//! These are pure functions over a time value, which is why they are here rather than beside the
//! methods that call them: every one is testable without a heap, a realm or a receiver.
//!
//! # The round trip is a requirement, not a courtesy
//!
//! §21.4.3.2 obliges `Date.parse` to read back what `toString`, `toUTCString` and `toISOString`
//! wrote, so the three writers and the reader here are one design. That is why [`parse_text`] takes
//! three passes rather than one grammar: the three formats agree on nothing except that they name
//! the same instant.
//!
//! Beyond those three, what `parse` accepts is implementation-defined. This one is deliberately
//! strict — text it does not recognise is NaN, never a guess. An engine that guesses turns a typo
//! into a date a year out, and nothing downstream can tell.

use super::date::{
    MS_PER_HOUR, MS_PER_MINUTE, date_from_time, hour_from_time, local_time, make_date, make_day,
    make_time, min_from_time, month_from_time, ms_from_time, sec_from_time, time_clip,
    utc_from_local, week_day, year_from_time,
};

/// §21.4.1.8's weekday names, Sunday first.
const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// §21.4.1.6's month names, January first.
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// What an invalid Date says in every text form but ISO, which throws instead.
pub(super) const INVALID: &str = "Invalid Date";

/// A non-negative integer, zero-padded to `width`.
fn pad(value: f64, width: usize) -> String {
    format!("{:0>width$}", value.abs() as u64, width = width)
}

/// §21.4.4.41.1 `DateString(tv)` — `Thu Jan 01 1970`.
///
/// The year is four digits at least, and a negative one carries a `-` rather than being padded into
/// something unreadable.
fn date_text(t: f64) -> String {
    let year = year_from_time(t);
    let sign = if year < 0.0 { "-" } else { "" };
    format!(
        "{} {} {} {sign}{}",
        DAYS[week_day(t) as usize],
        MONTHS[month_from_time(t) as usize],
        pad(date_from_time(t), 2),
        pad(year, 4),
    )
}

/// §21.4.4.41.2 `TimeString(tv)` — `00:00:00 GMT`.
fn time_text(t: f64) -> String {
    format!(
        "{}:{}:{} GMT",
        pad(hour_from_time(t), 2),
        pad(min_from_time(t), 2),
        pad(sec_from_time(t), 2),
    )
}

/// §21.4.4.41.3 `TimeZoneString(tv)` — `+0000 (Coordinated Universal Time)`.
///
/// One offset and one name, because DR-0014 fixes the local zone at UTC. The name is the one every
/// engine uses for a zero offset, so text written here reads the same as text written elsewhere.
fn zone_text() -> String {
    "+0000 (Coordinated Universal Time)".to_string()
}

/// §21.4.4.41 `Date.prototype.toString` — `Thu Jan 01 1970 00:00:00 GMT+0000 (…)`.
///
/// Takes a *local* time value, as every method that reaches it already has one.
pub(super) fn full_text(t: f64) -> String {
    if t.is_nan() {
        return INVALID.to_string();
    }
    format!("{} {}{}", date_text(t), time_text(t), zone_text())
}

/// §21.4.4.35 `Date.prototype.toDateString`.
pub(super) fn day_text(t: f64) -> String {
    if t.is_nan() {
        return INVALID.to_string();
    }
    date_text(t)
}

/// §21.4.4.42 `Date.prototype.toTimeString`.
pub(super) fn clock_text(t: f64) -> String {
    if t.is_nan() {
        return INVALID.to_string();
    }
    format!("{}{}", time_text(t), zone_text())
}

/// §21.4.4.43 `Date.prototype.toUTCString` — `Thu, 01 Jan 1970 00:00:00 GMT`.
///
/// A different order from `toString` and a comma after the weekday: this is the HTTP-style form, and
/// its shape is why the parser needs a pass of its own for it. Takes a UTC time value.
pub(super) fn utc_text(t: f64) -> String {
    if t.is_nan() {
        return INVALID.to_string();
    }
    let year = year_from_time(t);
    let sign = if year < 0.0 { "-" } else { "" };
    format!(
        "{}, {} {} {sign}{} {}:{}:{} GMT",
        DAYS[week_day(t) as usize],
        pad(date_from_time(t), 2),
        MONTHS[month_from_time(t) as usize],
        pad(year, 4),
        pad(hour_from_time(t), 2),
        pad(min_from_time(t), 2),
        pad(sec_from_time(t), 2),
    )
}

/// §21.4.4.36 `Date.prototype.toISOString`, for a time value known to be finite.
///
/// A year outside 0..9999 is written with six digits and an explicit sign, because `+275760` and
/// `-000001` are the only spellings §21.4.1.32 gives for years the four-digit field cannot hold.
pub(super) fn iso_text(t: f64) -> String {
    let year = year_from_time(t);
    let year_text = if (0.0..=9999.0).contains(&year) {
        pad(year, 4)
    } else {
        // Sign and six digits in one expression. A separate `year < 0` test could never be observed
        // here: this branch is only reached for years outside 0..9999, so the boundary the test sits
        // on is unreachable, and mutation coverage reported it as such.
        format!("{:+07}", year as i64)
    };
    format!(
        "{year_text}-{}-{}T{}:{}:{}.{}Z",
        pad(month_from_time(t) + 1.0, 2),
        pad(date_from_time(t), 2),
        pad(hour_from_time(t), 2),
        pad(min_from_time(t), 2),
        pad(sec_from_time(t), 2),
        pad(ms_from_time(t), 3),
    )
}

/// §21.4.3.2's reader — a time value, or NaN for anything not recognised.
///
/// Three passes because there are three formats to honour, tried most-specific first. ISO is first
/// because it is the only one the specification pins down completely.
pub(super) fn parse_text(units: &[u16]) -> f64 {
    let Ok(text) = String::from_utf16(units) else {
        return f64::NAN;
    };
    let text = text.trim();
    parse_iso(text)
        .or_else(|| parse_utc_form(text))
        .or_else(|| parse_full_form(text))
        .map_or(f64::NAN, time_clip)
}

/// A cursor over ASCII, which every format here is. Anything else fails the parse rather than being
/// skipped, which is what keeps a stray non-ASCII digit from being read as a number.
struct Scan<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Scan<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    /// Exactly `count` digits as a number, or nothing — a short run is a failed parse, so `199` is
    /// not read as the year 1990.
    fn digits(&mut self, count: usize) -> Option<f64> {
        let end = self.at + count;
        let slice = self.bytes.get(self.at..end)?;
        if !slice.iter().all(u8::is_ascii_digit) {
            return None;
        }
        self.at = end;
        let mut value = 0.0;
        for byte in slice {
            value = value * 10.0 + f64::from(byte - b'0');
        }
        Some(value)
    }

    fn literal(&mut self, expected: u8) -> Option<()> {
        if self.peek() == Some(expected) {
            self.at += 1;
            return Some(());
        }
        None
    }

    fn done(&self) -> bool {
        self.at == self.bytes.len()
    }
}

/// §21.4.1.32's Date Time String Format.
///
/// The two absences that matter: a date with no time is midnight, and a date-time with no offset is
/// *local* while a date-only form is UTC. That asymmetry is in the specification and is observable
/// the moment a host supplies a non-zero offset, so it is honoured here even though DR-0014 makes
/// the two the same today.
fn parse_iso(text: &str) -> Option<f64> {
    let mut scan = Scan {
        bytes: text.as_bytes(),
        at: 0,
    };
    // An expanded year carries a sign and six digits; a plain one has four and no sign.
    let year = match scan.peek() {
        Some(b'+') | Some(b'-') => {
            let negative = scan.peek() == Some(b'-');
            scan.at += 1;
            let magnitude = scan.digits(6)?;
            // `-000000` is the one expanded year the grammar rejects: there is no negative zero
            // year, and accepting it would give two spellings for 0000.
            if negative && magnitude == 0.0 {
                return None;
            }
            if negative { -magnitude } else { magnitude }
        }
        _ => scan.digits(4)?,
    };
    let mut month = 1.0;
    let mut day_of_month = 1.0;
    if scan.peek() == Some(b'-') {
        scan.at += 1;
        month = scan.digits(2)?;
        if scan.peek() == Some(b'-') {
            scan.at += 1;
            day_of_month = scan.digits(2)?;
        }
    }
    let mut hours = 0.0;
    let mut minutes = 0.0;
    let mut seconds = 0.0;
    let mut ms = 0.0;
    let mut had_time = false;
    if scan.peek() == Some(b'T') || scan.peek() == Some(b't') {
        scan.at += 1;
        had_time = true;
        hours = scan.digits(2)?;
        scan.literal(b':')?;
        minutes = scan.digits(2)?;
        if scan.peek() == Some(b':') {
            scan.at += 1;
            seconds = scan.digits(2)?;
            if scan.peek() == Some(b'.') {
                scan.at += 1;
                ms = scan.digits(3)?;
            }
        }
    }
    // The offset, if there is one. `Z` is zero; otherwise a sign and `HH:mm`.
    let mut offset = None;
    match scan.peek() {
        Some(b'Z') | Some(b'z') => {
            scan.at += 1;
            offset = Some(0.0);
        }
        Some(sign @ (b'+' | b'-')) => {
            scan.at += 1;
            let oh = scan.digits(2)?;
            // The colon is optional in the wild and required by the grammar; accept both, because
            // refusing `+0000` would refuse text this engine's own `toString` produces.
            if scan.peek() == Some(b':') {
                scan.at += 1;
            }
            let om = scan.digits(2)?;
            let magnitude = oh * MS_PER_HOUR + om * MS_PER_MINUTE;
            offset = Some(if sign == b'-' { -magnitude } else { magnitude });
        }
        _ => {}
    }
    if !scan.done() {
        return None;
    }
    // Field ranges are checked here rather than left to `MakeDay` to carry, because ISO text is a
    // fixed grammar: `2000-13-01` is not January of 2001, it is not a date at all.
    if !(1.0..=12.0).contains(&month)
        || !(1.0..=31.0).contains(&day_of_month)
        || minutes > 59.0
        || seconds > 59.0
        || hours > 24.0
    {
        return None;
    }
    let day = make_day(year, month - 1.0, day_of_month);
    let time = make_time(hours, minutes, seconds, ms);
    let naive = make_date(day, time);
    Some(match offset {
        Some(offset) => naive - offset,
        // No offset: a date-time is local, a date alone is UTC.
        None if had_time => utc_from_local(naive),
        None => naive,
    })
}

/// The `toUTCString` form — `Thu, 01 Jan 1970 00:00:00 GMT`.
fn parse_utc_form(text: &str) -> Option<f64> {
    let mut scan = Scan {
        bytes: text.as_bytes(),
        at: 0,
    };
    weekday(&mut scan)?;
    scan.literal(b',')?;
    scan.literal(b' ')?;
    let day_of_month = scan.digits(2)?;
    scan.literal(b' ')?;
    let month = month_name(&mut scan)?;
    scan.literal(b' ')?;
    let year = signed_year(&mut scan)?;
    scan.literal(b' ')?;
    let (hours, minutes, seconds) = clock(&mut scan)?;
    scan.literal(b' ')?;
    for byte in b"GMT" {
        scan.literal(*byte)?;
    }
    if !scan.done() {
        return None;
    }
    Some(make_date(
        make_day(year, month, day_of_month),
        make_time(hours, minutes, seconds, 0.0),
    ))
}

/// The `toString` form — `Thu Jan 01 1970 00:00:00 GMT+0000 (Coordinated Universal Time)`.
fn parse_full_form(text: &str) -> Option<f64> {
    let mut scan = Scan {
        bytes: text.as_bytes(),
        at: 0,
    };
    weekday(&mut scan)?;
    scan.literal(b' ')?;
    let month = month_name(&mut scan)?;
    scan.literal(b' ')?;
    let day_of_month = scan.digits(2)?;
    scan.literal(b' ')?;
    let year = signed_year(&mut scan)?;
    scan.literal(b' ')?;
    let (hours, minutes, seconds) = clock(&mut scan)?;
    scan.literal(b' ')?;
    for byte in b"GMT" {
        scan.literal(*byte)?;
    }
    let sign = scan.peek()?;
    if sign != b'+' && sign != b'-' {
        return None;
    }
    scan.at += 1;
    let oh = scan.digits(2)?;
    let om = scan.digits(2)?;
    let magnitude = oh * MS_PER_HOUR + om * MS_PER_MINUTE;
    let offset = if sign == b'-' { -magnitude } else { magnitude };
    // The zone name in brackets is decoration — it repeats what the offset already said, so it is
    // accepted and ignored rather than checked against a table this engine does not have.
    if scan.peek() == Some(b' ') {
        scan.at = scan.bytes.len();
    }
    if !scan.done() {
        return None;
    }
    Some(
        make_date(
            make_day(year, month, day_of_month),
            make_time(hours, minutes, seconds, 0.0),
        ) - offset,
    )
}

/// Three letters naming a day, which the formats carry and none of them need: the instant is fully
/// determined without it, so it is required to be *present* and not required to agree.
fn weekday(scan: &mut Scan<'_>) -> Option<()> {
    let name = scan.bytes.get(scan.at..scan.at + 3)?;
    if !DAYS.iter().any(|day| day.as_bytes() == name) {
        return None;
    }
    scan.at += 3;
    Some(())
}

/// Three letters naming a month, answered as a zero-based index.
fn month_name(scan: &mut Scan<'_>) -> Option<f64> {
    let name = scan.bytes.get(scan.at..scan.at + 3)?;
    let at = MONTHS.iter().position(|month| month.as_bytes() == name)?;
    scan.at += 3;
    Some(at as f64)
}

/// A year of four digits or more, with an optional leading `-`.
fn signed_year(scan: &mut Scan<'_>) -> Option<f64> {
    let negative = scan.peek() == Some(b'-');
    if negative {
        scan.at += 1;
    }
    let mut value = scan.digits(4)?;
    // More than four digits for the years the padded field cannot hold.
    while scan.peek().is_some_and(|byte| byte.is_ascii_digit()) {
        value = value * 10.0 + f64::from(scan.peek()? - b'0');
        scan.at += 1;
    }
    Some(if negative { -value } else { value })
}

/// `HH:MM:SS`.
fn clock(scan: &mut Scan<'_>) -> Option<(f64, f64, f64)> {
    let hours = scan.digits(2)?;
    scan.literal(b':')?;
    let minutes = scan.digits(2)?;
    scan.literal(b':')?;
    let seconds = scan.digits(2)?;
    Some((hours, minutes, seconds))
}

/// The text `toLocaleString` and its two siblings answer.
///
/// §21.4.4.39 leaves the locale forms implementation-defined, and with no ECMA-402 here there is
/// nothing to vary by: they answer what the non-locale forms answer. Saying so once is better than
/// three functions that look like they might differ.
pub(super) fn locale_text(t: f64) -> String {
    full_text(local_time(t))
}
