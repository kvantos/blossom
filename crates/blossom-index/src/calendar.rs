//! The app's agenda for a range of days: one-day items on the calendar and
//! ongoing items on the timeline.
//!
//! - Ongoing: a heading with both SCHEDULED (its start) and DEADLINE (its
//!   end), or an active range `<a>--<b>` over more than one day.
//! - One day: a SCHEDULED or DEADLINE alone, and plain active timestamps
//!   (repeats included). An open plan from before today is carried to
//!   today, marked late, as the Emacs agenda does; a repeating open plan
//!   also shows on its later repeats. Done headings stay on their dates.
//!
//! Unlike [`crate::agenda`], past days and done headings are kept: the
//! calendar shows whole weeks, months and years.

use std::collections::BTreeMap;

use blossom_core::Timestamp;
use chrono::{NaiveDate, NaiveTime};

use crate::record::Note;

/// What put a one-day item on its day.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// SCHEDULED.
    Scheduled,
    /// DEADLINE.
    Deadline,
    /// A plain active timestamp.
    Event,
}

/// A one-day item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// The heading.
    pub note: Note,
    /// Why it is here.
    pub kind: Kind,
    /// The timestamp, as stored.
    pub stamp: Timestamp,
    /// Carried to today from a past date, this many days ago.
    pub late: Option<i64>,
}

impl Item {
    /// The start time, if timed.
    #[must_use]
    pub fn time(&self) -> Option<NaiveTime> {
        self.stamp.time
    }
}

/// An ongoing item on the timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    /// The heading.
    pub note: Note,
    /// The first day.
    pub start: NaiveDate,
    /// The last day (the DEADLINE, or the range's end).
    pub end: NaiveDate,
    /// From a SCHEDULED and DEADLINE pair (else a timestamp range).
    pub planned: bool,
    /// The timestamps that make it, as org text, for the hover card.
    pub org: String,
}

impl Span {
    /// Days past its end on `today`, when still open.
    #[must_use]
    pub fn late(&self, today: NaiveDate) -> Option<i64> {
        (self.planned && !self.note.record.done && self.end < today)
            .then(|| (today - self.end).num_days())
    }
}

/// The agenda for `from..=to`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Calendar {
    /// The first day.
    pub from: NaiveDate,
    /// The last day.
    pub to: NaiveDate,
    /// The day it was built for.
    pub today: NaiveDate,
    /// Ongoing items touching the range (a late one counts up to today),
    /// by file, then start.
    pub ongoing: Vec<Span>,
    /// One-day items by day: timed first by time, then by title.
    pub days: BTreeMap<NaiveDate, Vec<Item>>,
}

impl Calendar {
    /// The same calendar with only the files `keep` accepts.
    #[must_use]
    pub fn only(&self, keep: impl Fn(&str) -> bool) -> Calendar {
        Calendar {
            from: self.from,
            to: self.to,
            today: self.today,
            ongoing: self
                .ongoing
                .iter()
                .filter(|s| keep(&s.note.record.path))
                .cloned()
                .collect(),
            days: self
                .days
                .iter()
                .filter_map(|(d, items)| {
                    let items: Vec<Item> = items
                        .iter()
                        .filter(|i| keep(&i.note.record.path))
                        .cloned()
                        .collect();
                    (!items.is_empty()).then_some((*d, items))
                })
                .collect(),
        }
    }

    /// The items on `day`.
    #[must_use]
    pub fn on(&self, day: NaiveDate) -> &[Item] {
        self.days.get(&day).map_or(&[], Vec::as_slice)
    }
}

/// Every day of `from..=to` on which `t` has an occurrence starting.
fn occurrences(t: &Timestamp, from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
    if !t.repeats() {
        return if (from..=to).contains(&t.date) {
            vec![t.date]
        } else {
            Vec::new()
        };
    }
    from.max(t.date)
        .iter_days()
        .take_while(|d| *d <= to)
        .filter(|d| t.occurrence_covering(*d) == Some(*d))
        .collect()
}

fn same_day_range(t: &Timestamp) -> bool {
    t.range_end.is_none_or(|e| e <= t.date)
}

impl Calendar {
    fn put(&mut self, day: NaiveDate, item: Item) {
        self.days.entry(day).or_default().push(item);
    }

    /// A heading with both SCHEDULED and DEADLINE: a span, if it touches
    /// the range (an open one reaches to today).
    fn add_planned_span(&mut self, n: &Note, s: &Timestamp, d: &Timestamp) {
        let (start, end) = (s.date.min(d.date), s.date.max(d.date));
        let reach = if n.record.done {
            end
        } else {
            end.max(self.today)
        };
        if start <= self.to && reach >= self.from {
            self.ongoing.push(Span {
                note: n.clone(),
                start,
                end,
                planned: true,
                org: format!("SCHEDULED: {} DEADLINE: {}", s.format(), d.format()),
            });
        }
    }

    /// A SCHEDULED or DEADLINE alone: on its day, or carried to today
    /// when open and past, and on its later repeats while open.
    fn add_plan(&mut self, n: &Note, t: &Timestamp, kind: Kind) {
        let (from, to, today) = (self.from, self.to, self.today);
        let item = |late| Item {
            note: n.clone(),
            kind,
            stamp: t.clone(),
            late,
        };
        if n.record.done {
            if (from..=to).contains(&t.date) {
                self.put(t.date, item(None));
            }
        } else if t.date < today {
            if (from..=to).contains(&today) {
                self.put(today, item(Some((today - t.date).num_days())));
            }
            // Its later repeats still come round.
            if t.repeats() {
                let after = today.succ_opt().unwrap_or(today);
                for day in occurrences(t, from.max(after), to) {
                    self.put(day, item(None));
                }
            }
        } else {
            for day in occurrences(t, from, to) {
                self.put(day, item(None));
            }
        }
    }

    /// A plain active timestamp: on each day it occurs, or a span when it
    /// is a range over several days.
    fn add_event(&mut self, n: &Note, t: &Timestamp) {
        if same_day_range(t) {
            for day in occurrences(t, self.from, self.to) {
                self.put(
                    day,
                    Item {
                        note: n.clone(),
                        kind: Kind::Event,
                        stamp: t.clone(),
                        late: None,
                    },
                );
            }
        } else if let Some(end) = t.range_end
            && t.date <= self.to
            && end >= self.from
        {
            self.ongoing.push(Span {
                note: n.clone(),
                start: t.date,
                end,
                planned: false,
                org: t.format(),
            });
        }
    }

    /// One entry per heading and day (its plan wins over its own
    /// timestamp), timed first; spans by file, then start.
    fn tidy(&mut self) {
        for items in self.days.values_mut() {
            let mut seen: Vec<(String, usize)> = Vec::new();
            items.sort_by_key(|i| i.kind == Kind::Event);
            items.retain(|i| {
                let key = (i.note.record.path.clone(), i.note.record.heading);
                if seen.contains(&key) {
                    return false;
                }
                seen.push(key);
                true
            });
            items.sort_by(|a, b| {
                a.time()
                    .is_none()
                    .cmp(&b.time().is_none())
                    .then(a.time().cmp(&b.time()))
                    .then_with(|| {
                        a.note
                            .record
                            .title
                            .to_lowercase()
                            .cmp(&b.note.record.title.to_lowercase())
                    })
            });
        }
        self.ongoing.sort_by(|a, b| {
            a.note
                .record
                .path
                .cmp(&b.note.record.path)
                .then(a.start.cmp(&b.start))
                .then(a.end.cmp(&b.end))
        });
    }
}

/// Builds the calendar for `from..=to` as seen on `today`.
#[must_use]
pub fn build(notes: &[Note], from: NaiveDate, to: NaiveDate, today: NaiveDate) -> Calendar {
    let mut cal = Calendar {
        from,
        to,
        today,
        ..Calendar::default()
    };
    for n in notes.iter().filter(|n| !n.record.is_intro()) {
        match (&n.scheduled, &n.deadline) {
            (Some(s), Some(d)) => cal.add_planned_span(n, s, d),
            (Some(t), None) => cal.add_plan(n, t, Kind::Scheduled),
            (None, Some(t)) => cal.add_plan(n, t, Kind::Deadline),
            (None, None) => {}
        }
        for t in &n.events {
            cal.add_event(n, t);
        }
    }
    cal.tidy();
    cal
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

    fn titles(c: &Calendar, d: u32) -> Vec<(String, Option<i64>)> {
        c.on(day(d))
            .iter()
            .map(|i| (i.note.record.title.clone(), i.late))
            .collect()
    }

    #[test]
    fn one_day_items_and_ongoing_spans() {
        let n = notes(
            "* TODO Beta [3/8]\nSCHEDULED: <2026-10-01 Thu> DEADLINE: <2026-11-30 Mon>\n\
             * Kitchen\n<2026-11-09 Mon>--<2026-12-20 Sun>\n\
             * Review\n<2026-11-26 Thu 10:00>\n\
             * TODO Rent\nDEADLINE: <2026-11-26 Thu>\n\
             * TODO Missed\nSCHEDULED: <2026-11-20 Fri>\n\
             * DONE Old\nSCHEDULED: <2026-11-23 Mon>\n\
             * Standup\n<2026-11-16 Mon 09:00 +1w>\n\
             * TODO Late project\nSCHEDULED: <2026-11-01 Sun> DEADLINE: <2026-11-24 Tue>\n\
             * TODO Far\nSCHEDULED: <2026-12-01 Tue> DEADLINE: <2026-12-24 Thu>\n",
        );
        let c = build(&n, day(22), day(28), day(26));
        let spans: Vec<&str> = c
            .ongoing
            .iter()
            .map(|s| s.note.record.title.as_str())
            .collect();
        assert_eq!(spans, ["Beta [3/8]", "Late project", "Kitchen"]);
        assert_eq!(c.ongoing[1].late(day(26)), Some(2));
        assert_eq!(c.ongoing[0].late(day(26)), None);
        // Today: the timed event first, then the late plan carried over.
        assert_eq!(
            titles(&c, 26),
            [
                ("Review".into(), None),
                ("Missed".into(), Some(6)),
                ("Rent".into(), None)
            ]
        );
        assert_eq!(titles(&c, 20), []);
        // Done stays on its date; a weekly event on each Monday.
        assert_eq!(
            titles(&c, 23),
            [("Standup".into(), None), ("Old".into(), None)]
        );
        assert!(c.on(day(23))[0].time().is_some());
        // A past week: nothing is carried to a today outside it.
        let past = build(&n, day(16), day(22), day(26));
        assert_eq!(titles(&past, 20), []);
        assert_eq!(titles(&past, 16), [("Standup".into(), None)]);
        // Keeping one file.
        let mut other = notes("* Gym\n<2026-11-26 Thu 07:00>\n");
        for n in &mut other {
            n.record.path = "b.org".into();
        }
        let both: Vec<Note> = n.iter().cloned().chain(other).collect();
        let c = build(&both, day(22), day(28), day(26));
        assert_eq!(c.on(day(26)).len(), 4);
        let b = c.only(|p| p == "b.org");
        assert_eq!(titles(&b, 26), [("Gym".into(), None)]);
        assert_eq!(b.ongoing, []);
        assert_eq!(b.days.len(), 1);
    }
}
