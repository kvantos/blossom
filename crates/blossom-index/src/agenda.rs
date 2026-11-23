//! The agenda, with Bloom's rules (`AgendaBuckets.kt` and
//! `AgendaViewModel.dateGroups`), after the Emacs agenda:
//!
//! - A planned heading belongs to exactly one day: its SCHEDULED date, else
//!   its DEADLINE. One with both shows on the scheduled day. Past ones go to
//!   Overdue instead.
//! - Bare active timestamps are events: one entry per stamp per day it
//!   covers, on every repeat of a repeating one, never overdue. An event of
//!   a heading already planned for that day is dropped.
//! - A repeating SCHEDULED or DEADLINE of an open heading also shows on each
//!   later repeat after today (SCHEDULED winning when both land on a day),
//!   never on a day the heading is already planned for.
//! - An open heading whose DEADLINE is after today but within its warning
//!   window (its own `-Nd`, else the default) is listed as "due soon",
//!   unless it's already on today's list or overdue.
//! - A SCHEDULED delay `-Nd` keeps the plan off the agenda until N days after
//!   its date, repeats included; with `--Nd`, repeats show once the stored
//!   date's own delay is over.

use std::cmp::Ordering;

use blossom_core::{EventStamp, Timestamp};
use chrono::{Days, NaiveDate, NaiveTime};

use crate::record::Note;

/// How to build an agenda.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// The first day.
    pub today: NaiveDate,
    /// How many days, today included (at least 1).
    pub days: u32,
    /// Deadline warning days when a deadline has no `-Nd` of its own
    /// (Emacs's `org-deadline-warning-days`, 14 by default).
    pub warning_days: i64,
    /// Keep headings with a done keyword.
    pub include_done: bool,
}

impl Options {
    /// Today plus `days - 1` days, 14 warning days, open headings only.
    #[must_use]
    pub fn new(today: NaiveDate, days: u32) -> Options {
        Options {
            today,
            days,
            warning_days: 14,
            include_done: false,
        }
    }

    fn horizon(&self) -> NaiveDate {
        self.today
            .checked_add_days(Days::new(u64::from(self.days.max(1) - 1)))
            .unwrap_or(self.today)
    }
}

/// Why an entry is where it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Its SCHEDULED date (or past it, in Overdue).
    Scheduled,
    /// Its DEADLINE date, without a SCHEDULED (or past it, in Overdue).
    Deadline,
    /// A bare active timestamp covering the day.
    Event,
    /// A later repeat of its repeating SCHEDULED.
    ScheduledRepeat,
    /// A later repeat of its repeating DEADLINE.
    DeadlineRepeat,
    /// Its DEADLINE is coming up, within the warning window.
    DueSoon,
}

impl Placement {
    /// A lower-case name for output.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Placement::Scheduled => "scheduled",
            Placement::Deadline => "deadline",
            Placement::Event => "event",
            Placement::ScheduledRepeat => "scheduled-repeat",
            Placement::DeadlineRepeat => "deadline-repeat",
            Placement::DueSoon => "due-soon",
        }
    }
}

/// What checking an entry off does (Bloom's `tasks-and-events.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completion {
    /// A task: mark it done (or repeat it, if anything on it repeats).
    Task,
    /// An event with a repeater: move that one timestamp to its next date.
    Advance(EventStamp),
}

/// One row of the agenda.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The heading.
    pub note: Note,
    /// Why it is here.
    pub placement: Placement,
    /// The timestamp that placed it, as stored in the file.
    pub stamp: Timestamp,
}

impl Entry {
    /// The time shown for it, if timed.
    #[must_use]
    pub fn time(&self) -> Option<NaiveTime> {
        self.stamp.time
    }

    /// Whether this is an event: a heading without a TODO keyword.
    #[must_use]
    pub fn is_event(&self) -> bool {
        self.note.record.keyword.is_none()
    }

    /// What its checkbox does, or `None` for an event with nothing to repeat
    /// (no checkbox).
    #[must_use]
    pub fn completion(&self) -> Option<Completion> {
        if !self.is_event() {
            return Some(Completion::Task);
        }
        let repeats = |t: &Option<Timestamp>| t.as_ref().is_some_and(Timestamp::repeats);
        let n = &self.note;
        let stamp = match self.placement {
            Placement::Event if self.stamp.repeats() => EventStamp::Active(self.stamp.date),
            Placement::ScheduledRepeat => EventStamp::Scheduled,
            Placement::DeadlineRepeat => EventStamp::Deadline,
            _ if repeats(&n.scheduled) => EventStamp::Scheduled,
            _ if repeats(&n.deadline) => EventStamp::Deadline,
            _ => return None,
        };
        Some(Completion::Advance(stamp))
    }
}

/// One day of the agenda.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Day {
    /// The date.
    pub date: NaiveDate,
    /// Timed entries first, by time, then by title.
    pub entries: Vec<Entry>,
}

/// The agenda.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agenda {
    /// Past-dated plans, oldest first.
    pub overdue: Vec<Entry>,
    /// Upcoming deadlines inside their warning window, soonest first.
    pub due_soon: Vec<Entry>,
    /// Today (always, even when empty), then each later day with entries.
    pub days: Vec<Day>,
}

impl Agenda {
    /// How many entries there are in all.
    #[must_use]
    pub fn len(&self) -> usize {
        self.overdue.len()
            + self.due_soon.len()
            + self.days.iter().map(|d| d.entries.len()).sum::<usize>()
    }

    /// Whether there is nothing on it.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The SCHEDULED, else the DEADLINE, with which one it is.
fn plan(n: &Note) -> Option<(&Timestamp, Placement)> {
    n.scheduled
        .as_ref()
        .map(|t| (t, Placement::Scheduled))
        .or_else(|| n.deadline.as_ref().map(|t| (t, Placement::Deadline)))
}

fn plan_date(n: &Note) -> Option<NaiveDate> {
    plan(n).map(|(t, _)| t.date)
}

/// A SCHEDULED delay still holds the plan back on `today`.
fn delayed(n: &Note, today: NaiveDate) -> bool {
    let Some(s) = &n.scheduled else { return false };
    let Some(w) = s.warning else { return false };
    let days = w.days();
    days > 0
        && s.date
            .checked_add_days(Days::new(days.unsigned_abs()))
            .is_some_and(|until| today < until)
}

/// `ts` has a repeat after its stored date starting on `day`.
fn repeats_on(ts: &Timestamp, day: NaiveDate) -> bool {
    ts.repeats() && day > ts.date && ts.occurrence_covering(day) == Some(day)
}

/// A later repeat of an open heading's SCHEDULED (preferred) or DEADLINE
/// on `day`, unless the heading is planned for `day` anyway.
fn repeat_on(n: &Note, day: NaiveDate) -> Option<Entry> {
    if n.record.done || plan_date(n) == Some(day) {
        return None;
    }
    // A single-dash delay holds back every repeat, not just the first.
    let held = n
        .scheduled
        .as_ref()
        .and_then(|s| s.warning)
        .is_some_and(|w| w.days() > 0 && !w.first_only);
    n.scheduled
        .as_ref()
        .filter(|s| !held && repeats_on(s, day))
        .map(|s| entry(n, Placement::ScheduledRepeat, s))
        .or_else(|| {
            n.deadline
                .as_ref()
                .filter(|d| repeats_on(d, day))
                .map(|d| entry(n, Placement::DeadlineRepeat, d))
        })
}

fn same(a: &Note, b: &Note) -> bool {
    a.record.path == b.record.path && a.record.heading == b.record.heading
}

fn priority_key(n: &Note) -> &str {
    n.record.priority.as_deref().unwrap_or("Z")
}

fn by_time(a: &Entry, b: &Entry) -> Ordering {
    let t = |e: &Entry| e.time().unwrap_or(NaiveTime::MIN);
    a.time()
        .is_none()
        .cmp(&b.time().is_none())
        .then(t(a).cmp(&t(b)))
        .then_with(|| {
            a.note
                .record
                .title
                .to_lowercase()
                .cmp(&b.note.record.title.to_lowercase())
        })
}

fn by_date_priority(a: &Entry, b: &Entry, date: impl Fn(&Entry) -> NaiveDate) -> Ordering {
    date(a)
        .cmp(&date(b))
        .then_with(|| priority_key(&a.note).cmp(priority_key(&b.note)))
        .then_with(|| {
            a.note
                .record
                .title
                .to_lowercase()
                .cmp(&b.note.record.title.to_lowercase())
        })
}

fn entry(n: &Note, placement: Placement, stamp: &Timestamp) -> Entry {
    Entry {
        note: n.clone(),
        placement,
        stamp: stamp.clone(),
    }
}

/// Builds the agenda from `notes` (any set of headings; those without dates
/// are ignored).
#[must_use]
pub fn build(notes: &[Note], opts: &Options) -> Agenda {
    let today = opts.today;
    let horizon = opts.horizon();
    let visible: Vec<&Note> = notes
        .iter()
        .filter(|n| opts.include_done || !n.record.done)
        .filter(|n| plan(n).is_some() || !n.events.is_empty())
        .collect();
    let plannable: Vec<&Note> = visible
        .iter()
        .copied()
        .filter(|n| !delayed(n, today))
        .collect();

    let planned_on = |day: NaiveDate| -> Vec<Entry> {
        plannable
            .iter()
            .filter_map(|n| {
                let (t, p) = plan(n)?;
                (t.date == day).then(|| entry(n, p, t))
            })
            .collect()
    };
    let events_on = |day: NaiveDate, taken: &[Entry]| -> Vec<Entry> {
        visible
            .iter()
            .filter(|n| !taken.iter().any(|e| same(&e.note, n)))
            .flat_map(|n| {
                n.events
                    .iter()
                    .filter(move |t| t.occurrence_covering(day).is_some())
                    .map(move |t| entry(n, Placement::Event, t))
            })
            .collect()
    };

    let mut overdue: Vec<Entry> = plannable
        .iter()
        .filter_map(|n| {
            let (t, p) = plan(n)?;
            (t.date < today).then(|| entry(n, p, t))
        })
        .collect();
    overdue.sort_by(|a, b| by_date_priority(a, b, |e| e.stamp.date));

    let mut first = planned_on(today);
    let events = events_on(today, &first);
    first.extend(events);
    first.sort_by(by_time);

    let mut due_soon: Vec<Entry> = visible
        .iter()
        .filter(|n| !n.record.done)
        .filter(|n| !first.iter().chain(&overdue).any(|e| same(&e.note, n)))
        .filter_map(|n| {
            let d = n.deadline.as_ref()?;
            let window = d.warning.map_or(opts.warning_days, |w| w.days());
            (d.date > today && (d.date - today).num_days() <= window)
                .then(|| entry(n, Placement::DueSoon, d))
        })
        .collect();
    due_soon.sort_by(|a, b| by_date_priority(a, b, |e| e.stamp.date));

    let mut days = vec![Day {
        date: today,
        entries: first,
    }];
    let mut day = today;
    while let Some(next) = day.succ_opt().filter(|d| *d <= horizon) {
        day = next;
        let mut entries = planned_on(day);
        entries.extend(plannable.iter().filter_map(|n| repeat_on(n, day)));
        let events = events_on(day, &entries);
        entries.extend(events);
        if !entries.is_empty() {
            entries.sort_by(by_time);
            days.push(Day { date: day, entries });
        }
    }
    Agenda {
        overdue,
        due_soon,
        days,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::records;
    use blossom_core::{Document, Keywords};

    fn notes(text: &str) -> Vec<Note> {
        let doc = Document::parse(text, Keywords::default());
        records("a.org", &doc, &Keywords::default(), 0)
            .into_iter()
            .map(Note::from)
            .collect()
    }

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, d).unwrap()
    }

    /// `(date, title, placement)` rows; `None` dates are Overdue, `Some(0)` Due soon.
    fn rows(a: &Agenda) -> Vec<(u32, String, &'static str)> {
        let mut out = Vec::new();
        for e in &a.overdue {
            out.push((0, e.note.record.title.clone(), e.placement.name()));
        }
        for e in &a.due_soon {
            out.push((1, e.note.record.title.clone(), e.placement.name()));
        }
        for d in &a.days {
            for e in &d.entries {
                out.push((
                    chrono::Datelike::day(&d.date),
                    e.note.record.title.clone(),
                    e.placement.name(),
                ));
            }
        }
        out
    }

    fn r(d: u32, t: &str, p: &'static str) -> (u32, String, &'static str) {
        (d, t.to_string(), p)
    }

    #[test]
    fn one_day_per_plan_events_and_overdue() {
        let text = "\
* TODO both
SCHEDULED: <2026-11-24 Tue> DEADLINE: <2026-11-26 Thu>
* TODO late
SCHEDULED: <2026-11-20 Fri>
* TODO timed
SCHEDULED: <2026-11-23 Mon 09:00>
* party
<2026-11-23 Mon 18:00>--<2026-11-24 Tue 01:00>
* TODO planned and event same day
SCHEDULED: <2026-11-23 Mon>
<2026-11-23 Mon>
* DONE finished
SCHEDULED: <2026-11-23 Mon>
* nothing
";
        let a = build(&notes(text), &Options::new(day(23), 7));
        assert_eq!(
            rows(&a),
            [
                r(0, "late", "scheduled"),
                // Shown on its scheduled day, and as due soon (as Bloom does).
                r(1, "both", "due-soon"),
                r(23, "timed", "scheduled"),
                r(23, "party", "event"),
                r(23, "planned and event same day", "scheduled"),
                // The range's start time sorts it before untimed rows.
                r(24, "party", "event"),
                r(24, "both", "scheduled"),
            ]
        );
        let mut all = Options::new(day(23), 1);
        all.include_done = true;
        assert!(rows(&build(&notes(text), &all)).contains(&r(23, "finished", "scheduled")));
        assert_eq!(build(&[], &Options::new(day(23), 3)).days.len(), 1);
    }

    #[test]
    fn repeats_due_soon_and_delays() {
        let text = "\
* TODO weekly
SCHEDULED: <2026-11-16 Mon +1w>
* TODO report
DEADLINE: <2026-11-27 Fri>
* TODO far report
DEADLINE: <2026-12-20 Sun>
* TODO warned
DEADLINE: <2026-12-20 Sun -30d>
* TODO delayed
SCHEDULED: <2026-11-22 Sun +1w -3d>
* TODO first only
SCHEDULED: <2026-11-22 Sun +1w --3d>
* TODO first only, delay over
SCHEDULED: <2026-11-19 Thu +1w --3d>
* bins
<2026-11-10 Tue +1w>
";
        let a = build(&notes(text), &Options::new(day(23), 9));
        assert_eq!(
            rows(&a),
            [
                r(0, "weekly", "scheduled"),
                r(0, "first only, delay over", "scheduled"),
                r(1, "report", "due-soon"),
                r(1, "warned", "due-soon"),
                r(24, "bins", "event"),
                r(26, "first only, delay over", "scheduled-repeat"),
                r(27, "report", "deadline"),
                r(30, "weekly", "scheduled-repeat"),
                r(1, "bins", "event"),
            ]
        );
        // Checking off an event advances the stamp that placed it.
        let bins = &a.days[1].entries[0];
        assert_eq!(
            bins.completion(),
            Some(Completion::Advance(EventStamp::Active(day(10))))
        );
        assert_eq!(a.due_soon[0].completion(), Some(Completion::Task));
    }

    #[test]
    fn plain_events_have_no_checkbox() {
        let a = build(
            &notes("* meeting\n<2026-11-23 Mon 10:00>\n* gym\nSCHEDULED: <2026-11-23 Mon +2d>\n"),
            &Options::new(day(23), 1),
        );
        let c: Vec<_> = a.days[0].entries.iter().map(Entry::completion).collect();
        assert_eq!(c, [None, Some(Completion::Advance(EventStamp::Scheduled))]);
    }
}
