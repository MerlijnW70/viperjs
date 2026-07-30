//! §21.4 — `Date`, its arithmetic, its text forms and reading them back.
//!
//! The arithmetic is checked through the getters rather than directly, because that is the only way
//! a test can tell `MonthFromTime` from a plausible neighbour. Three themes carry most of the rows:
//! dates *before* the epoch, where the specification's floored modulo is the whole difference;
//! the leap-year rule in all four of its cases; and the boundary at ±8.64e15, on both sides.

use super::*;

#[test]
fn the_epoch_is_a_thursday_and_every_field_agrees_about_it() {
    assert_eq!(run("new Date(0).getTime()"), "0");
    assert_eq!(run("new Date(0).valueOf()"), "0");
    assert_eq!(run("new Date(0).getUTCFullYear()"), "1970");
    assert_eq!(run("new Date(0).getUTCMonth()"), "0");
    assert_eq!(run("new Date(0).getUTCDate()"), "1");
    // §21.4.1.8's `+ 4` — 1970-01-01 was a Thursday, and getting this wrong shifts every weekday
    // in the language by the same amount, which no other row would notice.
    assert_eq!(run("new Date(0).getUTCDay()"), "4");
    assert_eq!(run("new Date(0).getUTCHours()"), "0");
    assert_eq!(run("new Date(0).getUTCMinutes()"), "0");
    assert_eq!(run("new Date(0).getUTCSeconds()"), "0");
    assert_eq!(run("new Date(0).getUTCMilliseconds()"), "0");
    assert_eq!(run("new Date(0).getTimezoneOffset()"), "0");
    // A field is `+0` and not `-0`: §5.2.5's modulo is floored, and floating point can still land
    // on a negative zero, which `1/x` is the only way to see.
    assert_eq!(run("1 / new Date(0).getUTCMilliseconds()"), "Infinity");
    assert_eq!(run("1 / new Date(0).getUTCMonth()"), "Infinity");
}

#[test]
fn a_time_before_the_epoch_needs_the_floored_modulo() {
    // Every one of these is negative territory, where a truncating `%` would give the right day and
    // the wrong time — or a month of `-1`, which is not a month.
    assert_eq!(run("new Date(-1).getUTCFullYear()"), "1969");
    assert_eq!(run("new Date(-1).getUTCMonth()"), "11");
    assert_eq!(run("new Date(-1).getUTCDate()"), "31");
    assert_eq!(run("new Date(-1).getUTCHours()"), "23");
    assert_eq!(run("new Date(-1).getUTCMinutes()"), "59");
    assert_eq!(run("new Date(-1).getUTCSeconds()"), "59");
    assert_eq!(run("new Date(-1).getUTCMilliseconds()"), "999");
    assert_eq!(run("new Date(-1).getUTCDay()"), "3");
    // Year zero exists, and so do negative years: the specification counts arithmetically rather
    // than by era, so there is no gap to skip between 1 BC and AD 1.
    assert_eq!(run("new Date(-62167219200000).getUTCFullYear()"), "0");
    assert_eq!(run("new Date(-62198755200000).getUTCFullYear()"), "-1");
}

#[test]
fn the_leap_year_rule_is_all_four_of_its_cases() {
    // Not divisible by four: no leap day, so day 59 of the year is March 1st.
    assert_eq!(run("new Date(Date.UTC(2001, 1, 29)).getUTCMonth()"), "2");
    // Divisible by four and not by a hundred: February has 29 days.
    assert_eq!(run("new Date(Date.UTC(2004, 1, 29)).getUTCMonth()"), "1");
    assert_eq!(run("new Date(Date.UTC(2004, 1, 29)).getUTCDate()"), "29");
    // Divisible by a hundred and not by four hundred: *not* a leap year, which is the case a naive
    // "every fourth year" gets wrong and which no year in living memory exercises.
    assert_eq!(run("new Date(Date.UTC(1900, 1, 29)).getUTCMonth()"), "2");
    // Divisible by four hundred: a leap year again.
    assert_eq!(run("new Date(Date.UTC(2000, 1, 29)).getUTCMonth()"), "1");
    assert_eq!(run("new Date(Date.UTC(2000, 1, 29)).getUTCDate()"), "29");
    // …and the leap day shifts every month after February by one, which is what the table's
    // conditional offset is for.
    assert_eq!(run("new Date(Date.UTC(2000, 2, 1)).getUTCDate()"), "1");
    assert_eq!(run("new Date(Date.UTC(2000, 11, 31)).getUTCMonth()"), "11");
    assert_eq!(run("new Date(Date.UTC(1999, 11, 31)).getUTCMonth()"), "11");
    // Each month's first and last day, so an off-by-one anywhere in the table is a failure rather
    // than a shift that two neighbouring rows would hide.
    for (month, last) in [
        (0, 31),
        (1, 28),
        (2, 31),
        (3, 30),
        (4, 31),
        (5, 30),
        (6, 31),
        (7, 31),
        (8, 30),
        (9, 31),
        (10, 30),
        (11, 31),
    ] {
        assert_eq!(
            run(&format!(
                "new Date(Date.UTC(2001, {month}, {last})).getUTCMonth()"
            )),
            month.to_string(),
            "last day of month {month}"
        );
        assert_eq!(
            run(&format!(
                "new Date(Date.UTC(2001, {month}, {})).getUTCMonth()",
                last + 1
            )),
            ((month + 1) % 12).to_string(),
            "one day past month {month}"
        );
    }
}

#[test]
fn a_time_value_has_a_range_and_outside_it_there_is_no_date() {
    // §21.4.1.1 — 8.64e15 exactly is a date; one millisecond further is not.
    assert_eq!(run("new Date(8.64e15).getTime()"), "8640000000000000");
    assert_eq!(run("new Date(-8.64e15).getTime()"), "-8640000000000000");
    assert_eq!(run("new Date(8.64e15 + 1).getTime().toString()"), "NaN");
    assert_eq!(run("new Date(-8.64e15 - 1).getTime().toString()"), "NaN");
    assert_eq!(run("new Date(Infinity).getTime().toString()"), "NaN");
    assert_eq!(run("new Date(NaN).getTime().toString()"), "NaN");
    // §21.4.1.31 truncates rather than rounding, so a fractional millisecond is dropped toward zero
    // from both directions.
    assert_eq!(run("new Date(1.9).getTime()"), "1");
    assert_eq!(run("new Date(-1.9).getTime()"), "-1");
    // …and the epoch never remembers a sign.
    assert_eq!(run("1 / new Date(-0).getTime()"), "Infinity");
}

#[test]
fn the_constructor_reads_its_arguments_four_different_ways() {
    // Called rather than constructed, it answers *text* and ignores everything it was given —
    // the one constructor in the language that does.
    assert_eq!(run("typeof Date()"), "string");
    assert_eq!(run("typeof Date(0)"), "string");
    assert_eq!(run("typeof new Date()"), "object");
    // One argument: a number is a time value…
    assert_eq!(run("new Date(86400000).getUTCDate()"), "2");
    // …a string is parsed…
    assert_eq!(run("new Date('1970-01-02').getUTCDate()"), "2");
    // …and a Date is *copied*, not converted, so no `valueOf` gets a say.
    assert_eq!(
        run(
            "(function () { var d = new Date(5); d.valueOf = function () { return 99; }; \
             return new Date(d).getTime(); })()"
        ),
        "5"
    );
    // An object that is not a Date goes through ToPrimitive, and a string result is parsed rather
    // than being run through ToNumber — which is the difference this row exists for.
    assert_eq!(
        run("new Date({toString: function () { return '1970-01-03'; }}).getUTCDate()"),
        "3"
    );
    assert_eq!(
        run("new Date({valueOf: function () { return 0; }}).getTime()"),
        "0"
    );
    // Two or more arguments are fields, and a missing day of the month is the 1st because there is
    // no zeroth — every other field counts from zero and defaults to it.
    assert_eq!(run("new Date(2000, 0).getUTCDate()"), "1");
    assert_eq!(run("new Date(2000, 0).getUTCHours()"), "0");
    assert_eq!(run("new Date(1970, 0, 1, 0, 0, 0, 0).getTime()"), "0");
    assert_eq!(run("new Date(1970, 0, 1, 1, 2, 3, 4).getTime()"), "3723004");
    // A month past December carries into the next year, and a negative one carries back.
    assert_eq!(run("new Date(2000, 12, 1).getUTCFullYear()"), "2001");
    assert_eq!(run("new Date(2000, -1, 1).getUTCFullYear()"), "1999");
    assert_eq!(run("new Date(2000, -1, 1).getUTCMonth()"), "11");
    // §21.4.2.1 step 5.h — a year of 0 through 99 means 1900 through 1999, and *only* in the field
    // form. A parsed `'0099'` is the year 99.
    assert_eq!(run("new Date(99, 0, 1).getUTCFullYear()"), "1999");
    assert_eq!(run("new Date(0, 0, 1).getUTCFullYear()"), "1900");
    assert_eq!(run("new Date(100, 0, 1).getUTCFullYear()"), "100");
    assert_eq!(run("new Date(-1, 0, 1).getUTCFullYear()"), "-1");
    assert_eq!(run("new Date('0099-01-01').getUTCFullYear()"), "99");
    // A field that is NaN makes the whole date invalid.
    assert_eq!(run("new Date(2000, NaN).getTime().toString()"), "NaN");
    assert_eq!(run("new Date(Infinity, 0).getTime().toString()"), "NaN");
}

#[test]
fn the_statics_are_three_and_they_differ_in_what_they_read() {
    assert_eq!(run("typeof Date.now()"), "number");
    assert_eq!(run("Date.now() > 1600000000000"), "true");
    // `Date.UTC` reads the field form as UTC, which is the whole of what it is for…
    assert_eq!(run("Date.UTC(1970, 0, 1)"), "0");
    assert_eq!(run("Date.UTC(2024, 1, 29)"), "1709164800000");
    assert_eq!(run("Date.UTC(99, 0, 1) === Date.UTC(1999, 0, 1)"), "true");
    // …and with nothing at all it is NaN rather than the epoch, unlike the constructor.
    assert_eq!(run("Date.UTC().toString()"), "NaN");
    assert_eq!(run("Date.parse('1970-01-01T00:00:00.000Z')"), "0");
    assert_eq!(run("Date.parse('nonsense').toString()"), "NaN");
    assert_eq!(run("Date.length"), "7");
    assert_eq!(run("Date.now.length"), "0");
    assert_eq!(run("Date.parse.length"), "1");
    assert_eq!(run("Date.UTC.length"), "7");
}

#[test]
fn the_prototype_is_not_itself_a_date() {
    // §21.4.4 — ES5 made `Date.prototype` a Date holding NaN and ES2015 changed it, so this throws
    // rather than answering NaN. The difference is observable exactly here.
    for method in [
        "getTime",
        "valueOf",
        "getUTCFullYear",
        "toISOString",
        "toString",
        "setTime",
    ] {
        assert_eq!(
            run(&format!(
                "(function () {{ try {{ Date.prototype.{method}(); return 'ok'; }} \
                 catch (e) {{ return e.constructor.name; }} }})()"
            )),
            "TypeError",
            "Date.prototype.{method}"
        );
    }
    // …and so does any other receiver without the slot, which is a different failure from a Date
    // whose value happens to be NaN.
    assert_eq!(
        run(
            "(function () { try { Date.prototype.getTime.call({}); return 'ok'; } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "TypeError"
    );
    assert_eq!(run("new Date(NaN).getTime().toString()"), "NaN");
}

#[test]
fn every_field_of_an_invalid_date_is_nan_and_its_text_says_so() {
    for method in [
        "getFullYear",
        "getMonth",
        "getDate",
        "getDay",
        "getHours",
        "getMinutes",
        "getSeconds",
        "getMilliseconds",
        "getUTCFullYear",
        "getTimezoneOffset",
        "getYear",
    ] {
        assert_eq!(
            run(&format!("new Date(NaN).{method}().toString()")),
            "NaN",
            "{method} of an invalid Date"
        );
    }
    assert_eq!(run("new Date(NaN).toString()"), "Invalid Date");
    assert_eq!(run("new Date(NaN).toDateString()"), "Invalid Date");
    assert_eq!(run("new Date(NaN).toTimeString()"), "Invalid Date");
    assert_eq!(run("new Date(NaN).toUTCString()"), "Invalid Date");
    assert_eq!(run("new Date(NaN).toLocaleString()"), "Invalid Date");
    // §21.4.4.36 is the one text form that refuses: ISO 8601 has no spelling for an instant that is
    // not one, and text that did not parse back would break the round trip.
    assert_eq!(
        run(
            "(function () { try { new Date(NaN).toISOString(); return 'ok'; } \
             catch (e) { return e.constructor.name; } })()"
        ),
        "RangeError"
    );
    // …but `toJSON` answers `null` rather than throwing, which is what lets `JSON.stringify` of an
    // invalid Date produce JSON at all.
    assert_eq!(run("new Date(NaN).toJSON() === null"), "true");
    assert_eq!(run("JSON.stringify({d: new Date(NaN)})"), "{\"d\":null}");
}

#[test]
fn the_text_forms_are_four_shapes_and_each_parses_back() {
    assert_eq!(
        run("new Date(0).toString()"),
        "Thu Jan 01 1970 00:00:00 GMT+0000 (Coordinated Universal Time)"
    );
    assert_eq!(run("new Date(0).toDateString()"), "Thu Jan 01 1970");
    assert_eq!(
        run("new Date(0).toTimeString()"),
        "00:00:00 GMT+0000 (Coordinated Universal Time)"
    );
    assert_eq!(
        run("new Date(0).toUTCString()"),
        "Thu, 01 Jan 1970 00:00:00 GMT"
    );
    assert_eq!(run("new Date(0).toISOString()"), "1970-01-01T00:00:00.000Z");
    assert_eq!(run("new Date(0).toJSON()"), "1970-01-01T00:00:00.000Z");
    // Annex B's alias is the same function rather than a copy of it.
    assert_eq!(
        run("new Date(0).toGMTString === new Date(0).toUTCString"),
        "true"
    );
    // §21.4.3.2 obliges `parse` to read back what these wrote. The two text forms carry no
    // milliseconds, so the round trip is to the second — which is why the value is chosen with
    // some.
    assert_eq!(run("Date.parse(new Date(12345678).toString())"), "12345000");
    assert_eq!(
        run("Date.parse(new Date(12345678).toUTCString())"),
        "12345000"
    );
    assert_eq!(
        run("Date.parse(new Date(12345678).toISOString())"),
        "12345678"
    );
    // A negative year is written with a sign rather than padded into something unreadable, and it
    // still reads back.
    assert_eq!(run("new Date(-62198755200000).getUTCFullYear()"), "-1");
    assert_eq!(
        run("Date.parse(new Date(-62198755200000).toUTCString())"),
        "-62198755200000"
    );
    // A year the four-digit ISO field cannot hold takes six digits and an explicit sign.
    assert_eq!(
        run("new Date(8.64e15).toISOString()"),
        "+275760-09-13T00:00:00.000Z"
    );
    assert_eq!(
        run("new Date(-8.64e15).toISOString()"),
        "-271821-04-20T00:00:00.000Z"
    );
    assert_eq!(
        run("Date.parse(new Date(8.64e15).toISOString())"),
        "8640000000000000"
    );
    assert_eq!(
        run("new Date(-62167219200000).toISOString()"),
        "0000-01-01T00:00:00.000Z"
    );
    // Zero padding in every field, which is what makes the widths fixed.
    assert_eq!(
        run("new Date(Date.UTC(2001, 1, 3, 4, 5, 6, 7)).toISOString()"),
        "2001-02-03T04:05:06.007Z"
    );
}

#[test]
fn the_reader_is_strict_and_answers_nan_rather_than_guessing() {
    // The forms §21.4.1.32 defines: a date alone, and a date with a time, to each precision.
    assert_eq!(run("Date.parse('1970-01-01')"), "0");
    assert_eq!(run("Date.parse('1970-01')"), "0");
    assert_eq!(run("Date.parse('1970')"), "0");
    assert_eq!(run("Date.parse('1970-01-01T00:00')"), "0");
    assert_eq!(run("Date.parse('1970-01-01T00:00:00')"), "0");
    assert_eq!(run("Date.parse('1970-01-01T00:00:00.000')"), "0");
    assert_eq!(run("Date.parse('1970-01-01T00:00:00.001Z')"), "1");
    // An offset shifts the instant, and the sign means what it says.
    assert_eq!(run("Date.parse('1970-01-01T01:00:00+01:00')"), "0");
    assert_eq!(run("Date.parse('1970-01-01T00:00:00-01:00')"), "3600000");
    assert_eq!(run("Date.parse('1970-01-01T01:00:00+0100')"), "0");
    // An expanded year carries a sign and six digits.
    assert_eq!(run("Date.parse('+001970-01-01')"), "0");
    assert_eq!(run("Date.parse('-000001-01-01') < 0"), "true");
    // …and `-000000` is the one expanded year the grammar refuses, because it would be a second
    // spelling of 0000.
    assert_eq!(run("Date.parse('-000000-01-01').toString()"), "NaN");
    // Everything a lenient reader would accept and this one does not. Each is its own row because
    // each is a separate place a guess could creep in.
    for text in [
        "",                         // nothing at all
        "nonsense",                 // not a date in any format
        "1970-1-1",                 // a month that is not two digits
        "197-01-01",                // a year that is not four
        "1970-13-01",               // a month past December, which ISO does not carry
        "1970-00-01",               // …nor a zeroth month
        "1970-01-32",               // a day past any month
        "1970-01-00",               // …nor a zeroth day
        "1970-01-01T25:00",         // an hour past 24
        "1970-01-01T00:60",         // a minute past 59
        "1970-01-01T00:00:60",      // a second past 59
        "1970-01-01T00",            // an hour with no minutes
        "1970-01-01 00:00",         // a space where the `T` belongs
        "1970-01-01T00:00:00.0000", // four fractional digits
        "1970-01-01T00:00:00Q",     // rubbish where the zone belongs
        "1970-01-01T00:00:00+1",    // an offset that is too short
        "1970-01-01extra",          // trailing rubbish after a complete date
    ] {
        assert_eq!(
            run(&format!("Date.parse('{text}').toString()")),
            "NaN",
            "parsing {text:?}"
        );
    }
}

#[test]
fn a_setter_writes_the_receiver_and_answers_the_new_time() {
    assert_eq!(
        run(
            "(function () { var d = new Date(0); var r = d.setTime(5); return r + ',' + d.getTime(); })()"
        ),
        "5,5"
    );
    // Each setter leaves the fields it was not given alone, which is what makes them composable.
    assert_eq!(
        run(
            "(function () { var d = new Date(Date.UTC(2000, 5, 15, 12, 30, 45, 500)); \
             d.setUTCMilliseconds(1); return d.toISOString(); })()"
        ),
        "2000-06-15T12:30:45.001Z"
    );
    assert_eq!(
        run(
            "(function () { var d = new Date(Date.UTC(2000, 5, 15, 12, 30, 45, 500)); \
             d.setUTCSeconds(1); return d.toISOString(); })()"
        ),
        "2000-06-15T12:30:01.500Z"
    );
    // …and the optional trailing arguments overwrite the finer fields when they are given.
    assert_eq!(
        run(
            "(function () { var d = new Date(Date.UTC(2000, 5, 15, 12, 30, 45, 500)); \
             d.setUTCSeconds(1, 2); return d.toISOString(); })()"
        ),
        "2000-06-15T12:30:01.002Z"
    );
    assert_eq!(
        run(
            "(function () { var d = new Date(Date.UTC(2000, 5, 15, 12, 30, 45, 500)); \
             d.setUTCMinutes(1, 2, 3); return d.toISOString(); })()"
        ),
        "2000-06-15T12:01:02.003Z"
    );
    assert_eq!(
        run(
            "(function () { var d = new Date(Date.UTC(2000, 5, 15, 12, 30, 45, 500)); \
             d.setUTCHours(1, 2, 3, 4); return d.toISOString(); })()"
        ),
        "2000-06-15T01:02:03.004Z"
    );
    assert_eq!(
        run(
            "(function () { var d = new Date(Date.UTC(2000, 5, 15)); d.setUTCDate(20); \
             return d.toISOString(); })()"
        ),
        "2000-06-20T00:00:00.000Z"
    );
    assert_eq!(
        run(
            "(function () { var d = new Date(Date.UTC(2000, 5, 15)); d.setUTCMonth(0); \
             return d.toISOString(); })()"
        ),
        "2000-01-15T00:00:00.000Z"
    );
    assert_eq!(
        run(
            "(function () { var d = new Date(Date.UTC(2000, 5, 15)); d.setUTCMonth(0, 2); \
             return d.toISOString(); })()"
        ),
        "2000-01-02T00:00:00.000Z"
    );
    assert_eq!(
        run(
            "(function () { var d = new Date(Date.UTC(2000, 5, 15)); d.setUTCFullYear(1999); \
             return d.toISOString(); })()"
        ),
        "1999-06-15T00:00:00.000Z"
    );
    assert_eq!(
        run(
            "(function () { var d = new Date(Date.UTC(2000, 5, 15)); d.setUTCFullYear(1999, 0, 2); \
             return d.toISOString(); })()"
        ),
        "1999-01-02T00:00:00.000Z"
    );
    // A field out of range carries, exactly as it does in the constructor.
    assert_eq!(
        run(
            "(function () { var d = new Date(Date.UTC(2000, 0, 31)); d.setUTCDate(32); \
             return d.getUTCMonth(); })()"
        ),
        "1"
    );
    // §21.4.4.21 step 5 — `setFullYear` is the one setter that revives an invalid Date, because a
    // NaN time value becomes `+0` rather than staying NaN.
    assert_eq!(
        run(
            "(function () { var d = new Date(NaN); d.setUTCFullYear(2000); \
             return d.toISOString(); })()"
        ),
        "2000-01-01T00:00:00.000Z"
    );
    // Every other setter leaves it invalid.
    assert_eq!(
        run(
            "(function () { var d = new Date(NaN); var r = d.setUTCMonth(1); \
             return r.toString() + ',' + d.getTime().toString(); })()"
        ),
        "NaN,NaN"
    );
    assert_eq!(
        run(
            "(function () { var d = new Date(NaN); d.setUTCHours(1); return d.getTime().toString(); })()"
        ),
        "NaN"
    );
    // A setter that pushes past the range invalidates the Date rather than clamping into it.
    assert_eq!(
        run(
            "(function () { var d = new Date(0); d.setUTCFullYear(300000); \
             return d.getTime().toString(); })()"
        ),
        "NaN"
    );
    assert_eq!(run("new Date(0).setTime.length"), "1");
    assert_eq!(run("new Date(0).setUTCHours.length"), "4");
    assert_eq!(run("new Date(0).setUTCFullYear.length"), "3");
}

#[test]
fn a_setter_converts_every_argument_before_it_looks_at_the_time() {
    // §21.4.4.23's order is observable: the conversion runs even when the answer is already known
    // to be NaN, so a `valueOf` that throws is propagated rather than skipped.
    assert_eq!(
        run("(function () { var d = new Date(NaN); \
             try { d.setUTCMonth({valueOf: function () { throw new TypeError('x'); }}); return 'ok'; } \
             catch (e) { return e.constructor.name; } })()"),
        "TypeError"
    );
    // …and the arguments are converted left to right, which a `valueOf` recording its turn can see.
    assert_eq!(
        run("(function () { var order = []; var mark = function (n) { \
               return {valueOf: function () { order.push(n); return 0; }}; }; \
             new Date(0).setUTCHours(mark(1), mark(2), mark(3), mark(4)); return order.join(''); })()"),
        "1234"
    );
    // The receiver is checked before any argument is converted, so a bad receiver throws a
    // TypeError about the receiver rather than running the argument's side effect.
    assert_eq!(
        run("(function () { var ran = false; \
             try { Date.prototype.setUTCMonth.call({}, {valueOf: function () { ran = true; return 0; }}); } \
             catch (e) {} return ran; })()"),
        "false"
    );
}

#[test]
fn annex_b_keeps_the_two_that_count_from_1900() {
    assert_eq!(run("new Date(0).getYear()"), "70");
    assert_eq!(run("new Date(Date.UTC(2000, 0, 1)).getYear()"), "100");
    assert_eq!(
        run("(function () { var d = new Date(0); d.setYear(99); return d.getUTCFullYear(); })()"),
        "1999"
    );
    assert_eq!(
        run("(function () { var d = new Date(0); d.setYear(2000); return d.getUTCFullYear(); })()"),
        "2000"
    );
    assert_eq!(
        run(
            "(function () { var d = new Date(0); d.setYear(NaN); return d.getTime().toString(); })()"
        ),
        "NaN"
    );
}

#[test]
fn a_date_is_an_ordinary_object_that_remembers_an_instant() {
    // The slot is not a property, so nothing enumerates it and a Date looks empty.
    assert_eq!(run("Object.keys(new Date(0)).length"), "0");
    assert_eq!(
        run("JSON.stringify(Object.getOwnPropertyNames(new Date(0)))"),
        "[]"
    );
    // It takes properties like anything else.
    assert_eq!(
        run("(function () { var d = new Date(0); d.x = 1; return d.x; })()"),
        "1"
    );
    assert_eq!(run("new Date(0) instanceof Date"), "true");
    assert_eq!(
        run("Object.prototype.toString.call(new Date(0))"),
        "[object Date]"
    );
    assert_eq!(run("Date.prototype.constructor === Date"), "true");
    assert_eq!(run("new Date(0).constructor === Date"), "true");
    // §21.4.4's methods are writable and configurable but not enumerable, like every other built-in.
    assert_eq!(
        run(
            "(function () { var d = Object.getOwnPropertyDescriptor(Date.prototype, 'getTime'); \
             return d.writable + ',' + d.enumerable + ',' + d.configurable; })()"
        ),
        "true,false,true"
    );
    // §21.4.3.1 — `Date.prototype` cannot be moved.
    assert_eq!(
        run(
            "(function () { var d = Object.getOwnPropertyDescriptor(Date, 'prototype'); \
             return d.writable + ',' + d.enumerable + ',' + d.configurable; })()"
        ),
        "false,false,false"
    );
    // `getTime` and `valueOf` are the same operation but not the same function object.
    assert_eq!(run("typeof new Date(0).valueOf"), "function");
}

#[test]
fn a_host_offset_is_the_only_thing_that_separates_local_time_from_utc() {
    // DR-0014 defaults the offset to zero, which makes every local getter agree with its UTC twin —
    // and makes the two halves of §21.4.4 indistinguishable by any script. So this test supplies an
    // offset, because without one there is no observation that tells `getHours` from `getUTCHours`
    // and half of this file would be untested logic that merely looked covered.
    crate::set_local_offset(3_600_000.0 + 1_800_000.0); // UTC+01:30
    assert_eq!(crate::local_offset(), 5_400_000.0);

    // Every getter pair, on an instant chosen so the offset crosses a day, a month and a year.
    assert_eq!(run("new Date(0).getUTCHours()"), "0");
    assert_eq!(run("new Date(0).getHours()"), "1");
    assert_eq!(run("new Date(0).getUTCMinutes()"), "0");
    assert_eq!(run("new Date(0).getMinutes()"), "30");
    assert_eq!(run("new Date(0).getUTCDate()"), "1");
    assert_eq!(run("new Date(-5400000).getDate()"), "1");
    assert_eq!(run("new Date(-5400000).getUTCDate()"), "31");
    assert_eq!(run("new Date(-5400000).getUTCFullYear()"), "1969");
    assert_eq!(run("new Date(-5400000).getFullYear()"), "1970");
    assert_eq!(run("new Date(-5400000).getUTCMonth()"), "11");
    assert_eq!(run("new Date(-5400000).getMonth()"), "0");
    assert_eq!(run("new Date(-5400000).getUTCDay()"), "3");
    assert_eq!(run("new Date(-5400000).getDay()"), "4");
    assert_eq!(run("new Date(-5400000).getYear()"), "70");
    assert_eq!(run("new Date(-5400000).getUTCFullYear() - 1900"), "69");
    // Seconds and milliseconds are below the offset's resolution, so those two pairs agree even
    // here — asserted rather than omitted, because agreeing is the claim.
    assert_eq!(run("new Date(1234).getSeconds()"), "1");
    assert_eq!(run("new Date(1234).getUTCSeconds()"), "1");
    assert_eq!(run("new Date(1234).getMilliseconds()"), "234");

    // §21.4.4.11 — in minutes, and with the sign the other way round: a zone *ahead* of UTC reports
    // a negative number. Both the subtraction and the division are load-bearing.
    assert_eq!(run("new Date(0).getTimezoneOffset()"), "-90");

    // The field form of the constructor reads local time, so the same fields now name a different
    // instant, while `Date.UTC` is unmoved. That difference is the whole point of the two.
    assert_eq!(run("new Date(1970, 0, 1).getTime()"), "-5400000");
    assert_eq!(run("Date.UTC(1970, 0, 1)"), "0");
    // …and text without an offset is local, while a bare date is UTC.
    assert_eq!(run("Date.parse('1970-01-01T00:00:00')"), "-5400000");
    assert_eq!(run("Date.parse('1970-01-01')"), "0");
    assert_eq!(run("Date.parse('1970-01-01T00:00:00Z')"), "0");
    // The text forms name the offset they were written in.
    assert_eq!(run("new Date(0).toISOString()"), "1970-01-01T00:00:00.000Z");
    assert_eq!(
        run("new Date(0).toUTCString()"),
        "Thu, 01 Jan 1970 00:00:00 GMT"
    );
    assert_eq!(
        run("new Date(0).getHours() + ':' + new Date(0).getMinutes()"),
        "1:30"
    );

    // Every setter pair: the local one lands on a different instant from its UTC twin.
    assert_eq!(
        run("(function () { var d = new Date(0); d.setHours(0); return d.getTime(); })()"),
        // The local minutes stay at 30, so this is UTC-01:00 and not UTC-01:30.
        "-3600000"
    );
    assert_eq!(
        run("(function () { var d = new Date(0); d.setUTCHours(0); return d.getTime(); })()"),
        "0"
    );
    assert_eq!(
        run("(function () { var d = new Date(0); d.setMinutes(0); return d.getTime(); })()"),
        "-1800000"
    );
    assert_eq!(
        run("(function () { var d = new Date(0); d.setUTCMinutes(0); return d.getTime(); })()"),
        "0"
    );
    assert_eq!(
        run("(function () { var d = new Date(0); d.setSeconds(0); return d.getTime(); })()"),
        "0"
    );
    assert_eq!(
        run("(function () { var d = new Date(0); d.setMilliseconds(0); return d.getTime(); })()"),
        "0"
    );
    // A date-valued setter keeps the local wall clock, which moves the instant by the offset.
    assert_eq!(
        run("(function () { var d = new Date(0); d.setDate(1); return d.getTime(); })()"),
        "0"
    );
    assert_eq!(
        run("(function () { var d = new Date(0); d.setDate(2); return d.getTime(); })()"),
        "86400000"
    );
    assert_eq!(
        run("(function () { var d = new Date(0); d.setUTCDate(2); return d.getTime(); })()"),
        "86400000"
    );
    assert_eq!(
        run("(function () { var d = new Date(0); d.setMonth(1); return d.getTime(); })()"),
        "2678400000"
    );
    assert_eq!(
        run("(function () { var d = new Date(0); d.setMonth(1, 2); return d.getTime(); })()"),
        "2764800000"
    );
    assert_eq!(
        run("(function () { var d = new Date(0); d.setUTCMonth(1); return d.getTime(); })()"),
        "2678400000"
    );
    assert_eq!(
        run("(function () { var d = new Date(0); d.setUTCMonth(1, 2); return d.getTime(); })()"),
        "2764800000"
    );
    assert_eq!(
        run("(function () { var d = new Date(0); d.setFullYear(1971); return d.getTime(); })()"),
        "31536000000"
    );
    assert_eq!(
        run(
            "(function () { var d = new Date(0); d.setFullYear(1971, 1, 2); return d.getTime(); })()"
        ),
        "34300800000"
    );
    assert_eq!(
        run("(function () { var d = new Date(0); d.setUTCFullYear(1971); return d.getTime(); })()"),
        "31536000000"
    );
    // §21.4.4.21 step 5 revives an invalid Date to the *epoch*, then applies the fields in local
    // time — so the result carries the offset, which is how the local branch of that step is seen.
    assert_eq!(
        run("(function () { var d = new Date(NaN); d.setFullYear(1970); return d.getTime(); })()"),
        "-5400000"
    );
    assert_eq!(
        run(
            "(function () { var d = new Date(NaN); d.setUTCFullYear(1970); return d.getTime(); })()"
        ),
        "0"
    );
    assert_eq!(
        run("(function () { var d = new Date(0); d.setYear(70); return d.getTime(); })()"),
        // The local time-within-day is carried whole, so shifting back to UTC cancels the offset.
        "0"
    );
    // toString and toDateString read the local clock, toUTCString does not.
    assert_eq!(run("new Date(0).toDateString()"), "Thu Jan 01 1970");
    assert_eq!(
        run("new Date(-5400000).toDateString() + '|' + new Date(-5400000).toUTCString()"),
        "Thu Jan 01 1970|Wed, 31 Dec 1969 22:30:00 GMT"
    );

    // A non-finite offset is refused rather than accepted, because a NaN clock would look broken
    // instead of misconfigured.
    crate::set_local_offset(f64::NAN);
    assert_eq!(crate::local_offset(), 5_400_000.0);
    crate::set_local_offset(f64::INFINITY);
    assert_eq!(crate::local_offset(), 5_400_000.0);
    // A fractional offset is truncated, so the clock stays on whole milliseconds.
    crate::set_local_offset(1.9);
    assert_eq!(crate::local_offset(), 1.0);

    // Put it back, because the offset is thread state and the next test on this thread inherits it.
    crate::set_local_offset(0.0);
    assert_eq!(run("new Date(0).getHours()"), "0");
}

#[test]
fn a_year_of_zero_is_the_boundary_the_sign_sits_on() {
    // §21.4.4.41.1 gives the year a sign only when it is *below* zero, so year 0 carries none. That
    // boundary is the whole content of this test, and no other year can show it: 1 and -1 agree
    // whichever way the comparison is written.
    assert_eq!(
        run("new Date(-62167219200000).toDateString()"),
        "Sat Jan 01 0000"
    );
    assert_eq!(
        run("new Date(-62167219200000).toUTCString()"),
        "Sat, 01 Jan 0000 00:00:00 GMT"
    );
    assert_eq!(
        run("new Date(-62167219200000).toISOString()"),
        "0000-01-01T00:00:00.000Z"
    );
    // …and below it the sign appears, in both padded forms.
    assert_eq!(
        run("new Date(-62198755200000).toDateString()"),
        "Fri Jan 01 -0001"
    );
    assert_eq!(
        run("new Date(-62198755200000).toUTCString()"),
        "Fri, 01 Jan -0001 00:00:00 GMT"
    );
}

#[test]
fn the_year_is_exact_across_the_whole_representable_range() {
    // The closed-form conversion has no correction loop to hide an error in, so a spread of years is
    // a real check on it rather than a spot check. Both extremes, both sides of the epoch, both
    // sides of year zero, and the century and four-century leap boundaries.
    for year in [
        -271820, -100000, -10000, -1601, -1600, -401, -400, -1, 0, 1, 400, 1600, 1899, 1900, 1901,
        1969, 1970, 1971, 2000, 2100, 9999, 10000, 100000, 275759,
    ] {
        // Built through `setUTCFullYear` rather than `Date.UTC`, because the field form maps a year
        // of 0 through 99 onto 1900 through 1999 and that rule would swallow two of these rows.
        let built = |month: u32, date: u32| {
            format!(
                "(function () {{ var d = new Date(0); \
                 d.setUTCFullYear({year}, {month}, {date}); \
                 return d.getUTCFullYear() + ',' + d.getUTCMonth() + ',' + d.getUTCDate(); }})()"
            )
        };
        assert_eq!(
            run(&built(0, 1)),
            format!("{year},0,1"),
            "January 1st of {year}"
        );
        assert_eq!(
            run(&built(11, 31)),
            format!("{year},11,31"),
            "December 31st of {year}"
        );
    }
    // The two extreme years exist only in part, which is why the spread above stops one short of
    // each: the range ends mid-year, so the 20th of April is a date in -271821 and the 1st of
    // January is not.
    assert_eq!(run("Date.UTC(-271821, 0, 1).toString()"), "NaN");
    assert_eq!(run("Date.UTC(-271821, 3, 20)"), "-8640000000000000");
    assert_eq!(run("Date.UTC(-271821, 3, 19).toString()"), "NaN");
    assert_eq!(run("Date.UTC(275760, 8, 13)"), "8640000000000000");
    assert_eq!(run("Date.UTC(275760, 8, 14).toString()"), "NaN");

    // Eighty consecutive Februaries, each checked against the leap rule computed independently in
    // the script — which is what the two halves of the conversion promise about each other.
    assert_eq!(
        run("(function () { for (var y = 1960; y < 2040; y++) { \
               var d = new Date(Date.UTC(y, 1, 29)); \
               if (d.getUTCFullYear() !== y) return 'year ' + y; \
               var leap = (y % 4 === 0 && y % 100 !== 0) || y % 400 === 0; \
               if (d.getUTCMonth() !== (leap ? 1 : 2)) return 'leap ' + y; } return 'ok'; })()"),
        "ok"
    );
}

#[test]
fn an_offset_of_whole_minutes_cannot_show_what_the_finer_fields_do() {
    // §21.4.1.9 puts no granularity on LocalTZA — it is a count of milliseconds. Every real zone is
    // minute-aligned, and that is exactly why a minute-aligned offset cannot tell `getSeconds` from
    // `getUTCSeconds`: for any such offset the two agree. So this one is not minute-aligned.
    crate::set_local_offset(45_123.0); // +00:00:45.123
    assert_eq!(run("new Date(0).getUTCSeconds()"), "0");
    assert_eq!(run("new Date(0).getSeconds()"), "45");
    assert_eq!(run("new Date(0).getUTCMilliseconds()"), "0");
    assert_eq!(run("new Date(0).getMilliseconds()"), "123");
    // Multiplied back out rather than asserted as a fraction, because the minutes here do not divide
    // evenly and a literal would be pinning float formatting rather than the offset.
    assert_eq!(run("new Date(0).getTimezoneOffset() * 60000"), "-45123");
    // The two setters that reach only those fields now land on different instants.
    assert_eq!(
        run("(function () { var d = new Date(0); d.setSeconds(0); return d.getTime(); })()"),
        "-45000"
    );
    assert_eq!(
        run("(function () { var d = new Date(0); d.setUTCSeconds(0); return d.getTime(); })()"),
        "0"
    );
    assert_eq!(
        run("(function () { var d = new Date(0); d.setMilliseconds(0); return d.getTime(); })()"),
        "-123"
    );
    assert_eq!(
        run(
            "(function () { var d = new Date(0); d.setUTCMilliseconds(0); return d.getTime(); })()"
        ),
        "0"
    );
    crate::set_local_offset(0.0);
}

#[test]
fn a_date_setter_reads_the_clock_its_own_half_belongs_to() {
    // The instant matters as much as the offset: at the epoch the local and UTC clocks fall on the
    // same day, so `setDate` and `setUTCDate` agree there by coincidence. This instant is chosen so
    // that they cannot — UTC is still in December 1969 while local has crossed into January 1970.
    // The results are written as ISO text rather than as time values, because a reader can check
    // "the 2nd of December 1969, still at 22:30" and cannot check -2511000000.
    crate::set_local_offset(5_400_000.0); // +01:30
    assert_eq!(run("new Date(-5400000).getUTCFullYear()"), "1969");
    assert_eq!(run("new Date(-5400000).getFullYear()"), "1970");
    assert_eq!(
        run(
            "(function () { var d = new Date(-5400000); d.setDate(2); return d.toISOString(); })()"
        ),
        "1970-01-01T22:30:00.000Z"
    );
    assert_eq!(
        run(
            "(function () { var d = new Date(-5400000); d.setUTCDate(2); \
             return d.toISOString(); })()"
        ),
        "1969-12-02T22:30:00.000Z"
    );
    assert_eq!(
        run(
            "(function () { var d = new Date(-5400000); d.setMonth(1); return d.toISOString(); })()"
        ),
        "1970-01-31T22:30:00.000Z"
    );
    // February 31st of 1969 carries into March, which is the month setter's carry seen through the
    // UTC clock rather than the local one.
    assert_eq!(
        run(
            "(function () { var d = new Date(-5400000); d.setUTCMonth(1); \
             return d.toISOString(); })()"
        ),
        "1969-03-03T22:30:00.000Z"
    );
    // §21.4.4.21 with exactly two arguments: the day of the month comes off the existing time value
    // rather than from a third argument nobody wrote.
    assert_eq!(
        run(
            "(function () { var d = new Date(Date.UTC(2000, 5, 15)); d.setUTCFullYear(1999, 0); \
             return d.toISOString(); })()"
        ),
        "1999-01-15T00:00:00.000Z"
    );
    crate::set_local_offset(0.0);
}

#[test]
fn the_reader_checks_every_separator_and_every_field_edge() {
    // The largest value each field admits, which a `>=` where the specification writes `>` refuses.
    assert_eq!(run("Date.parse('1970-01-01T00:59')"), "3540000");
    assert_eq!(run("Date.parse('1970-01-01T00:00:59')"), "59000");
    // §21.4.1.32 allows hour 24, which names the midnight ending the day.
    assert_eq!(run("Date.parse('1970-01-01T24:00')"), "86400000");
    assert_eq!(run("Date.parse('1970-01-01T23:59:59.999Z')"), "86399999");
    // A separator that is merely *assumed* rather than checked would read these as `00:00`.
    assert_eq!(run("Date.parse('1970-01-01T00X00').toString()"), "NaN");
    assert_eq!(run("Date.parse('1970-01-01T00:00X00').toString()"), "NaN");
    // An offset's minutes are load-bearing, not decoration.
    assert_eq!(run("Date.parse('1970-01-01T00:00:00+00:30')"), "-1800000");
    assert_eq!(run("Date.parse('1970-01-01T00:00:00-00:30')"), "1800000");
    assert_eq!(run("Date.parse('1970-01-01T00:00:00+0130')"), "-5400000");
}

#[test]
fn the_two_written_forms_are_read_back_field_by_field() {
    // §21.4.3.2 obliges the round trip, and these rows are what each part of the two extra passes is
    // for — the weekday, the offset's sign, its minutes, a year too long for the padded field, and
    // the refusal of anything trailing.
    assert_eq!(
        run("Date.parse('Thu Jan 01 1970 00:00:00 GMT+0130')"),
        "-5400000"
    );
    assert_eq!(
        run("Date.parse('Thu Jan 01 1970 00:00:00 GMT-0100')"),
        "3600000"
    );
    assert_eq!(run("Date.parse('Thu Jan 01 1970 00:00:00 GMT+0000')"), "0");
    // The bracketed zone name is decoration and is accepted; rubbish in the sign's place is not.
    assert_eq!(
        run("Date.parse('Thu Jan 01 1970 00:00:00 GMT+0000 (Coordinated Universal Time)')"),
        "0"
    );
    assert_eq!(
        run("Date.parse('Thu Jan 01 1970 00:00:00 GMTX0000').toString()"),
        "NaN"
    );
    assert_eq!(
        run("Date.parse('Thu Jan 01 1970 00:00:00 GMT+0000X').toString()"),
        "NaN"
    );
    assert_eq!(
        run("Date.parse('Thu, 01 Jan 1970 00:00:00 GMTX').toString()"),
        "NaN"
    );
    assert_eq!(
        run("Date.parse('Thu, 01 Jan 1970 00:00:00 GMT extra').toString()"),
        "NaN"
    );
    // The weekday has to *be* one, even though the instant does not depend on which.
    assert_eq!(
        run("Date.parse('Xxx, 01 Jan 1970 00:00:00 GMT').toString()"),
        "NaN"
    );
    assert_eq!(
        run("Date.parse('Xxx Jan 01 1970 00:00:00 GMT+0000').toString()"),
        "NaN"
    );
    assert_eq!(
        run("Date.parse('Thu, 01 Xxx 1970 00:00:00 GMT').toString()"),
        "NaN"
    );
    // A year of more than four digits, which the padded field writes and the reader has to take.
    assert_eq!(
        run("Date.parse(new Date(8.64e15).toUTCString())"),
        "8640000000000000"
    );
    assert_eq!(
        run("Date.parse(new Date(-8.64e15).toUTCString())"),
        "-8640000000000000"
    );
    assert_eq!(
        run("Date.parse(new Date(8.64e15).toString())"),
        "8640000000000000"
    );
}

#[test]
fn a_setter_on_an_invalid_date_must_not_undo_what_the_conversion_did() {
    // §21.4.4.23 step 6 returns NaN *without writing*, and the step order is what makes that
    // observable: the time value is read before the argument is converted, so a `valueOf` that calls
    // `setTime` leaves the receiver holding a real instant which the setter must then leave alone.
    // Writing NaN over it would look like a no-op and is not one.
    for setter in [
        "setMilliseconds",
        "setUTCMilliseconds",
        "setSeconds",
        "setUTCSeconds",
        "setMinutes",
        "setUTCMinutes",
        "setHours",
        "setUTCHours",
        "setDate",
        "setUTCDate",
        "setMonth",
        "setUTCMonth",
    ] {
        assert_eq!(
            run(&format!(
                "(function () {{ var d = new Date(NaN); var calls = 0; \
                 var arg = {{valueOf: function () {{ calls++; d.setTime(7); return 1; }}}}; \
                 var r = d.{setter}(arg); \
                 return r.toString() + ',' + d.getTime() + ',' + calls; }})()"
            )),
            "NaN,7,1",
            "{setter} on an invalid Date"
        );
    }
    // §21.4.4.21 and §B.2.3.2 are the exceptions that *do* write: a NaN time value becomes the epoch
    // rather than a reason to stop, so what the conversion did is overwritten on purpose.
    assert_eq!(
        run("(function () { var d = new Date(NaN); \
             var arg = {valueOf: function () { d.setTime(7); return 1970; }}; \
             var r = d.setUTCFullYear(arg); \
             return (r === 0) + ',' + d.getTime(); })()"),
        "true,0"
    );
    assert_eq!(
        run("(function () { var d = new Date(NaN); \
             var arg = {valueOf: function () { d.setTime(7); return 70; }}; \
             d.setYear(arg); return d.getTime(); })()"),
        "0"
    );
}
