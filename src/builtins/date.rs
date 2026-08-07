//! §21.4 `Date` — the time abstractions, the constructor, and the three static methods.
//!
//! # Why the arithmetic is spelled out
//!
//! §21.4.1 defines a dozen operations over a *time value*, a count of milliseconds since
//! 1970-01-01T00:00:00Z, and every method in §21.4.4 is a composition of them. Writing them as the
//! specification names them — [`day`], [`year_from_time`], [`make_day`], [`time_clip`] — costs
//! nothing and makes each one checkable against one paragraph. Deriving them ad hoc inside the
//! getters is how an engine ends up disagreeing with itself about what February 29th is.
//!
//! Two of them earn a note. `modulo` in §5.2.5 is *floored*, not truncated: its result takes the
//! sign of the divisor, so `-1 modulo 12` is 11 and not -1. Rust's `%` is truncated, so every use
//! goes through [`modulo`] instead — that is the whole reason dates before 1970 work here.
//! And there is no year zero problem to solve, because the specification counts years
//! arithmetically rather than by era: year 0 exists, and so do negative ones.
//!
//! The local time zone is UTC — see DR-0014, which explains why an engine with no dependencies and
//! no `unsafe` cannot ask the host and must not guess. Every local operation goes through
//! [`local_tza`], which is the invariant that record leaves behind.
//!
//! The getters, setters and formatters are [`super::date_methods`].

use super::{define_fixed, define_function_metadata, define_method, define_value};
use crate::heap::{Heap, NativeCall, ObjectId};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Hint, Value};
use crate::vm::Vm;

/// Milliseconds in a second — §21.4.1.2.
pub(super) const MS_PER_SECOND: f64 = 1000.0;
/// Milliseconds in a minute.
pub(super) const MS_PER_MINUTE: f64 = 60_000.0;
/// Milliseconds in an hour.
pub(super) const MS_PER_HOUR: f64 = 3_600_000.0;
/// Milliseconds in a day.
pub(super) const MS_PER_DAY: f64 = 86_400_000.0;

/// §21.4.1.1 — the largest magnitude a time value may have, 100 million days either side of the
/// epoch. Anything past it is not a date, and [`time_clip`] answers NaN for it.
pub(super) const MAX_TIME: f64 = 8.64e15;

/// §5.2.5 `x modulo y` — floored, so the result takes the sign of the divisor.
///
/// Rust's `%` truncates, which for `-1 % 12` gives `-1` where the specification wants `11`. Every
/// date before 1970 depends on this being the floored one.
pub(super) fn modulo(x: f64, y: f64) -> f64 {
    // No `+ 0.0` to normalise a negative zero: for a positive divisor this cannot produce one,
    // because reaching `-0.0` would need the subtrahend to be `+0.0` where this always computes it
    // as `-0.0`. Mutation coverage found the correction unreachable, which says the same thing more
    // cheaply. §21.4.1.31 still normalises the *time value*, where a sign is reachable and shows up
    // as `1 / new Date(-0).getTime()`.
    x - y * (x / y).floor()
}

/// §21.4.1.3 `Day(t)` — which day a time value falls in.
pub(super) fn day(t: f64) -> f64 {
    (t / MS_PER_DAY).floor()
}

/// §21.4.1.3 `TimeWithinDay(t)`.
pub(super) fn time_within_day(t: f64) -> f64 {
    modulo(t, MS_PER_DAY)
}

/// §21.4.1.5 to §21.4.1.7 — the year, month and day a day number falls in, computed exactly.
///
/// # Why this is not the estimate-and-correct loop it looks like it should be
///
/// The obvious implementation guesses the year from the mean Gregorian year and walks to the right
/// one. It gives correct answers and it is untestable: mutate the guess and the walk repairs it, so
/// every term in the estimate is a term no test can pin. Mutation coverage said exactly that, with
/// a survivor for the `1970` and for each loop bound.
///
/// This is the closed form instead — era arithmetic over the 146,097-day (400-year) Gregorian
/// cycle, in which every term is load-bearing. Break any one and some date is wrong, which is what
/// makes it checkable. It also removes the leap-year table and the March shift, because the
/// algorithm counts from March and folds January and February into the previous year: that is what
/// the `+ 3` / `- 9` and the `month <= 2` are doing.
///
/// Answers the month zero-based, as §21.4.1.6 counts them. Non-finite input propagates rather than
/// being guarded: every operation is float arithmetic, so no cast and no index is reached.
fn civil_from_day(day: f64) -> (f64, f64, f64) {
    // Shift the epoch to 0000-03-01, where the 400-year cycle starts.
    let shifted = day + 719_468.0;
    let era = (shifted / 146_097.0).floor();
    // Day of era, 0 to 146,096.
    let doe = shifted - era * 146_097.0;
    // Year of era, 0 to 399. The three corrections are the leap rule: every fourth year, except
    // every hundredth, except every four-hundredth.
    let yoe = ((doe - (doe / 1460.0).floor() + (doe / 36_524.0).floor()
        - (doe / 146_096.0).floor())
        / 365.0)
        .floor();
    let year = yoe + era * 400.0;
    // Day of year counted from March 1st, 0 to 365.
    let doy = doe - (365.0 * yoe + (yoe / 4.0).floor() - (yoe / 100.0).floor());
    // The month, still counted from March. The 153-over-5 is the repeating five-month run of 31 and
    // 30 day months, which is what makes this exact rather than tabulated.
    let mp = ((5.0 * doy + 2.0) / 153.0).floor();
    let date = doy - ((153.0 * mp + 2.0) / 5.0).floor() + 1.0;
    // Back to January-first counting.
    let month = mp + if mp < 10.0 { 3.0 } else { -9.0 };
    let year = if month <= 2.0 { year + 1.0 } else { year };
    (year, month - 1.0, date)
}

/// The day number of a civil date — the inverse of [`civil_from_day`], exact for the same reason.
///
/// `month` is 1 through 12 here, as the era arithmetic counts them.
fn day_from_civil(year: f64, month: f64, date: f64) -> f64 {
    // January and February belong to the previous era-year, which is what puts the leap day at the
    // end of it.
    let year = if month <= 2.0 { year - 1.0 } else { year };
    let era = (year / 400.0).floor();
    let yoe = year - era * 400.0;
    let shift = if month > 2.0 { -3.0 } else { 9.0 };
    let doy = ((153.0 * (month + shift) + 2.0) / 5.0).floor() + date - 1.0;
    let doe = yoe * 365.0 + (yoe / 4.0).floor() - (yoe / 100.0).floor() + doy;
    era * 146_097.0 + doe - 719_468.0
}

/// §21.4.1.5 `YearFromTime(t)`.
pub(super) fn year_from_time(t: f64) -> f64 {
    civil_from_day(day(t)).0
}

/// §21.4.1.6 `MonthFromTime(t)` — 0 for January.
pub(super) fn month_from_time(t: f64) -> f64 {
    civil_from_day(day(t)).1
}

/// §21.4.1.7 `DateFromTime(t)` — the day of the month, 1-based.
pub(super) fn date_from_time(t: f64) -> f64 {
    civil_from_day(day(t)).2
}

/// §21.4.1.8 `WeekDay(t)` — 0 for Sunday.
///
/// The `+ 4` is the epoch itself: 1970-01-01 was a Thursday.
pub(super) fn week_day(t: f64) -> f64 {
    modulo(day(t) + 4.0, 7.0)
}

/// §21.4.1.11 `HourFromTime(t)`.
pub(super) fn hour_from_time(t: f64) -> f64 {
    modulo((t / MS_PER_HOUR).floor(), 24.0)
}

/// §21.4.1.11 `MinFromTime(t)`.
pub(super) fn min_from_time(t: f64) -> f64 {
    modulo((t / MS_PER_MINUTE).floor(), 60.0)
}

/// §21.4.1.11 `SecFromTime(t)`.
pub(super) fn sec_from_time(t: f64) -> f64 {
    modulo((t / MS_PER_SECOND).floor(), 60.0)
}

/// §21.4.1.11 `msFromTime(t)`.
pub(super) fn ms_from_time(t: f64) -> f64 {
    modulo(t, MS_PER_SECOND)
}

thread_local! {
    /// The host's offset from UTC in milliseconds — DR-0014's hook, and zero until something sets it.
    ///
    /// Thread-local for the reason [`super::math`]'s generator seed is: it is host state rather than
    /// realm state, and a `Heap` does not own it. Per-thread rather than global so that two engines
    /// on two threads cannot change each other's clock.
    static LOCAL_OFFSET: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
}

/// The offset from UTC the engine is currently treating as local, in milliseconds.
pub fn local_offset() -> f64 {
    LOCAL_OFFSET.with(std::cell::Cell::get)
}

/// Tell the engine what the host's offset from UTC is, in milliseconds east of it.
///
/// DR-0014 fixes the *default* at zero, because a standard library that reports only UTC leaves an
/// engine with no dependencies and no `unsafe` nothing to ask, and guessing would make one script
/// answer differently on two machines. It does not stop a host that *does* know from saying so, and
/// this is where it says it: `+3_600_000` is UTC+01:00, and every local operation in §21.4 shifts by
/// it at once.
///
/// A non-finite offset is ignored rather than accepted, because a NaN here would make every local
/// getter NaN and look like a broken clock rather than a bad argument.
pub fn set_local_offset(milliseconds: f64) {
    if milliseconds.is_finite() {
        LOCAL_OFFSET.with(|offset| offset.set(milliseconds.trunc()));
    }
}

/// §21.4.1.9 `LocalTZA(t, isUTC)`.
///
/// The single place the offset is read — that is DR-0014's invariant, and it is what makes the local
/// and UTC halves of §21.4.4 two different answers rather than one written twice.
fn local_tza() -> f64 {
    local_offset()
}

/// §21.4.1.12 `LocalTime(t)` — UTC to local.
pub(super) fn local_time(t: f64) -> f64 {
    t + local_tza()
}

/// §21.4.1.13 `UTC(t)` — local to UTC.
pub(super) fn utc_from_local(t: f64) -> f64 {
    t - local_tza()
}

/// §7.1.5 `ToIntegerOrInfinity`, reduced to what it actually does here.
///
/// The specification's NaN and infinity cases are absent on purpose. [`make_date`] is the single
/// gate that turns a non-finite field into NaN, and every path through this module runs through it,
/// so a branch here for NaN or infinity is a branch no input can reach — mutation coverage reported
/// both as untestable. A guard that cannot be observed only looks tested.
pub(super) fn to_integer(value: f64) -> f64 {
    value.trunc()
}

/// §21.4.1.14 `MakeTime(hour, min, sec, ms)`.
///
/// No finiteness guard of its own: a non-finite field makes the sum non-finite, and [`make_date`] —
/// which every caller hands the result to — answers NaN for that. One gate rather than four.
pub(super) fn make_time(hour: f64, min: f64, sec: f64, ms: f64) -> f64 {
    to_integer(hour) * MS_PER_HOUR
        + to_integer(min) * MS_PER_MINUTE
        + to_integer(sec) * MS_PER_SECOND
        + to_integer(ms)
}

/// §21.4.1.15 `MakeDay(year, month, date)`.
///
/// A month outside 0..11 is not an error — it *carries*, which is why `new Date(2000, 12, 1)` is
/// January of 2001 and why `setMonth(-1)` walks back into the previous year.
/// Guardless for the reason [`make_time`] is: non-finite arithmetic stays non-finite through
/// [`day_from_civil`], which is float operations end to end with no cast or index to reach, and
/// [`make_date`] is the one gate that turns it into NaN.
pub(super) fn make_day(year: f64, month: f64, date: f64) -> f64 {
    let y = to_integer(year);
    let m = to_integer(month);
    let dt = to_integer(date);
    // A month outside 0..11 carries into the year rather than being refused, which is why
    // `new Date(2000, 12, 1)` is January of 2001.
    let ym = y + (m / 12.0).floor();
    let mn = modulo(m, 12.0);
    day_from_civil(ym, mn + 1.0, 1.0) + dt - 1.0
}

/// §21.4.1.16 `MakeDate(day, time)`.
pub(super) fn make_date(day: f64, time: f64) -> f64 {
    // One check, at the end. A leading guard on the arguments asks the same question the result
    // already answers — a non-finite day or time makes the sum non-finite, and infinities of
    // opposite sign make it NaN — so the two are indistinguishable and mutation coverage said so.
    // This is the gate the rest of §21.4.1 leans on: `MakeTime` and `MakeDay` are guardless because
    // everything they compute arrives here.
    let result = day * MS_PER_DAY + time;
    if result.is_finite() { result } else { f64::NAN }
}

/// §21.4.1.31 `TimeClip(time)`.
///
/// Out of range is NaN rather than an error, and the truncation is what makes every time value an
/// integral number of milliseconds. The `+ 0.0` turns `-0` into `+0`, which §21.4.1.31 step 4 asks
/// for explicitly — a Date at the epoch must not remember a sign.
pub(super) fn time_clip(time: f64) -> f64 {
    if !time.is_finite() || time.abs() > MAX_TIME {
        return f64::NAN;
    }
    to_integer(time) + 0.0
}

/// The current time, in milliseconds since the epoch.
///
/// A clock that cannot be read answers NaN, which every Date operation already handles — an engine
/// on a host with no clock produces invalid dates rather than refusing to run.
pub(super) fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(f64::NAN, |since| since.as_millis() as f64)
}

/// Build `Date` and its prototype into `heap`.
pub fn install(heap: &mut Heap, realm: &Realm, global: ObjectId) {
    let prototype = realm.date_prototype();
    let date = heap.new_native_constructor(realm.function_prototype(), construct, realm.id());
    define_function_metadata(heap, date, "Date", 7);
    define_fixed(heap, date, "prototype", Value::Object(prototype));
    define_value(heap, prototype, "constructor", Value::Object(date));
    define_value(heap, global, "Date", Value::Object(date));

    define_method(heap, realm, date, "now", 0, now);
    define_method(heap, realm, date, "parse", 1, parse);
    define_method(heap, realm, date, "UTC", 7, utc);

    // §21.4.4.45 — the one built-in `@@toPrimitive` that changes an answer rather than reporting
    // one, and the whole reason `date + 1` concatenates where `date - 1` subtracts. Not writable
    // and *configurable*, which is §21.4.4.45's own line and not §17's usual attributes.
    if let Some(symbol) = heap.well_known(super::well_known_at("toPrimitive")) {
        let method = heap.new_native_function(realm.function_prototype(), to_primitive, realm.id());
        define_function_metadata(heap, method, "[Symbol.toPrimitive]", 1);
        let _ = heap.define_own_property(
            prototype,
            crate::heap::PropertyKey::from_symbol(symbol),
            &crate::heap::PropertyDescriptor {
                value: Some(Value::Object(method)),
                writable: Some(false),
                enumerable: Some(false),
                configurable: Some(true),
                ..crate::heap::PropertyDescriptor::EMPTY
            },
        );
    }
    super::date_methods::install(heap, realm, prototype);
}

/// §21.4.4.45 `Date.prototype[@@toPrimitive](hint)`.
///
/// Reads `"default"` as `"string"`, which is the entire clause and the only place in the language
/// where the absence of a preference means anything. It is why `date + 1` is text and `date * 1`
/// is a number: `+` asks with no preference and lands here on `toString`, and every other
/// arithmetic operator asks for a number and lands on `valueOf`.
///
/// The three named hints are the only ones accepted. Anything else — including no argument at all
/// — is a **TypeError** rather than a fallback, so `Date.prototype[Symbol.toPrimitive].call(d)`
/// throws; the method is written to be reached by §7.1.1 and says so by refusing everything else.
fn to_primitive(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    // Step 2 — the receiver must be an Object, and *any* object: this reads no `[[DateValue]]` of
    // its own and is inherited by anything a script points at it.
    let Value::Object(_) = call.this_value else {
        return Err(Abrupt::type_error(
            "Date.prototype[Symbol.toPrimitive] requires an object",
        ));
    };
    let hint = match call.argument(0) {
        Value::String(id) => heap.string(id).unwrap_or(&[]).to_vec(),
        _ => Vec::new(),
    };
    let spelled = String::from_utf16_lossy(&hint);
    // Steps 3 and 4 — `"default"` joins `"string"`, and the two are not merely similar: the clause
    // lists them in one step precisely so that no reader can implement them apart.
    let hint = match spelled.as_str() {
        "default" | "string" => Hint::String,
        "number" => Hint::Number,
        _ => {
            return Err(Abrupt::type_error(
                "the hint given to Date.prototype[Symbol.toPrimitive] is not one of the three",
            ));
        }
    };
    // Step 5 — `OrdinaryToPrimitive`, which is §7.1.1.1 *without* step 1's lookup. Going back
    // through `ToPrimitive` would find this method again and recur until the stack ran out.
    vm.ordinary_to_primitive(call.this_value, hint, heap)
}

/// §21.4.2.1 `Date(...)` and `new Date(...)`.
///
/// Called rather than constructed it answers *text*, not a number and not a Date — §21.4.2.1 step 1
/// says so, and it is the one constructor in the language that ignores its arguments entirely when
/// called that way.
fn construct(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    if !call.constructing() {
        let text = super::date_format::full_text(local_time(now_ms()));
        return Ok(super::text(heap, &text));
    }
    let time = match call.arguments.len() {
        // §21.4.2.1 step 3 — no arguments is *now*.
        0 => now_ms(),
        1 => one_argument(vm, heap, call.argument(0))?,
        // §21.4.2.1 step 5 — two or more are fields, read in local time.
        _ => {
            let mut fields = [0.0; 7];
            // The defaults are not all zero: a missing day of the month is the 1st, because there
            // is no zeroth. Everything else counts from zero and so defaults to it.
            fields[2] = 1.0;
            for (at, field) in fields.iter_mut().enumerate().take(call.arguments.len()) {
                *field = vm.to_number(call.argument(at), heap)?;
            }
            let year = two_digit_year(fields[0]);
            let day = make_day(year, fields[1], fields[2]);
            let time = make_time(fields[3], fields[4], fields[5], fields[6]);
            utc_from_local(make_date(day, time))
        }
    };
    let prototype = super::prototype_from(heap, call, vm.realm().date_prototype());
    Ok(Value::Object(heap.new_date(prototype, time_clip(time))))
}

/// §21.4.2.1 step 4 — the single-argument form, which reads a String differently from a Number.
fn one_argument(vm: &mut Vm, heap: &mut Heap, argument: Value) -> Completion<f64> {
    // Step 4.a — a Date argument is *copied* rather than converted, so a subclass's `valueOf` does
    // not get a say and `new Date(d)` cannot lose precision through text.
    if let Value::Object(object) = argument
        && let Some(time) = heap
            .object(object)
            .and_then(crate::heap::Object::date_value)
    {
        return Ok(time);
    }
    let primitive = vm.to_primitive(argument, Hint::Number, heap)?;
    match primitive {
        // Step 4.b.i — text is parsed, which is the only place `Date.parse`'s grammar is reached
        // without calling it.
        Value::String(id) => {
            let units = heap.string(id).unwrap_or(&[]).to_vec();
            Ok(super::date_format::parse_text(&units))
        }
        other => vm.to_number(other, heap),
    }
}

/// §21.4.2.1 step 5.h — a year of 0 through 99 means 1900 through 1999.
///
/// Only for the field form: `new Date(99, 0)` is 1999, and `new Date(Date.UTC(99, 0))` is too,
/// while `new Date("0099-01-01")` is the year 99. The rule is about the *arguments*, not about
/// small years.
pub(super) fn two_digit_year(year: f64) -> f64 {
    let y = to_integer(year);
    if !year.is_nan() && (0.0..=99.0).contains(&y) {
        1900.0 + y
    } else {
        year
    }
}

/// §21.4.3.1 `Date.now()`.
fn now(_vm: &mut Vm, _heap: &mut Heap, _call: &NativeCall<'_>) -> Completion<Value> {
    Ok(Value::Number(time_clip(now_ms())))
}

/// §21.4.3.2 `Date.parse(string)`.
fn parse(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let text = vm.to_string(call.argument(0), heap)?;
    let units = heap.string(text).unwrap_or(&[]).to_vec();
    Ok(Value::Number(super::date_format::parse_text(&units)))
}

/// §21.4.3.4 `Date.UTC(year[, month[, date[, hours[, minutes[, seconds[, ms]]]]]])`.
///
/// The one place the field form is read as UTC rather than local, which is the whole of what it is
/// for. A missing year is NaN — unlike the constructor, there is no "now" fallback here.
fn utc(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let mut fields = [0.0; 7];
    fields[2] = 1.0;
    for (at, field) in fields.iter_mut().enumerate().take(call.arguments.len()) {
        *field = vm.to_number(call.argument(at), heap)?;
    }
    // §21.4.3.4 step 2 — with no arguments at all the year is `undefined`, whose ToNumber is NaN,
    // and the answer is NaN rather than the epoch.
    if call.arguments.is_empty() {
        return Ok(Value::Number(f64::NAN));
    }
    let year = two_digit_year(fields[0]);
    let day = make_day(year, fields[1], fields[2]);
    let time = make_time(fields[3], fields[4], fields[5], fields[6]);
    Ok(Value::Number(time_clip(make_date(day, time))))
}
