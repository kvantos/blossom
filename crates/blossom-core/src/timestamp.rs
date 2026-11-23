//! Org timestamps: `<2026-11-21 Sat>`, `[2026-11-21 Sat 14:32]`,
//! `<2026-11-21 Sat 10:00-11:30 +1w -2d>`, `<2026-11-21 Sat>--<2026-11-23 Mon>`.
//!
//! Parsing follows Bloom's `OrgTimestamp.kt`: lenient about the day name (any
//! word, so `Do`, `lun.` and `Пн` all parse), strict about the date and clock
//! values (an impossible date or `25:00` is not a timestamp). Formatting always
//! writes the English three-letter day name, as Emacs does by default.

use std::fmt::{self, Write as _};
use std::ops::Range;

use chrono::{Datelike, Days, Months, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, Timelike};

/// How a repeater moves its timestamp when the task is marked done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepeaterKind {
    /// `+`: one interval from the old date.
    Cumulate,
    /// `++`: whole intervals until the date is in the future.
    CatchUp,
    /// `.+`: one interval from today.
    Restart,
}

impl RepeaterKind {
    fn mark(self) -> &'static str {
        match self {
            RepeaterKind::Cumulate => "+",
            RepeaterKind::CatchUp => "++",
            RepeaterKind::Restart => ".+",
        }
    }
}

/// The unit of a repeater or warning cookie.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeUnit {
    /// `h`
    Hour,
    /// `d`
    Day,
    /// `w`
    Week,
    /// `m`
    Month,
    /// `y`
    Year,
}

impl TimeUnit {
    fn from_byte(b: u8) -> Option<TimeUnit> {
        match b {
            b'h' => Some(TimeUnit::Hour),
            b'd' => Some(TimeUnit::Day),
            b'w' => Some(TimeUnit::Week),
            b'm' => Some(TimeUnit::Month),
            b'y' => Some(TimeUnit::Year),
            _ => None,
        }
    }

    fn letter(self) -> char {
        match self {
            TimeUnit::Hour => 'h',
            TimeUnit::Day => 'd',
            TimeUnit::Week => 'w',
            TimeUnit::Month => 'm',
            TimeUnit::Year => 'y',
        }
    }
}

/// A repeater cookie such as `+1w`, `++2d` or `.+1m`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Repeater {
    /// `+`, `++` or `.+`.
    pub kind: RepeaterKind,
    /// The interval count; zero means "does not repeat" (Emacs).
    pub value: u32,
    /// The interval unit.
    pub unit: TimeUnit,
}

/// A warning (on DEADLINE) or delay (on SCHEDULED) cookie such as `-2d` or `--1w`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Warning {
    /// `--`: on a repeating SCHEDULED, delays only the first occurrence.
    pub first_only: bool,
    /// The count.
    pub value: u32,
    /// The unit.
    pub unit: TimeUnit,
}

impl Warning {
    /// The cookie in whole days, as Emacs's `org-get-wdays` counts it: a week
    /// is 7 days, a month 30.4, a year 365.25, an hour 1/24, rounded down.
    #[must_use]
    pub fn days(&self) -> i64 {
        let n = f64::from(self.value);
        let days = match self.unit {
            TimeUnit::Hour => n / 24.0,
            TimeUnit::Day => n,
            TimeUnit::Week => n * 7.0,
            TimeUnit::Month => n * 30.4,
            TimeUnit::Year => n * 365.25,
        };
        // At most u32::MAX * 365.25, well inside i64.
        #[allow(clippy::cast_possible_truncation)]
        let whole = days.floor() as i64;
        whole
    }
}

/// One org timestamp, possibly a range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timestamp {
    /// `<…>` (active, shows in the agenda) or `[…]` (inactive).
    pub active: bool,
    /// The (start) date.
    pub date: NaiveDate,
    /// Start time, if any.
    pub time: Option<NaiveTime>,
    /// End time: `10:00-11:30` on one day, or the time on a range's closing bracket.
    pub end_time: Option<NaiveTime>,
    /// Repeater cookie.
    pub repeater: Option<Repeater>,
    /// Warning or delay cookie.
    pub warning: Option<Warning>,
    /// End date of a range `<a>--<b>`; `None` for a single timestamp.
    pub range_end: Option<NaiveDate>,
}

impl Timestamp {
    /// A plain timestamp on `date` with nothing else set.
    #[must_use]
    pub fn new(date: NaiveDate, active: bool) -> Timestamp {
        Timestamp {
            active,
            date,
            time: None,
            end_time: None,
            repeater: None,
            warning: None,
            range_end: None,
        }
    }

    /// An inactive `[date time]` stamp for `now`, to the minute, as Emacs
    /// writes for CLOSED and logbook entries.
    #[must_use]
    pub fn inactive_at(now: NaiveDateTime) -> Timestamp {
        Timestamp {
            time: NaiveTime::from_hms_opt(now.hour(), now.minute(), 0),
            ..Timestamp::new(now.date(), false)
        }
    }

    /// Parses `text` (surrounding whitespace ignored) as exactly one timestamp
    /// or range. Returns `None` for anything else.
    #[must_use]
    pub fn parse(text: &str) -> Option<Timestamp> {
        let trimmed = text.trim();
        let (ts, end) = parse_at(trimmed, 0)?;
        (end == trimmed.len()).then_some(ts)
    }

    /// Every timestamp in `text`, in order, with its byte range. `<a>--<b>`
    /// becomes one range timestamp when both halves have the same brackets.
    #[must_use]
    pub fn scan(text: &str) -> Vec<(Range<usize>, Timestamp)> {
        let bytes = text.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if matches!(bytes[i], b'<' | b'[')
                && let Some((ts, end)) = parse_at(text, i)
            {
                out.push((i..end, ts));
                i = end;
            } else {
                i += 1;
            }
        }
        out
    }

    /// Whether this timestamp really repeats: Emacs treats `+0d` as no repeater.
    #[must_use]
    pub fn repeats(&self) -> bool {
        self.repeater.is_some_and(|r| r.value > 0)
    }

    /// The canonical org text, e.g. `<2026-11-21 Sat 10:00-11:30 +1w -2d>`.
    #[must_use]
    pub fn format(&self) -> String {
        let (open, close) = if self.active { ('<', '>') } else { ('[', ']') };
        // A range with times puts the end time on the closing bracket (org's
        // canonical form), not on the opener.
        let split_times = self.range_end.is_some() && self.end_time.is_some();
        let mut s = String::new();
        s.push(open);
        s.push_str(&format_date(self.date));
        if let Some(t) = self.time {
            s.push(' ');
            s.push_str(&format_time(t));
            if let (Some(e), false) = (self.end_time, split_times) {
                s.push('-');
                s.push_str(&format_time(e));
            }
        }
        if let Some(r) = self.repeater {
            let _ = write!(s, " {}{}{}", r.kind.mark(), r.value, r.unit.letter());
        }
        if let Some(w) = self.warning {
            let dash = if w.first_only { "--" } else { "-" };
            let _ = write!(s, " {dash}{}{}", w.value, w.unit.letter());
        }
        s.push(close);
        if let Some(end) = self.range_end {
            s.push_str("--");
            s.push(open);
            s.push_str(&format_date(end));
            if let (Some(e), true) = (self.end_time, split_times) {
                s.push(' ');
                s.push_str(&format_time(e));
            }
            s.push(close);
        }
        s
    }

    /// This timestamp moved to its next occurrence when its task is marked
    /// done at `now`, per Emacs's `org-auto-repeat-maybe`. Unchanged when it
    /// doesn't repeat, or when the result would be out of chrono's range.
    #[must_use]
    pub fn advance(&self, now: NaiveDateTime) -> Timestamp {
        let Some(rep) = self.repeater.filter(|r| r.value > 0) else {
            return self.clone();
        };
        if rep.unit == TimeUnit::Hour {
            // A stamp without a time has no hour to shift (Emacs refuses).
            return match self.time {
                Some(time) => self
                    .advance_hours(rep, time, now)
                    .unwrap_or_else(|| self.clone()),
                None => self.clone(),
            };
        }
        let n = rep.value;
        let today = now.date();
        let new_date = match rep.kind {
            RepeaterKind::Cumulate => add_units(self.date, n, rep.unit),
            RepeaterKind::CatchUp => {
                let mut d = add_units(self.date, n, rep.unit);
                while let Some(day) = d.filter(|day| *day <= today) {
                    d = add_units(day, n, rep.unit);
                }
                d
            }
            RepeaterKind::Restart => add_units(today, n, rep.unit),
        };
        let Some(new_date) = new_date else {
            return self.clone();
        };
        let shift = new_date - self.date;
        Timestamp {
            date: new_date,
            range_end: self.range_end.and_then(|e| e.checked_add_signed(shift)),
            ..self.clone()
        }
    }

    /// Hour repeaters shift the whole date-time, so crossing midnight moves the
    /// date: `+` adds one interval, `++` adds intervals until after `now`, `.+`
    /// restarts from `now`.
    fn advance_hours(
        &self,
        rep: Repeater,
        time: NaiveTime,
        now: NaiveDateTime,
    ) -> Option<Timestamp> {
        let step = TimeDelta::try_hours(i64::from(rep.value))?;
        let start = self.date.and_time(time);
        let new_start = match rep.kind {
            RepeaterKind::Cumulate => start.checked_add_signed(step)?,
            RepeaterKind::CatchUp => {
                let mut t = start.checked_add_signed(step)?;
                while t <= now {
                    t = t.checked_add_signed(step)?;
                }
                t
            }
            RepeaterKind::Restart => now
                .with_second(0)
                .and_then(|n| n.with_nanosecond(0))?
                .checked_add_signed(step)?,
        };
        let delta = new_start - start;
        let new_end = match self.end_time {
            Some(e) => Some(
                self.range_end
                    .unwrap_or(self.date)
                    .and_time(e)
                    .checked_add_signed(delta)?,
            ),
            None => None,
        };
        let range_end = match (self.range_end, new_end) {
            (None, _) => None,
            (Some(_), Some(end)) => Some(end.date()),
            (Some(e), None) => Some(e.checked_add_signed(new_start.date() - self.date)?),
        };
        Some(Timestamp {
            date: new_start.date(),
            time: Some(new_start.time()),
            end_time: new_end.map(|e| e.time()),
            range_end,
            ..self.clone()
        })
    }
}

/// Which days a timestamp falls on, for the agenda (Bloom's `RepeatOccurrence`
/// and `occurrenceCovering`, after Emacs's agenda).
impl Timestamp {
    /// The start date of this timestamp's occurrence that covers `day`, or
    /// `None` when none does. A plain stamp covers its own date (through
    /// `range_end` for a range). A repeating one also covers every later
    /// occurrence projected from `date`; `+`, `++` and `.+` project alike.
    /// A range repeats as a block of the same length.
    #[must_use]
    pub fn occurrence_covering(&self, day: NaiveDate) -> Option<NaiveDate> {
        if day < self.date {
            return None;
        }
        let span = self
            .range_end
            .map_or(0, |e| (e - self.date).num_days().max(0));
        let covers = |start: NaiveDate| (day - start).num_days() <= span;
        let Some(rep) = self.repeater.filter(|r| r.value > 0) else {
            return covers(self.date).then_some(self.date);
        };
        if rep.unit == TimeUnit::Hour {
            return self.hour_occurrence_on(rep.value, day);
        }
        let step = i64::from(rep.value);
        let distance = match rep.unit {
            TimeUnit::Day => (day - self.date).num_days(),
            TimeUnit::Week => (day - self.date).num_days() / 7,
            TimeUnit::Month => months_between(self.date, day),
            TimeUnit::Year => i64::from(day.year() - self.date.year()),
            TimeUnit::Hour => 0,
        };
        // The last occurrence starting on or before `day`: estimate, then walk
        // down (an overflowed month end can put the estimate one step high).
        let mut n = distance / step + 1;
        while n >= 0 {
            let units = u32::try_from(n * step).ok()?;
            let start = add_units(self.date, units, rep.unit)?;
            if start <= day {
                return covers(start).then_some(start);
            }
            n -= 1;
        }
        None
    }

    /// Whether any occurrence touches a day in `from..=to`.
    #[must_use]
    pub fn occurs_between(&self, from: NaiveDate, to: NaiveDate) -> bool {
        if !self.repeats() {
            return self.date <= to && self.range_end.unwrap_or(self.date) >= from;
        }
        let mut day = from.max(self.date);
        while day <= to {
            if self.occurrence_covering(day).is_some() {
                return true;
            }
            let Some(next) = day.succ_opt() else { break };
            day = next;
        }
        false
    }

    /// An hour repeater lands on `day` when one of its starts
    /// `date time + k * hours` falls on it (Emacs's `org-closest-date`).
    fn hour_occurrence_on(&self, hours: u32, day: NaiveDate) -> Option<NaiveDate> {
        let start = self.date.and_time(self.time.unwrap_or(NaiveTime::MIN));
        let day_start = day.and_time(NaiveTime::MIN);
        if day_start <= start {
            return (day == self.date).then_some(day);
        }
        let step = i64::from(hours) * 60;
        let k = ((day_start - start).num_minutes() + step - 1) / step;
        let at = start.checked_add_signed(TimeDelta::try_minutes(k * step)?)?;
        (at.date() == day).then_some(day)
    }
}

/// Whole calendar months from `a`'s month to `b`'s.
fn months_between(a: NaiveDate, b: NaiveDate) -> i64 {
    i64::from(b.year() - a.year()) * 12 + i64::from(b.month()) - i64::from(a.month())
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.format())
    }
}

/// `date` moved by `n` units the way Emacs does it: months and years keep the
/// day of month and let a day the target month lacks overflow into the next
/// (Jan 31 + 1m = Mar 3, Feb 29 + 1y = Mar 1). Hours leave the date as is.
/// `None` when the result is out of range.
#[must_use]
pub fn add_units(date: NaiveDate, n: u32, unit: TimeUnit) -> Option<NaiveDate> {
    let overflow = |months: u32| -> Option<NaiveDate> {
        date.with_day(1)?
            .checked_add_months(Months::new(months))?
            .checked_add_days(Days::new(u64::from(date.day() - 1)))
    };
    match unit {
        TimeUnit::Hour => Some(date),
        TimeUnit::Day => date.checked_add_days(Days::new(u64::from(n))),
        TimeUnit::Week => date.checked_add_days(Days::new(u64::from(n) * 7)),
        TimeUnit::Month => overflow(n),
        TimeUnit::Year => overflow(n.checked_mul(12)?),
    }
}

/// `2026-11-21 Sat`.
fn format_date(d: NaiveDate) -> String {
    format!(
        "{:04}-{:02}-{:02} {}",
        d.year(),
        d.month(),
        d.day(),
        d.weekday()
    )
}

/// `09:05`.
fn format_time(t: NaiveTime) -> String {
    format!("{:02}:{:02}", t.hour(), t.minute())
}

/// Parses a timestamp or range starting exactly at byte `start` of `text`.
/// Returns it and the byte offset just past it.
fn parse_at(text: &str, start: usize) -> Option<(Timestamp, usize)> {
    let (ts, end) = parse_single(text, start)?;
    // `<a>--<b>`: pair with a closing stamp of the same kind.
    if text[end..].starts_with("--")
        && let Some((closer, closer_end)) = parse_single(text, end + 2)
        && closer.active == ts.active
        && ts.range_end.is_none()
    {
        let range = Timestamp {
            range_end: Some(closer.date),
            // The canonical timed range puts the end time on the closer; fall
            // back to an opener's `09:00-12:00` (older form).
            end_time: closer.time.or(ts.end_time),
            ..ts
        };
        return Some((range, closer_end));
    }
    Some((ts, end))
}

/// A byte cursor over one timestamp.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn eat(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// Exactly `n` ASCII digits.
    fn digits(&mut self, n: usize) -> Option<u32> {
        let slice = self.bytes.get(self.pos..self.pos + n)?;
        if !slice.iter().all(u8::is_ascii_digit) {
            return None;
        }
        self.pos += n;
        Some(
            slice
                .iter()
                .fold(0, |acc, d| acc * 10 + u32::from(d - b'0')),
        )
    }

    /// One or more ASCII digits (at most 9, so the value fits a `u32`).
    fn number(&mut self) -> Option<u32> {
        let len = self.bytes[self.pos..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if len == 0 || len > 9 {
            return None;
        }
        self.digits(len)
    }

    /// One or more spaces or tabs.
    fn spaces(&mut self) -> bool {
        let start = self.pos;
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.pos += 1;
        }
        self.pos > start
    }

    /// `H:MM` or `HH:MM`, validated.
    fn time(&mut self) -> Option<NaiveTime> {
        let save = self.pos;
        let hour = self.digits(2).or_else(|| self.digits(1));
        let parsed = hour.and_then(|h| {
            if !self.eat(b':') {
                return None;
            }
            let m = self.digits(2)?;
            NaiveTime::from_hms_opt(h, m, 0)
        });
        if parsed.is_none() {
            self.pos = save;
        }
        parsed
    }
}

/// One `<…>` or `[…]` timestamp starting exactly at `start`.
fn parse_single(text: &str, start: usize) -> Option<(Timestamp, usize)> {
    let mut c = Cursor {
        bytes: text.as_bytes(),
        pos: start,
    };
    let active = match c.peek()? {
        b'<' => true,
        b'[' => false,
        _ => return None,
    };
    c.pos += 1;
    let year = c.digits(4)?;
    c.eat(b'-').then_some(())?;
    let month = c.digits(2)?;
    c.eat(b'-').then_some(())?;
    let day = c.digits(2)?;
    let date = NaiveDate::from_ymd_opt(i32::try_from(year).ok()?, month, day)?;
    let after_date = c.pos;
    // The day name is optional and free-form; like the regex it ports, try
    // with it first and fall back to reading the rest without one.
    if let Some(found) = skip_day_name(&mut c).and_then(|()| parse_tail(&mut c, active, date)) {
        return Some(found);
    }
    c.pos = after_date;
    parse_tail(&mut c, active, date)
}

/// ` word`: a day name is any run of characters other than whitespace,
/// digits and closing brackets.
fn skip_day_name(c: &mut Cursor<'_>) -> Option<()> {
    if !c.spaces() {
        return None;
    }
    let start = c.pos;
    let rest = std::str::from_utf8(&c.bytes[c.pos..]).ok()?;
    let len: usize = rest
        .chars()
        .take_while(|ch| !ch.is_whitespace() && !ch.is_ascii_digit() && *ch != '>' && *ch != ']')
        .map(char::len_utf8)
        .sum();
    c.pos += len;
    (c.pos > start).then_some(())
}

/// Everything after the date (and day name): time, cookies, closing bracket.
fn parse_tail(c: &mut Cursor<'_>, active: bool, date: NaiveDate) -> Option<(Timestamp, usize)> {
    let mut ts = Timestamp::new(date, active);
    let close = if active { b'>' } else { b']' };
    let save = c.pos;
    if c.spaces()
        && let Some(t) = c.time()
    {
        ts.time = Some(t);
        if c.peek() == Some(b'-') {
            c.pos += 1;
            ts.end_time = Some(c.time()?);
        }
    } else {
        c.pos = save;
    }
    // Up to one repeater and one warning cookie, in either order.
    loop {
        let save = c.pos;
        if !c.spaces() {
            break;
        }
        if ts.repeater.is_none()
            && let Some(r) = repeater(c)
        {
            ts.repeater = Some(r);
        } else if ts.warning.is_none()
            && let Some(w) = warning(c)
        {
            ts.warning = Some(w);
        } else {
            c.pos = save;
            break;
        }
    }
    c.spaces();
    c.eat(close).then_some(())?;
    Some((ts, c.pos))
}

/// `+1w`, `++1w` or `.+1w`.
fn repeater(c: &mut Cursor<'_>) -> Option<Repeater> {
    let save = c.pos;
    let kind = if c.eat(b'.') {
        c.eat(b'+').then_some(RepeaterKind::Restart)
    } else if c.eat(b'+') {
        Some(if c.eat(b'+') {
            RepeaterKind::CatchUp
        } else {
            RepeaterKind::Cumulate
        })
    } else {
        None
    };
    let parsed = kind.and_then(|kind| {
        let value = c.number()?;
        let unit = TimeUnit::from_byte(c.peek()?)?;
        c.pos += 1;
        Some(Repeater { kind, value, unit })
    });
    if parsed.is_none() {
        c.pos = save;
    }
    parsed
}

/// `-2d` or `--2d`.
fn warning(c: &mut Cursor<'_>) -> Option<Warning> {
    let save = c.pos;
    let parsed = c.eat(b'-').then_some(()).and_then(|()| {
        let first_only = c.eat(b'-');
        let value = c.number()?;
        let unit = TimeUnit::from_byte(c.peek()?)?;
        c.pos += 1;
        Some(Warning {
            first_only,
            value,
            unit,
        })
    });
    if parsed.is_none() {
        c.pos = save;
    }
    parsed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    fn at(y: i32, m: u32, day: u32, h: u32, min: u32) -> NaiveDateTime {
        d(y, m, day).and_time(t(h, min))
    }

    fn ts(s: &str) -> Timestamp {
        Timestamp::parse(s).unwrap_or_else(|| panic!("{s} should parse"))
    }

    #[test]
    fn parses_every_part() {
        let x = ts("<2026-11-21 Sat 10:00-11:30 +1w -2d>");
        assert!(x.active);
        assert_eq!(x.date, d(2026, 11, 21));
        assert_eq!(x.time, Some(t(10, 0)));
        assert_eq!(x.end_time, Some(t(11, 30)));
        assert_eq!(
            x.repeater,
            Some(Repeater {
                kind: RepeaterKind::Cumulate,
                value: 1,
                unit: TimeUnit::Week
            })
        );
        assert_eq!(
            x.warning,
            Some(Warning {
                first_only: false,
                value: 2,
                unit: TimeUnit::Day
            })
        );
        assert_eq!(x.range_end, None);
    }

    #[test]
    fn day_name_is_optional_and_free_form() {
        for s in [
            "<2026-11-19>",
            "<2026-11-19 Do>",
            "<2026-11-19 jeu.>",
            "<2026-11-19 Чт>",
            "<2026-11-19 木>",
        ] {
            assert_eq!(ts(s).date, d(2026, 11, 19), "{s}");
        }
        assert_eq!(ts("<2026-11-19 9:05>").time, Some(t(9, 5)));
        assert_eq!(
            ts("<2026-11-19 +1d>").repeater.map(|r| r.value),
            Some(1),
            "a repeater is not a day name"
        );
    }

    #[test]
    fn repeater_kinds_and_warnings() {
        let kind = |s| ts(s).repeater.map(|r| r.kind);
        assert_eq!(kind("<2026-01-01 Thu +1d>"), Some(RepeaterKind::Cumulate));
        assert_eq!(kind("<2026-01-01 Thu ++1d>"), Some(RepeaterKind::CatchUp));
        assert_eq!(kind("<2026-01-01 Thu .+1d>"), Some(RepeaterKind::Restart));
        let w = ts("<2026-01-01 Thu --3w>").warning.unwrap();
        assert!(w.first_only);
        assert_eq!((w.value, w.unit), (3, TimeUnit::Week));
        // Either order.
        let x = ts("<2026-01-01 Thu -1d +2m>");
        assert_eq!(x.repeater.map(|r| r.value), Some(2));
        assert_eq!(x.warning.map(|w| w.value), Some(1));
    }

    #[test]
    fn rejects_invalid() {
        for s in [
            "<2026-02-30 Mon>",
            "<2026-11-21 Sat 25:00>",
            "<2026-11-21 Sat 10:99>",
            "<2026-11-21 Sat]",
            "[2026-11-21 Sat>",
            "<26-11-21>",
            "<2026-11-21 Sat +1x>",
            "<2026-11-21 Sat> trailing",
            "",
        ] {
            assert_eq!(Timestamp::parse(s), None, "{s}");
        }
    }

    #[test]
    fn ranges() {
        let r = ts("<2026-11-21 Sat 09:00>--<2026-11-23 Mon 12:00>");
        assert_eq!(r.range_end, Some(d(2026, 11, 23)));
        assert_eq!(r.time, Some(t(9, 0)));
        assert_eq!(r.end_time, Some(t(12, 0)));
        assert_eq!(r.format(), "<2026-11-21 Sat 09:00>--<2026-11-23 Mon 12:00>");
        // Mixed brackets are two stamps, not a range.
        let found = Timestamp::scan("<2026-11-21 Sat>--[2026-11-23 Mon]");
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn format_round_trips_canonical_text() {
        for s in [
            "<2026-11-21 Sat>",
            "[2026-11-21 Sat 14:32]",
            "<2026-11-21 Sat 10:00-11:30 .+1w --2d>",
            "<2026-11-21 Sat>--<2026-11-23 Mon>",
            "<2026-11-21 Sat ++1y>",
        ] {
            assert_eq!(ts(s).format(), s);
        }
        // Non-English and missing day names are rewritten in English.
        assert_eq!(
            ts("<2026-11-19 Do 9:05>").format(),
            "<2026-11-19 Thu 09:05>"
        );
        assert_eq!(ts("<2026-11-19>").to_string(), "<2026-11-19 Thu>");
    }

    #[test]
    fn scan_finds_ranges_in_prose() {
        let text = "see <2026-11-21 Sat> and [2026-11-22 Sun 10:00] or <bad> <2026-11-30 Mon>";
        let found = Timestamp::scan(text);
        let spans: Vec<&str> = found.iter().map(|(r, _)| &text[r.clone()]).collect();
        assert_eq!(
            spans,
            [
                "<2026-11-21 Sat>",
                "[2026-11-22 Sun 10:00]",
                "<2026-11-30 Mon>"
            ]
        );
    }

    #[test]
    fn add_units_overflows_like_emacs() {
        assert_eq!(
            add_units(d(2026, 1, 31), 1, TimeUnit::Month),
            Some(d(2026, 3, 3))
        );
        assert_eq!(
            add_units(d(2028, 2, 29), 1, TimeUnit::Year),
            Some(d(2029, 3, 1))
        );
        assert_eq!(
            add_units(d(2026, 1, 15), 2, TimeUnit::Week),
            Some(d(2026, 1, 29))
        );
        assert_eq!(
            add_units(d(2026, 1, 15), 5, TimeUnit::Hour),
            Some(d(2026, 1, 15))
        );
    }

    #[test]
    fn advance_by_kind() {
        let now = at(2026, 11, 23, 8, 30);
        assert_eq!(ts("<2026-11-01 Sun +1w>").advance(now).date, d(2026, 11, 8));
        assert_eq!(
            ts("<2026-11-01 Sun ++1w>").advance(now).date,
            d(2026, 11, 29)
        );
        assert_eq!(
            ts("<2026-11-01 Sun .+1w>").advance(now).date,
            d(2026, 11, 30)
        );
        // No repeater, or a zero one: unchanged.
        let plain = ts("<2026-11-01 Sun>");
        assert_eq!(plain.advance(now), plain);
        let zero = ts("<2026-11-01 Sun +0d>");
        assert!(!zero.repeats());
        assert_eq!(zero.advance(now), zero);
    }

    #[test]
    fn advance_moves_range_end_with_start() {
        let r = ts("<2026-11-01 Sun +1w>--<2026-11-03 Tue>").advance(at(2026, 11, 2, 0, 0));
        assert_eq!(r.format(), "<2026-11-08 Sun +1w>--<2026-11-10 Tue>");
    }

    #[test]
    fn advance_hours_crosses_midnight() {
        let now = at(2026, 11, 23, 8, 30);
        let x = ts("<2026-11-22 Sun 22:00 +3h>").advance(now);
        assert_eq!(x.format(), "<2026-11-23 Mon 01:00 +3h>");
        let x = ts("<2026-11-22 Sun 22:00 ++3h>").advance(now);
        assert_eq!(x.format(), "<2026-11-23 Mon 10:00 ++3h>");
        let x = ts("<2026-11-22 Sun 22:00-23:00 .+1h>").advance(now);
        assert_eq!(x.format(), "<2026-11-23 Mon 09:30-10:30 .+1h>");
        // No time: nothing to shift.
        let no_time = ts("<2026-11-22 Sun +3h>");
        assert_eq!(no_time.advance(now), no_time);
    }

    #[test]
    fn occurrences() {
        let plain = ts("<2026-11-20 Fri>");
        assert_eq!(
            plain.occurrence_covering(d(2026, 11, 20)),
            Some(d(2026, 11, 20))
        );
        assert_eq!(plain.occurrence_covering(d(2026, 11, 21)), None);
        let range = ts("<2026-11-20 Fri>--<2026-11-22 Sun>");
        assert_eq!(
            range.occurrence_covering(d(2026, 11, 22)),
            Some(d(2026, 11, 20))
        );
        assert_eq!(range.occurrence_covering(d(2026, 11, 23)), None);
        let weekly = ts("<2026-11-20 Fri +1w>");
        assert_eq!(
            weekly.occurrence_covering(d(2026, 12, 4)),
            Some(d(2026, 12, 4))
        );
        assert_eq!(weekly.occurrence_covering(d(2026, 12, 5)), None);
        assert_eq!(
            weekly.occurrence_covering(d(2026, 11, 13)),
            None,
            "never before its date"
        );
        // Month-end overflow: Jan 31 + 1m lands on Mar 3.
        let monthly = ts("<2026-01-31 Sat +1m>");
        assert_eq!(
            monthly.occurrence_covering(d(2026, 3, 3)),
            Some(d(2026, 3, 3))
        );
        assert_eq!(monthly.occurrence_covering(d(2026, 2, 28)), None);
        let hourly = ts("<2026-11-20 Fri 22:00 +36h>");
        assert_eq!(
            hourly.occurrence_covering(d(2026, 11, 22)),
            Some(d(2026, 11, 22))
        );
        assert_eq!(hourly.occurrence_covering(d(2026, 11, 21)), None);
        assert!(weekly.occurs_between(d(2026, 11, 26), d(2026, 11, 27)));
        assert!(!weekly.occurs_between(d(2026, 11, 21), d(2026, 11, 26)));
        assert!(range.occurs_between(d(2026, 11, 22), d(2026, 11, 30)));
    }

    #[test]
    fn inactive_at_drops_seconds() {
        let now = d(2026, 11, 23).and_hms_opt(14, 5, 59).unwrap();
        assert_eq!(
            Timestamp::inactive_at(now).format(),
            "[2026-11-23 Mon 14:05]"
        );
    }
}
