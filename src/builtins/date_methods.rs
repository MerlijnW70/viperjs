//! §21.4.4 `Date.prototype` — the getters, the setters and the text forms.
//!
//! # The shape every method here shares
//!
//! Each one reads `[[DateValue]]` off the receiver, and a receiver without that slot is a TypeError
//! rather than NaN — that is [`this_time`], and it is why `Date.prototype.getTime()` throws while
//! `new Date(NaN).getTime()` answers NaN. The two are different failures and the specification keeps
//! them apart at every one of these forty-odd entry points.
//!
//! The local variants differ from the UTC ones by one call to `LocalTime`, which DR-0014 currently
//! makes an identity. They are still written as two, because the day a host supplies an offset the
//! difference has to already be in the right places.
//!
//! # Why the setters coerce before they check
//!
//! §21.4.4.23 and its neighbours read the slot, then convert *every* argument, and only then ask
//! whether the time value was NaN. The order is observable: `new Date(NaN).setMilliseconds(bad)`
//! must run `bad`'s `valueOf` and propagate what it throws, even though the answer would be NaN
//! either way. Checking first would swallow that.

use super::date::{
    date_from_time, day, hour_from_time, local_time, make_date, make_day, make_time, min_from_time,
    month_from_time, ms_from_time, sec_from_time, time_clip, time_within_day, two_digit_year,
    utc_from_local, week_day, year_from_time,
};
use super::date_format::{clock_text, day_text, full_text, iso_text, locale_text, utc_text};
use super::{define_method, define_value, text};
use crate::heap::{Heap, NativeCall, Object, ObjectId};
use crate::realm::Realm;
use crate::value::{Abrupt, Completion, Value};
use crate::vm::Vm;

/// Put §21.4.4's methods on `prototype`.
pub(super) fn install(heap: &mut Heap, realm: &Realm, prototype: ObjectId) {
    for (name, length, native) in [
        ("getTime", 0, get_time as crate::heap::Native),
        ("valueOf", 0, get_time),
        ("getFullYear", 0, get_full_year),
        ("getUTCFullYear", 0, get_utc_full_year),
        ("getMonth", 0, get_month),
        ("getUTCMonth", 0, get_utc_month),
        ("getDate", 0, get_date),
        ("getUTCDate", 0, get_utc_date),
        ("getDay", 0, get_day),
        ("getUTCDay", 0, get_utc_day),
        ("getHours", 0, get_hours),
        ("getUTCHours", 0, get_utc_hours),
        ("getMinutes", 0, get_minutes),
        ("getUTCMinutes", 0, get_utc_minutes),
        ("getSeconds", 0, get_seconds),
        ("getUTCSeconds", 0, get_utc_seconds),
        ("getMilliseconds", 0, get_milliseconds),
        ("getUTCMilliseconds", 0, get_utc_milliseconds),
        ("getTimezoneOffset", 0, get_timezone_offset),
        ("setTime", 1, set_time),
        ("setMilliseconds", 1, set_milliseconds),
        ("setUTCMilliseconds", 1, set_utc_milliseconds),
        ("setSeconds", 2, set_seconds),
        ("setUTCSeconds", 2, set_utc_seconds),
        ("setMinutes", 3, set_minutes),
        ("setUTCMinutes", 3, set_utc_minutes),
        ("setHours", 4, set_hours),
        ("setUTCHours", 4, set_utc_hours),
        ("setDate", 1, set_date),
        ("setUTCDate", 1, set_utc_date),
        ("setMonth", 2, set_month),
        ("setUTCMonth", 2, set_utc_month),
        ("setFullYear", 3, set_full_year),
        ("setUTCFullYear", 3, set_utc_full_year),
        ("toString", 0, to_string),
        ("toDateString", 0, to_date_string),
        ("toTimeString", 0, to_time_string),
        ("toUTCString", 0, to_utc_string),
        ("toISOString", 0, to_iso_string),
        ("toJSON", 1, to_json),
        ("toLocaleString", 0, to_locale_string),
        ("toLocaleDateString", 0, to_locale_string),
        ("toLocaleTimeString", 0, to_locale_string),
        // Annex B §B.2.3 — kept because the web kept it, and because `getYear` is the one place a
        // 1900 offset is still observable.
        ("getYear", 0, get_year),
        ("setYear", 1, set_year),
    ] {
        define_method(heap, realm, prototype, name, length, native);
    }
    // §B.2.3.3 — `toGMTString` is not a second function that does the same thing, it is *the same
    // function object*. A script comparing the two finds them equal, and one that replaces
    // `toUTCString` does not change what `toGMTString` does.
    if let Some(same) = super::own_value(heap, prototype, "toUTCString") {
        define_value(heap, prototype, "toGMTString", same);
    }
}

/// §21.4.4's `thisTimeValue` — the receiver's `[[DateValue]]`, or a TypeError.
///
/// The TypeError is for a receiver that is not a Date at all. A Date holding NaN is *not* that case
/// and answers `Ok(NaN)`, which is what every method here then carries through its arithmetic.
fn this_time(heap: &Heap, call: &NativeCall<'_>) -> Completion<f64> {
    if let Value::Object(object) = call.this_value
        && let Some(time) = heap.object(object).and_then(Object::date_value)
    {
        return Ok(time);
    }
    Err(Abrupt::type_error("this is not a Date"))
}

/// The receiver as an object, for the setters that have to write back.
fn this_date(call: &NativeCall<'_>) -> Option<ObjectId> {
    match call.this_value {
        Value::Object(object) => Some(object),
        _ => None,
    }
}

/// One getter: read the slot, and answer a field of it.
///
/// `local` is what separates each pair — `getHours` from `getUTCHours` — and a NaN time value
/// answers NaN without the field function ever running.
fn field(
    heap: &Heap,
    call: &NativeCall<'_>,
    local: bool,
    read: fn(f64) -> f64,
) -> Completion<Value> {
    let time = this_time(heap, call)?;
    // No NaN short-circuit: every reader below is arithmetic over the time value, so each already
    // answers NaN for one — the closed-form date conversion included, since it is float operations
    // end to end. The branch was unobservable and mutation coverage reported it as such.
    let time = if local { local_time(time) } else { time };
    Ok(Value::Number(read(time)))
}

/// §21.4.4.10 `getTime` and §21.4.4.44 `valueOf`, which are the same operation.
fn get_time(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    Ok(Value::Number(this_time(heap, call)?))
}

/// §21.4.4.4 `getFullYear`.
fn get_full_year(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    field(heap, call, true, year_from_time)
}

/// §21.4.4.14 `getUTCFullYear`.
fn get_utc_full_year(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    field(heap, call, false, year_from_time)
}

/// §21.4.4.8 `getMonth`.
fn get_month(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    field(heap, call, true, month_from_time)
}

/// §21.4.4.18 `getUTCMonth`.
fn get_utc_month(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    field(heap, call, false, month_from_time)
}

/// §21.4.4.2 `getDate`.
fn get_date(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    field(heap, call, true, date_from_time)
}

/// §21.4.4.12 `getUTCDate`.
fn get_utc_date(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    field(heap, call, false, date_from_time)
}

/// §21.4.4.3 `getDay`.
fn get_day(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    field(heap, call, true, week_day)
}

/// §21.4.4.13 `getUTCDay`.
fn get_utc_day(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    field(heap, call, false, week_day)
}

/// §21.4.4.5 `getHours`.
fn get_hours(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    field(heap, call, true, hour_from_time)
}

/// §21.4.4.15 `getUTCHours`.
fn get_utc_hours(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    field(heap, call, false, hour_from_time)
}

/// §21.4.4.7 `getMinutes`.
fn get_minutes(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    field(heap, call, true, min_from_time)
}

/// §21.4.4.17 `getUTCMinutes`.
fn get_utc_minutes(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    field(heap, call, false, min_from_time)
}

/// §21.4.4.9 `getSeconds`.
fn get_seconds(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    field(heap, call, true, sec_from_time)
}

/// §21.4.4.19 `getUTCSeconds`.
fn get_utc_seconds(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    field(heap, call, false, sec_from_time)
}

/// §21.4.4.6 `getMilliseconds`.
fn get_milliseconds(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    field(heap, call, true, ms_from_time)
}

/// §21.4.4.16 `getUTCMilliseconds`.
fn get_utc_milliseconds(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    field(heap, call, false, ms_from_time)
}

/// §21.4.4.11 `getTimezoneOffset` — in *minutes*, and with the sign the other way round.
///
/// `(t - LocalTime(t)) / msPerMinute`, so a zone ahead of UTC reports a negative number. DR-0014
/// makes it zero here, but the arithmetic is the specification's rather than a constant, so it will
/// be right when it is not.
fn get_timezone_offset(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let time = this_time(heap, call)?;
    // No NaN guard: `NaN - NaN` over a division is NaN, so the arithmetic already answers what the
    // guard would have returned. Mutation coverage reported the branch untestable, which is the
    // same statement.
    Ok(Value::Number(
        (time - local_time(time)) / super::date::MS_PER_MINUTE,
    ))
}

/// §21.4.4.20 `getYear` — Annex B, the year less 1900.
fn get_year(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    field(heap, call, true, |t| year_from_time(t) - 1900.0)
}

/// Read `count` arguments as Numbers, in order, before anything else happens.
///
/// In order and unconditionally, because each conversion may run a `valueOf` that throws or that
/// observes the ones before it. A setter that read them lazily would call them in the wrong order.
fn numbers(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    count: usize,
) -> Completion<Vec<f64>> {
    let mut read = Vec::with_capacity(count);
    for at in 0..count {
        read.push(vm.to_number(call.argument(at), heap)?);
    }
    Ok(read)
}

/// Write a new time value back, and answer it — the tail every setter shares.
fn store(heap: &mut Heap, call: &NativeCall<'_>, time: f64) -> Completion<Value> {
    let clipped = time_clip(time);
    if let Some(object) = this_date(call)
        && let Some(found) = heap.object_mut(object)
    {
        found.set_date_value(clipped);
    }
    Ok(Value::Number(clipped))
}

/// §21.4.4.27 `setTime`.
fn set_time(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    this_time(heap, call)?;
    let time = vm.to_number(call.argument(0), heap)?;
    store(heap, call, time)
}

/// The body every field setter shares.
///
/// `count` arguments are converted first, then the NaN check, then the fields the caller did not
/// give are read back off the existing time value. `local` decides which clock the fields are in.
fn set_fields(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    local: bool,
    count: usize,
    combine: fn(f64, &[f64], usize) -> f64,
) -> Completion<Value> {
    let time = this_time(heap, call)?;
    let given = numbers(vm, heap, call, count)?;
    // §21.4.4.23 step 6 — "if t is NaN, return NaN", and it returns *without writing*. That looks
    // redundant, because the arithmetic below would answer NaN anyway and the receiver already holds
    // NaN. It is not, and the difference is the conversion on the line above: a `valueOf` may have
    // called `setTime` in the meantime, so by now the receiver can hold a perfectly good instant
    // that this must not overwrite. `t` was read before that could happen, which is the whole point
    // of the step order.
    //
    // Removing this cost 24 test262 tests — the `date-value-read-before-tonumber-when-date-is-invalid`
    // family, one per setter — on the reasoning that writing NaN over NaN changes nothing. Reasoning
    // about equivalence has to account for what runs *between* the read and the write.
    if time.is_nan() {
        return Ok(Value::Number(f64::NAN));
    }
    let base = if local { local_time(time) } else { time };
    let combined = combine(base, &given, call.arguments.len());
    let result = if local {
        utc_from_local(combined)
    } else {
        combined
    };
    store(heap, call, result)
}

/// `MakeDate(Day(t), MakeTime(...))` with the hour, minute, second and millisecond fields chosen
/// from `given` where the call supplied one and from `t` where it did not.
///
/// `first` says which of the four this setter starts at, which is the only thing that differs
/// between `setHours`, `setMinutes`, `setSeconds` and `setMilliseconds`.
fn clock_fields(t: f64, given: &[f64], supplied: usize, first: usize) -> f64 {
    let mut fields = [
        hour_from_time(t),
        min_from_time(t),
        sec_from_time(t),
        ms_from_time(t),
    ];
    for (at, value) in given.iter().enumerate() {
        if at < supplied {
            fields[first + at] = *value;
        }
    }
    make_date(
        day(t),
        make_time(fields[0], fields[1], fields[2], fields[3]),
    )
}

/// §21.4.4.23 `setMilliseconds`.
fn set_milliseconds(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    set_fields(vm, heap, call, true, 1, |t, g, n| clock_fields(t, g, n, 3))
}

/// §21.4.4.31 `setUTCMilliseconds`.
fn set_utc_milliseconds(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    set_fields(vm, heap, call, false, 1, |t, g, n| clock_fields(t, g, n, 3))
}

/// §21.4.4.26 `setSeconds`.
fn set_seconds(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    set_fields(vm, heap, call, true, 2, |t, g, n| clock_fields(t, g, n, 2))
}

/// §21.4.4.34 `setUTCSeconds`.
fn set_utc_seconds(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    set_fields(vm, heap, call, false, 2, |t, g, n| clock_fields(t, g, n, 2))
}

/// §21.4.4.24 `setMinutes`.
fn set_minutes(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    set_fields(vm, heap, call, true, 3, |t, g, n| clock_fields(t, g, n, 1))
}

/// §21.4.4.32 `setUTCMinutes`.
fn set_utc_minutes(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    set_fields(vm, heap, call, false, 3, |t, g, n| clock_fields(t, g, n, 1))
}

/// §21.4.4.22 `setHours`.
fn set_hours(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    set_fields(vm, heap, call, true, 4, |t, g, n| clock_fields(t, g, n, 0))
}

/// §21.4.4.30 `setUTCHours`.
fn set_utc_hours(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    set_fields(vm, heap, call, false, 4, |t, g, n| clock_fields(t, g, n, 0))
}

/// §21.4.4.21 `setDate`.
fn set_date(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    set_fields(vm, heap, call, true, 1, |t, g, _| {
        make_date(
            make_day(year_from_time(t), month_from_time(t), g[0]),
            time_within_day(t),
        )
    })
}

/// §21.4.4.29 `setUTCDate`.
fn set_utc_date(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    set_fields(vm, heap, call, false, 1, |t, g, _| {
        make_date(
            make_day(year_from_time(t), month_from_time(t), g[0]),
            time_within_day(t),
        )
    })
}

/// §21.4.4.25 `setMonth`.
fn set_month(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    set_fields(vm, heap, call, true, 2, |t, g, n| {
        let date = if n > 1 { g[1] } else { date_from_time(t) };
        make_date(make_day(year_from_time(t), g[0], date), time_within_day(t))
    })
}

/// §21.4.4.33 `setUTCMonth`.
fn set_utc_month(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    set_fields(vm, heap, call, false, 2, |t, g, n| {
        let date = if n > 1 { g[1] } else { date_from_time(t) };
        make_date(make_day(year_from_time(t), g[0], date), time_within_day(t))
    })
}

/// §21.4.4.21 `setFullYear` — the one setter that revives an invalid Date.
///
/// Step 5: a NaN time value becomes `+0` rather than staying NaN, so `new Date(NaN).setFullYear(2000)`
/// is a real date in 2000. Every other setter leaves NaN as NaN, which is why this cannot go through
/// [`set_fields`].
fn set_full_year(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    full_year(vm, heap, call, true)
}

/// §21.4.4.35 `setUTCFullYear`, which revives in the same way.
fn set_utc_full_year(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    full_year(vm, heap, call, false)
}

/// The body both full-year setters share.
fn full_year(
    vm: &mut Vm,
    heap: &mut Heap,
    call: &NativeCall<'_>,
    local: bool,
) -> Completion<Value> {
    let time = this_time(heap, call)?;
    let given = numbers(vm, heap, call, 3)?;
    // Step 5 — NaN becomes the epoch, and only here.
    let base = if time.is_nan() {
        0.0
    } else if local {
        local_time(time)
    } else {
        time
    };
    let supplied = call.arguments.len();
    let month = if supplied > 1 {
        given[1]
    } else {
        month_from_time(base)
    };
    let date = if supplied > 2 {
        given[2]
    } else {
        date_from_time(base)
    };
    let combined = make_date(make_day(given[0], month, date), time_within_day(base));
    let result = if local {
        utc_from_local(combined)
    } else {
        combined
    };
    store(heap, call, result)
}

/// §B.2.3.1 `setYear` — Annex B, and the two-digit rule lives here as well as in the constructor.
fn set_year(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let time = this_time(heap, call)?;
    let year = vm.to_number(call.argument(0), heap)?;
    let base = if time.is_nan() { 0.0 } else { local_time(time) };
    // A NaN year needs no guard of its own: `two_digit_year` leaves it alone, `MakeDay` answers NaN
    // for it, and `MakeDate` carries that out. The branch was unreachable by observation.
    let combined = make_date(
        make_day(
            two_digit_year(year),
            month_from_time(base),
            date_from_time(base),
        ),
        time_within_day(base),
    );
    store(heap, call, utc_from_local(combined))
}

/// §21.4.4.41 `toString`.
fn to_string(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let time = this_time(heap, call)?;
    let written = full_text(if time.is_nan() {
        time
    } else {
        local_time(time)
    });
    Ok(text(heap, &written))
}

/// §21.4.4.35 `toDateString`.
fn to_date_string(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let time = this_time(heap, call)?;
    let written = day_text(if time.is_nan() {
        time
    } else {
        local_time(time)
    });
    Ok(text(heap, &written))
}

/// §21.4.4.42 `toTimeString`.
fn to_time_string(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let time = this_time(heap, call)?;
    let written = clock_text(if time.is_nan() {
        time
    } else {
        local_time(time)
    });
    Ok(text(heap, &written))
}

/// §21.4.4.43 `toUTCString`, which takes the time value unshifted.
fn to_utc_string(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let time = this_time(heap, call)?;
    let written = utc_text(time);
    Ok(text(heap, &written))
}

/// §21.4.4.36 `toISOString` — the one text form that *throws* rather than saying "Invalid Date".
///
/// A RangeError, because ISO 8601 has no spelling for an instant that is not one, and answering text
/// that does not parse back would break the round trip every other format here keeps.
fn to_iso_string(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let time = this_time(heap, call)?;
    if !time.is_finite() {
        return Err(Abrupt::range_error("an invalid Date has no ISO form"));
    }
    let written = iso_text(time);
    Ok(text(heap, &written))
}

/// §21.4.4.37 `toJSON`.
///
/// Not a Date method in the way the others are: it takes `this` as an *object* rather than requiring
/// the slot, so it works on anything with a `toISOString`. And a non-finite time value is `null`
/// rather than a throw — which is what lets `JSON.stringify` of an invalid Date produce JSON at all.
fn to_json(vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let primitive = vm.to_primitive(call.this_value, crate::value::Hint::Number, heap)?;
    if let Value::Number(number) = primitive
        && !number.is_finite()
    {
        return Ok(Value::Null);
    }
    let name = super::key(heap, "toISOString");
    let method = vm.get_property_key(call.this_value, name, heap)?;
    vm.call_value(method, call.this_value, &[], heap)
}

/// §21.4.4.38, .39 and .40 — the three locale forms, which have nothing to vary by here.
fn to_locale_string(_vm: &mut Vm, heap: &mut Heap, call: &NativeCall<'_>) -> Completion<Value> {
    let time = this_time(heap, call)?;
    let written = if time.is_nan() {
        super::date_format::INVALID.to_string()
    } else {
        locale_text(time)
    };
    Ok(text(heap, &written))
}
