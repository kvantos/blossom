//! Deciding matches, ranking and the agenda layout, ported from Bloom's
//! `QueryMatcher.kt`. The index only narrows candidates; every result is
//! decided here.

use std::cmp::Ordering;

use blossom_core::Timestamp;
use chrono::{NaiveDate, NaiveDateTime, NaiveTime, TimeDelta};

use crate::query::{
    CompareOp, Condition, Period, Query, SortField, SortKey, StateType, Target, Term,
};
use crate::record::Note;

/// Settings and the clock that change what a query means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchContext {
    /// What `now` and every relative date count from.
    pub now: NaiveDateTime,
    /// The configured keyword order (active then done), for `o.state`.
    pub state_order: Vec<String>,
    /// Org's default priority: `p.X` and `o.p` treat no priority as this.
    pub default_priority: Option<String>,
}

impl MatchContext {
    fn today(&self) -> NaiveDate {
        self.now.date()
    }
}

/// Whether `note` matches `query`.
#[must_use]
pub fn matches(note: &Note, query: &Query, ctx: &MatchContext) -> bool {
    if let Some(days) = query.agenda_days
        && !in_agenda_window(note, ctx.today(), days)
    {
        return false;
    }
    query
        .expr
        .as_ref()
        .is_none_or(|e| e.matches(&mut |t| term_matches(note, t, ctx)))
}

/// The last day `ad.N` covers: `ad.1` is today only (`ad.0` counts as 1).
#[must_use]
pub fn agenda_horizon(today: NaiveDate, days: u32) -> NaiveDate {
    today
        .checked_add_days(chrono::Days::new(u64::from(days.max(1) - 1)))
        .unwrap_or(today)
}

/// `ad.N`: a SCHEDULED or DEADLINE on or before the horizon (overdue
/// included), or an event covering one of the N days.
fn in_agenda_window(note: &Note, today: NaiveDate, days: u32) -> bool {
    let horizon = agenda_horizon(today, days);
    let planned = |t: &Option<Timestamp>| t.as_ref().is_some_and(|t| t.date <= horizon);
    planned(&note.scheduled)
        || planned(&note.deadline)
        || note.events.iter().any(|e| e.occurs_between(today, horizon))
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

fn term_matches(note: &Note, term: &Term, ctx: &MatchContext) -> bool {
    let r = &note.record;
    let result = match &term.condition {
        // Title, body or the note's own tags (Orgzly).
        Condition::Text(t) => {
            contains_ci(&r.text, t) || r.tags.iter().any(|tag| contains_ci(tag, t))
        }
        Condition::State(s) => {
            if s.eq_ignore_ascii_case("none") {
                r.keyword.is_none()
            } else {
                r.keyword
                    .as_deref()
                    .is_some_and(|k| k.eq_ignore_ascii_case(s))
            }
        }
        Condition::StateType(t) => match t {
            StateType::Todo => r.keyword.is_some() && !r.done,
            StateType::Done => r.keyword.is_some() && r.done,
            StateType::None => r.keyword.is_none(),
        },
        Condition::Notebook(name) => notebook_matches(&r.path, name),
        Condition::Tag { tag, own_only } => {
            let pool = if *own_only {
                &r.tags
            } else {
                &r.inherited_tags
            };
            pool.iter().any(|t| contains_ci(t, tag))
        }
        Condition::Priority { priority, set_only } => {
            let effective = if *set_only {
                r.priority.as_ref()
            } else {
                r.priority.as_ref().or(ctx.default_priority.as_ref())
            };
            effective.is_some_and(|p| p.eq_ignore_ascii_case(priority))
        }
        // Orgzly's default operators: le for s./d./cr., eq for c./e.
        Condition::Scheduled(p) => date_matches(note.scheduled.as_ref(), p, ctx, CompareOp::Le),
        Condition::Deadline(p) => date_matches(note.deadline.as_ref(), p, ctx, CompareOp::Le),
        Condition::Closed(p) => date_matches(note.closed.as_ref(), p, ctx, CompareOp::Eq),
        Condition::Created(p) => date_matches(note.created.as_ref(), p, ctx, CompareOp::Le),
        Condition::Event(p) => event_matches(&note.events, p, ctx),
        Condition::Id(prefix) => {
            if prefix.eq_ignore_ascii_case("none") {
                r.id.is_none() && r.custom_id.is_none()
            } else {
                let p = prefix.to_lowercase();
                [&r.id, &r.custom_id]
                    .into_iter()
                    .flatten()
                    .any(|id| id.to_lowercase().starts_with(&p))
            }
        }
    };
    result != term.negated
}

/// `b.NAME`: the vault-relative path or the file name, `.org` optional,
/// ignoring case.
fn notebook_matches(path: &str, name: &str) -> bool {
    let strip = |s: &str| s.strip_suffix(".org").unwrap_or(s).to_lowercase();
    let want = strip(name);
    let path = strip(path);
    path == want || path.rsplit('/').next() == Some(want.as_str())
}

/// The time a timestamp covers, for comparing against a moment: a timed
/// stamp is an instant unless it has an end; an untimed one covers its whole
/// day (so all-day items don't vanish at 00:01); a range runs to its end time
/// or the end of its last day.
fn span(ts: &Timestamp) -> (NaiveDateTime, Option<NaiveDateTime>) {
    let start = ts.date.and_time(ts.time.unwrap_or(NaiveTime::MIN));
    let last = ts.range_end.unwrap_or(ts.date);
    let end = match (ts.end_time, ts.time, ts.range_end) {
        (Some(e), _, _) => Some(last.and_time(e)),
        (None, Some(_), None) => None,
        _ => last
            .and_time(NaiveTime::MIN)
            .checked_add_signed(TimeDelta::days(1)),
    };
    (start, end)
}

/// A span against a moment: `eq` means the moment falls inside it.
fn span_test(
    op: CompareOp,
    (start, end): (NaiveDateTime, Option<NaiveDateTime>),
    at: NaiveDateTime,
) -> bool {
    let Some(end) = end else {
        return op.test(&start, &at);
    };
    let inside = at >= start && at < end;
    match op {
        CompareOp::Eq => inside,
        CompareOp::Ne => !inside,
        CompareOp::Lt => start < at,
        CompareOp::Le => start <= at,
        CompareOp::Gt | CompareOp::Ge => end > at,
    }
}

fn date_matches(
    ts: Option<&Timestamp>,
    period: &Period,
    ctx: &MatchContext,
    default: CompareOp,
) -> bool {
    let target = period.target(ctx.now);
    let Some(ts) = ts else {
        return target == Target::NoDate;
    };
    let op = period.op.unwrap_or(default);
    match target {
        Target::NoDate | Target::Invalid => false,
        Target::Overdue => ts.date < ctx.today(),
        Target::Moment(at) => span_test(op, span(ts), at),
        Target::Day(day) => op.test(&ts.date, &day),
    }
}

/// `e.`: any event day satisfies it; a range counts every day it spans.
fn event_matches(events: &[Timestamp], period: &Period, ctx: &MatchContext) -> bool {
    let op = period.op.unwrap_or(CompareOp::Eq);
    match period.target(ctx.now) {
        Target::NoDate => events.is_empty(),
        Target::Invalid => false,
        Target::Overdue => events
            .iter()
            .any(|e| e.range_end.unwrap_or(e.date) < ctx.today()),
        Target::Moment(at) => events.iter().any(|e| span_test(op, span(e), at)),
        Target::Day(day) => events.iter().any(|e| {
            let mut d = e.date;
            loop {
                if op.test(&d, &day) {
                    return true;
                }
                match d.succ_opt() {
                    Some(next) if next <= e.range_end.unwrap_or(e.date) => d = next,
                    _ => return false,
                }
            }
        }),
    }
}

// --- ranking ---

/// Sorts matches: by `o.` keys when given; else, with text terms, exact
/// title, then title contains, then anything, newest file first; else
/// Orgzly's default order (notebook, priority, scheduled/deadline when the
/// query filters on them, position).
pub fn sort(notes: &mut [Note], query: &Query, ctx: &MatchContext) {
    if !query.sort.is_empty() {
        notes.sort_by(|a, b| {
            query
                .sort
                .iter()
                .map(|k| compare_key(a, b, *k, ctx))
                .find(|o| o.is_ne())
                .unwrap_or(Ordering::Equal)
        });
        return;
    }
    let terms: Vec<String> = query
        .text_terms()
        .iter()
        .map(|t| t.to_lowercase())
        .collect();
    if !terms.is_empty() {
        let rank = |n: &Note| {
            let title = n.record.title.to_lowercase();
            if terms.contains(&title) {
                0
            } else if terms.iter().any(|t| title.contains(t.as_str())) {
                1
            } else {
                2
            }
        };
        notes.sort_by(|a, b| {
            rank(a)
                .cmp(&rank(b))
                .then(b.record.modified.cmp(&a.record.modified))
        });
        return;
    }
    let filters: Vec<&Condition> = query.expr.as_ref().map_or_else(Vec::new, |e| {
        e.leaves()
            .into_iter()
            .filter(|t| !t.negated)
            .map(|t| &t.condition)
            .collect()
    });
    let by_sched = filters.iter().any(|c| matches!(c, Condition::Scheduled(_)));
    let by_dead = filters.iter().any(|c| matches!(c, Condition::Deadline(_)));
    let asc = |field| SortKey {
        field,
        descending: false,
    };
    notes.sort_by(|a, b| {
        let (pa, pb) = (&a.record.path, &b.record.path);
        pa.to_lowercase()
            .cmp(&pb.to_lowercase())
            .then(pa.cmp(pb))
            .then(compare_key(a, b, asc(SortField::Priority), ctx))
            .then(if by_sched {
                compare_key(a, b, asc(SortField::Scheduled), ctx)
            } else {
                Ordering::Equal
            })
            .then(if by_dead {
                compare_key(a, b, asc(SortField::Deadline), ctx)
            } else {
                Ordering::Equal
            })
            .then(a.record.line.cmp(&b.record.line))
    });
}

/// Missing values sort last in either direction.
fn nulls_last<T: Ord>(a: Option<T>, b: Option<T>, descending: bool) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) if descending => y.cmp(&x),
        (Some(x), Some(y)) => x.cmp(&y),
    }
}

fn moment(ts: Option<&Timestamp>) -> Option<NaiveDateTime> {
    ts.map(|t| t.date.and_time(t.time.unwrap_or(NaiveTime::MIN)))
}

fn compare_key(a: &Note, b: &Note, key: SortKey, ctx: &MatchContext) -> Ordering {
    let desc = key.descending;
    match key.field {
        SortField::Notebook => nulls_last(
            Some(a.record.path.to_lowercase()),
            Some(b.record.path.to_lowercase()),
            desc,
        ),
        SortField::Title => nulls_last(
            Some(a.record.title.to_lowercase()),
            Some(b.record.title.to_lowercase()),
            desc,
        ),
        SortField::Scheduled => nulls_last(
            moment(a.scheduled.as_ref()),
            moment(b.scheduled.as_ref()),
            desc,
        ),
        SortField::Deadline => nulls_last(
            moment(a.deadline.as_ref()),
            moment(b.deadline.as_ref()),
            desc,
        ),
        SortField::Closed => nulls_last(moment(a.closed.as_ref()), moment(b.closed.as_ref()), desc),
        SortField::Created => {
            nulls_last(moment(a.created.as_ref()), moment(b.created.as_ref()), desc)
        }
        // Ascending uses the oldest event, descending the most recent.
        SortField::Event => {
            let pick = |n: &Note| {
                let times = n.events.iter().filter_map(|e| moment(Some(e)));
                if desc { times.max() } else { times.min() }
            };
            nulls_last(pick(a), pick(b), desc)
        }
        SortField::Priority => {
            let p = |n: &Note| {
                n.record
                    .priority
                    .as_ref()
                    .or(ctx.default_priority.as_ref())
                    .map(|p| p.to_uppercase())
            };
            nulls_last(p(a), p(b), desc)
        }
        SortField::State => {
            let s = |n: &Note| {
                n.record.keyword.as_ref().map(|k| {
                    ctx.state_order
                        .iter()
                        .position(|o| o.eq_ignore_ascii_case(k))
                        .unwrap_or(ctx.state_order.len())
                })
            };
            nulls_last(s(a), s(b), desc)
        }
    }
}

// --- agenda ---

/// Why a note is on an agenda day.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgendaReason {
    /// Its SCHEDULED date.
    Scheduled,
    /// Its DEADLINE date.
    Deadline,
    /// A bare active timestamp covering the day.
    Event,
}

/// One row of the agenda.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgendaItem {
    /// The note.
    pub note: Note,
    /// Why it is here.
    pub reason: AgendaReason,
    /// The time shown for it, if timed.
    pub time: Option<NaiveTime>,
}

/// One section of the agenda.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgendaDay {
    /// The day, or `None` for the Overdue section.
    pub date: Option<NaiveDate>,
    /// Timed items first in time order, then untimed ones.
    pub items: Vec<AgendaItem>,
}

/// Lays out `ad.N` results by day (Bloom's search agenda): Overdue first, then
/// each day that has something. A note lands on its SCHEDULED day, else its
/// DEADLINE day (also when SCHEDULED is past the window); an event shows on
/// every day of the window it covers.
#[must_use]
pub fn agenda(notes: &[Note], today: NaiveDate, days: u32) -> Vec<AgendaDay> {
    let horizon = agenda_horizon(today, days);
    let mut placed: Vec<(Option<NaiveDate>, AgendaItem)> = Vec::new();
    for note in notes {
        let planning = within(note.scheduled.as_ref(), horizon)
            .map(|t| (t, AgendaReason::Scheduled))
            .or_else(|| {
                within(note.deadline.as_ref(), horizon).map(|t| (t, AgendaReason::Deadline))
            });
        if let Some((ts, reason)) = planning {
            let day = (ts.date >= today).then_some(ts.date);
            placed.push((
                day,
                AgendaItem {
                    note: note.clone(),
                    reason,
                    time: ts.time,
                },
            ));
        }
        let mut day = today;
        while day <= horizon {
            if note
                .events
                .iter()
                .any(|e| e.occurrence_covering(day).is_some())
            {
                let time = note
                    .events
                    .iter()
                    .find(|e| e.occurrence_covering(day).is_some())
                    .and_then(|e| e.time);
                placed.push((
                    Some(day),
                    AgendaItem {
                        note: note.clone(),
                        reason: AgendaReason::Event,
                        time,
                    },
                ));
            }
            let Some(next) = day.succ_opt() else { break };
            day = next;
        }
    }
    // Stable sort keeps the matcher's order within equal keys.
    placed
        .sort_by(|(da, a), (db, b)| nulls_first(*da, *db).then(nulls_last(a.time, b.time, false)));
    let mut out: Vec<AgendaDay> = Vec::new();
    for (date, item) in placed {
        match out.last_mut() {
            Some(last) if last.date == date => last.items.push(item),
            _ => out.push(AgendaDay {
                date,
                items: vec![item],
            }),
        }
    }
    out
}

fn within(t: Option<&Timestamp>, horizon: NaiveDate) -> Option<&Timestamp> {
    t.filter(|t| t.date <= horizon)
}

fn nulls_first(a: Option<NaiveDate>, b: Option<NaiveDate>) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(x), Some(y)) => x.cmp(&y),
    }
}
