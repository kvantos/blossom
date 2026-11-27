//! The agenda: Week, Month, Quarter or Year, with previous/next and Today.
//! On top, the "Ongoing" timeline (headings with both SCHEDULED and
//! DEADLINE, and date ranges); below it, the calendar of one-day items. The
//! 3-month strip of mini calendars sits above both in Week and Month.
//! Items are laid out by [`blossom_index::calendar`]; hovering one shows a
//! card with its details, clicking opens the heading.

pub mod month;
pub mod prefs;
pub mod timeline;
pub mod week;

use std::fmt::Write as _;
use std::time::Duration;

use blossom_index::Note;
use blossom_index::calendar::{Calendar, Item, Kind};
use chrono::{Datelike, Days, Months, NaiveDate};
use chrono_tz::Tz;
use iced::widget::{
    Column, button, column, container, row, rule, scrollable, space, text, tooltip,
};
use iced::{Color, Element, Fill, Theme};
use serde::{Deserialize, Serialize};

use crate::app::{Message, bold, dim, placeholder};
use crate::style;
use prefs::AgendaPrefs;

/// How long the pointer rests on an item before its card shows.
const CARD_DELAY: Duration = Duration::from_millis(350);

/// How much the agenda shows at once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Seven days, with an hour grid.
    #[default]
    Week,
    /// A month grid.
    Month,
    /// Three months, for quarterly goals.
    Quarter,
    /// Twelve mini months.
    Year,
}

impl Scope {
    /// Every scope, as the toolbar lists them.
    pub const ALL: [Scope; 4] = [Scope::Week, Scope::Month, Scope::Quarter, Scope::Year];

    fn label(self) -> &'static str {
        match self {
            Scope::Week => "Week",
            Scope::Month => "Month",
            Scope::Quarter => "Quarter",
            Scope::Year => "Year",
        }
    }
}

/// What the agenda's controls do.
#[derive(Clone, Debug)]
pub enum AgendaMsg {
    /// Show a week, month, quarter or year.
    Scope(Scope),
    /// The one before.
    Prev,
    /// The one after.
    Next,
    /// Back to today.
    Today,
    /// Show the day's week, month… (in `Some` scope, else the current one).
    Jump(NaiveDate, Option<Scope>),
    /// Open or close the timeline.
    ToggleTimeline,
}

/// Where the agenda is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    /// Week, month, quarter or year.
    pub scope: Scope,
    /// A day inside what's shown.
    pub anchor: NaiveDate,
    /// The timeline is open.
    pub timeline: bool,
}

impl Place {
    /// `Place` at `today`.
    #[must_use]
    pub fn new(scope: Scope, today: NaiveDate, timeline: bool) -> Place {
        Place {
            scope,
            anchor: today,
            timeline,
        }
    }

    /// Applies `msg`; whether the days shown changed.
    pub fn update(&mut self, msg: &AgendaMsg, today: NaiveDate) -> bool {
        let before = *self;
        match msg {
            AgendaMsg::Scope(s) => self.scope = *s,
            AgendaMsg::Prev => self.anchor = self.step(false),
            AgendaMsg::Next => self.anchor = self.step(true),
            AgendaMsg::Today => self.anchor = today,
            AgendaMsg::Jump(day, scope) => {
                self.anchor = *day;
                if let Some(s) = scope {
                    self.scope = *s;
                }
            }
            AgendaMsg::ToggleTimeline => self.timeline = !self.timeline,
        }
        self.scope != before.scope || self.anchor != before.anchor
    }

    fn step(self, forward: bool) -> NaiveDate {
        let a = self.anchor;
        let months = |n| {
            if forward {
                a.checked_add_months(Months::new(n))
            } else {
                a.checked_sub_months(Months::new(n))
            }
        };
        match self.scope {
            Scope::Week if forward => a.checked_add_days(Days::new(7)),
            Scope::Week => a.checked_sub_days(Days::new(7)),
            Scope::Month => months(1),
            Scope::Quarter => months(3),
            Scope::Year => months(12),
        }
        .unwrap_or(a)
    }

    /// The days shown.
    #[must_use]
    pub fn shown(self, prefs: &AgendaPrefs) -> (NaiveDate, NaiveDate) {
        let a = self.anchor;
        match self.scope {
            Scope::Week => {
                let start = prefs.week_of(a);
                (start, start + Days::new(6))
            }
            Scope::Month => (month_start(a), month_end(a)),
            Scope::Quarter => {
                let first = month_start(a)
                    .with_month(((a.month0() / 3) * 3) + 1)
                    .unwrap_or(a);
                (first, month_end(first + Months::new(2)))
            }
            Scope::Year => (
                NaiveDate::from_ymd_opt(a.year(), 1, 1).unwrap_or(a),
                NaiveDate::from_ymd_opt(a.year(), 12, 31).unwrap_or(a),
            ),
        }
    }

    /// The months of the strip of mini calendars: the anchor's month and
    /// the ones either side (Week and Month only).
    #[must_use]
    pub fn strip(self) -> Option<[NaiveDate; 3]> {
        matches!(self.scope, Scope::Week | Scope::Month).then(|| {
            let m = month_start(self.anchor);
            [m - Months::new(1), m, m + Months::new(1)]
        })
    }

    /// The days to ask the worker for: what's shown and the strip.
    #[must_use]
    pub fn fetch(self, prefs: &AgendaPrefs) -> (NaiveDate, NaiveDate) {
        let (from, to) = self.shown(prefs);
        match self.strip() {
            Some([first, _, last]) => (from.min(first), to.max(month_end(last))),
            None => (from, to),
        }
    }

    /// The toolbar's title: `22 – 28 Nov 2026`, `November 2026`,
    /// `Q4 2026`, `2026`.
    #[must_use]
    pub fn title(self, prefs: &AgendaPrefs) -> String {
        let (from, to) = self.shown(prefs);
        match self.scope {
            Scope::Week if from.year() != to.year() => {
                format!("{} – {}", from.format("%-d %b %Y"), to.format("%-d %b %Y"))
            }
            Scope::Week if from.month() != to.month() => {
                format!("{} – {}", from.format("%-d %b"), to.format("%-d %b %Y"))
            }
            Scope::Week => format!("{} – {}", from.day(), to.format("%-d %b %Y")),
            Scope::Month => from.format("%B %Y").to_string(),
            Scope::Quarter => format!(
                "Q{} {} · {} – {}",
                from.month0() / 3 + 1,
                from.year(),
                from.format("%b"),
                to.format("%b")
            ),
            Scope::Year => from.year().to_string(),
        }
    }
}

/// The first of `d`'s month.
#[must_use]
pub fn month_start(d: NaiveDate) -> NaiveDate {
    d.with_day(1).unwrap_or(d)
}

/// The last day of `d`'s month.
#[must_use]
pub fn month_end(d: NaiveDate) -> NaiveDate {
    (month_start(d) + Months::new(1)).pred_opt().unwrap_or(d)
}

/// Everything the agenda view reads.
pub struct View<'a> {
    /// The days asked for, once they came.
    pub calendar: Option<&'a Calendar>,
    /// Where the agenda is.
    pub place: Place,
    /// Week start, working hours, time zones.
    pub prefs: &'a AgendaPrefs,
    /// Today.
    pub today: NaiveDate,
    /// The time now, local, as hours (10.5 is 10:30), for the week's line.
    pub now: f32,
    /// The notes folder's files, in the sidebar's order: a file's colour
    /// is its place here, so files keep their colours from view to view.
    pub files: &'a [crate::tree::FileInfo],
}

impl View<'_> {
    /// The colour slot of the file at `path`.
    #[must_use]
    pub fn slot(&self, path: &str) -> usize {
        self.files.iter().position(|f| f.path == path).unwrap_or(0)
    }
}

/// The whole agenda pane.
pub fn view<'a>(v: &View<'a>) -> Element<'a, Message> {
    let Some(cal) = v.calendar else {
        return column![toolbar(v), placeholder("Loading the agenda…")].into();
    };
    let mut parts: Vec<Element<'a, Message>> = Vec::new();
    if let Some(months) = v.place.strip() {
        parts.push(month::strip(months, cal, v));
        parts.push(rule::horizontal(1).into());
    }
    parts.push(timeline::view(cal, v));
    parts.push(rule::horizontal(1).into());
    let body: Element<'a, Message> = match v.place.scope {
        Scope::Week => week::view(cal, v),
        Scope::Month => month::grid(cal, v),
        Scope::Quarter | Scope::Year => month::many(cal, v),
    };
    parts.push(body);
    column![
        toolbar(v),
        rule::horizontal(1),
        scrollable(Column::with_children(parts)).height(Fill)
    ]
    .into()
}

/// The title without progress cookies (`[3/8]`, `[40%]`), which the
/// timeline shows on its own.
#[must_use]
pub fn plain_title(title: &str) -> String {
    let mut out = String::new();
    let mut rest = title;
    while let Some(i) = rest.find('[') {
        let after = &rest[i..];
        let cookie = after.find(']').filter(|&j| {
            let inner = &after[1..j];
            !inner.is_empty()
                && inner
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == '/' || c == '%')
        });
        if let Some(j) = cookie {
            out.push_str(&rest[..i]);
            rest = &after[j + 1..];
        } else {
            out.push_str(&rest[..=i]);
            rest = &after[1..];
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn send(m: AgendaMsg) -> Message {
    Message::Agenda(m)
}

/// ‹ Today › · title · Week Month Quarter Year.
fn toolbar<'a>(v: &View<'a>) -> Element<'a, Message> {
    let small = |label: &'static str, m: AgendaMsg| {
        button(text(label).size(13))
            .on_press(send(m))
            .padding([3, 10])
            .style(button::secondary)
    };
    let scopes = row(Scope::ALL.map(|s| {
        button(text(s.label()).size(13))
            .on_press(send(AgendaMsg::Scope(s)))
            .padding([3, 12])
            .style(style::row(s == v.place.scope))
            .into()
    }))
    .spacing(2);
    row![
        small("‹", AgendaMsg::Prev),
        small("Today", AgendaMsg::Today),
        small("›", AgendaMsg::Next),
        text(v.place.title(v.prefs)).size(17).font(bold()),
        space::horizontal(),
        scopes,
    ]
    .spacing(8)
    .padding([8, 14])
    .align_y(iced::Center)
    .into()
}

/// The colour of file slot `slot` (see [`View::slot`]): Blossom's pink,
/// then blue, green, amber, violet and teal, then round again.
#[must_use]
pub fn colour(theme: &Theme, slot: usize) -> Color {
    let light: [u32; 6] = [
        0x00c2_457a,
        0x0035_67b0,
        0x002e_7d5b,
        0x009a_6400,
        0x0070_48b5,
        0x001f_7f86,
    ];
    let dark: [u32; 6] = [
        0x00e5_8fb1,
        0x008f_b4ea,
        0x0086_cba6,
        0x00e7_c27d,
        0x00b9_a0ea,
        0x007f_cfd4,
    ];
    let c = if theme.extended_palette().is_dark {
        dark[slot % 6]
    } else {
        light[slot % 6]
    };
    let [_, r, g, b] = c.to_be_bytes();
    Color::from_rgb8(r, g, b)
}

/// `inbox` for `notes/inbox.org`.
#[must_use]
pub fn file_label(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".org").unwrap_or(name)
}

/// What an item's box does: a task is marked done (or repeated, or
/// reopened when done); an event with a repeating timestamp moves that
/// date on; a plain event has no box.
#[must_use]
pub fn complete(item: &Item) -> Option<Message> {
    use blossom_core::EventStamp;
    use blossom_vault::ops::{Complete, HeadingRef};
    let how = if item.note.record.keyword.is_some() {
        Complete::Toggle
    } else if item.stamp.repeats() {
        Complete::Advance(match item.kind {
            Kind::Scheduled => EventStamp::Scheduled,
            Kind::Deadline => EventStamp::Deadline,
            Kind::Event => EventStamp::Active(item.stamp.date),
        })
    } else {
        return None;
    };
    let r = &item.note.record;
    Some(Message::Run(crate::worker::Op::Complete(
        HeadingRef {
            path: r.path.clone(),
            index: r.heading,
            title: r.title.clone(),
        },
        how,
    )))
}

/// The box for `item`, if it has one.
pub fn check_box<'a>(item: &Item) -> Option<Element<'a, Message>> {
    let msg = complete(item)?;
    let done = item.note.record.done;
    Some(
        button(text(if done { "☑" } else { "☐" }).size(12))
            .on_press(msg)
            .padding([0, 2])
            .style(button::text)
            .into(),
    )
}

/// Opens the heading behind `note`.
#[must_use]
pub fn open(note: &Note) -> Message {
    Message::OpenHeading(note.record.path.clone(), note.record.heading)
}

/// The first lines of the heading's body, for its card.
fn body_lines(note: &Note) -> Vec<String> {
    note.record
        .text
        .lines()
        .skip(1)
        .map(str::trim)
        .filter(|l| {
            !l.is_empty()
                && !l.starts_with("SCHEDULED:")
                && !l.starts_with("DEADLINE:")
                && !l.starts_with("CLOSED:")
                && !l.starts_with(':')
        })
        .take(3)
        .map(|l| {
            if l.chars().count() > 70 {
                format!("{}…", l.chars().take(70).collect::<String>())
            } else {
                l.to_string()
            }
        })
        .collect()
}

/// What an item's card says about when: `Thu 26 Nov · 10:00–11:00`.
fn when(item: &Item, day: NaiveDate) -> String {
    let mut s = day.format("%a %-d %b").to_string();
    if let Some(t) = item.stamp.time {
        let _ = write!(s, " · {}", t.format("%H:%M"));
        if let Some(e) = item.stamp.end_time {
            let _ = write!(s, "–{}", e.format("%H:%M"));
        }
    }
    match (item.kind, item.late) {
        (_, Some(n)) => {
            let _ = write!(
                s,
                " · {} {n} day{} ago",
                if item.kind == Kind::Deadline {
                    "due"
                } else {
                    "planned"
                },
                if n == 1 { "" } else { "s" }
            );
        }
        (Kind::Deadline, None) => s.push_str(" · deadline"),
        _ => {}
    }
    s
}

/// A timed item's time in each extra zone: `New York 04:00`.
fn zone_times(item: &Item, day: NaiveDate, zones: &[Tz]) -> Option<String> {
    use chrono::TimeZone;
    let t = item.stamp.time?;
    let local = chrono::Local
        .from_local_datetime(&day.and_time(t))
        .earliest()?;
    let parts: Vec<String> = zones
        .iter()
        .map(|z| {
            let there = local.with_timezone(z);
            let mut s = format!("{} {}", prefs::city(*z), there.format("%H:%M"));
            if there.date_naive() != day {
                let _ = write!(s, " ({})", there.format("%a"));
            }
            s
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// The hover card for a one-day item.
fn card<'a>(item: &Item, day: NaiveDate, zones: &[Tz], slot: usize) -> Element<'a, Message> {
    let mut parts: Vec<Element<'a, Message>> = vec![
        text(title_line(&item.note)).size(14).font(bold()).into(),
        row![
            dot_for(slot),
            text(item.note.record.path.clone()).size(12).style(dim)
        ]
        .spacing(6)
        .align_y(iced::Center)
        .into(),
        text(when(item, day)).size(12).into(),
    ];
    if let Some(z) = zone_times(item, day, zones) {
        parts.push(text(z).size(12).style(dim).into());
    }
    for l in body_lines(&item.note) {
        parts.push(text(l).size(12).into());
    }
    let org = match item.kind {
        Kind::Scheduled => format!("SCHEDULED: {}", item.stamp.format()),
        Kind::Deadline => format!("DEADLINE: {}", item.stamp.format()),
        Kind::Event => item.stamp.format(),
    };
    parts.push(org_line(org));
    card_box(parts)
}

/// `TODO Pay rent`, with the keyword.
fn title_line(note: &Note) -> String {
    match &note.record.keyword {
        Some(k) => format!("{k} {}", note.record.title),
        None => note.record.title.clone(),
    }
}

fn dot_for<'a>(slot: usize) -> Element<'a, Message> {
    container(space())
        .width(8)
        .height(8)
        .style(move |theme: &Theme| container::Style {
            background: Some(colour(theme, slot).into()),
            border: iced::Border {
                radius: 4.0.into(),
                ..iced::Border::default()
            },
            ..container::Style::default()
        })
        .into()
}

fn org_line<'a>(org: String) -> Element<'a, Message> {
    container(text(org).size(11).font(iced::Font::MONOSPACE).style(dim))
        .padding([3, 6])
        .style(style::code_block)
        .into()
}

fn card_box(parts: Vec<Element<'_, Message>>) -> Element<'_, Message> {
    container(Column::with_children(parts).spacing(5))
        .padding([10, 12])
        .max_width(320)
        .style(style::modal)
        .into()
}

/// `content` with `card` shown on hover, clicking opens `note`.
fn hoverable<'a>(
    content: impl Into<Element<'a, Message>>,
    card: Element<'a, Message>,
    note: &Note,
) -> Element<'a, Message> {
    tooltip(
        button(content)
            .on_press(open(note))
            .padding(0)
            .style(button::text)
            .width(Fill),
        card,
        tooltip::Position::FollowCursor,
    )
    .delay(CARD_DELAY)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use prefs::WeekStart;

    fn d(m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, m, day).unwrap()
    }

    #[test]
    fn ranges_and_titles() {
        let prefs = AgendaPrefs {
            week_start: WeekStart::Sunday,
            ..AgendaPrefs::default()
        };
        let mut p = Place::new(Scope::Week, d(11, 26), true);
        assert_eq!(p.shown(&prefs), (d(11, 22), d(11, 28)));
        assert_eq!(p.title(&prefs), "22 – 28 Nov 2026");
        assert_eq!(p.fetch(&prefs), (d(10, 1), d(12, 31)));
        assert!(p.update(&AgendaMsg::Next, d(11, 26)));
        assert_eq!(p.title(&prefs), "29 Nov – 5 Dec 2026");
        assert!(p.update(&AgendaMsg::Scope(Scope::Quarter), d(11, 26)));
        assert_eq!(p.shown(&prefs), (d(10, 1), d(12, 31)));
        assert_eq!(p.title(&prefs), "Q4 2026 · Oct – Dec");
        assert_eq!(p.strip(), None);
        p.update(&AgendaMsg::Scope(Scope::Month), d(11, 26));
        p.update(&AgendaMsg::Prev, d(11, 26));
        assert_eq!(p.title(&prefs), "November 2026");
        p.update(&AgendaMsg::Scope(Scope::Year), d(11, 26));
        assert_eq!(p.shown(&prefs), (d(1, 1), d(12, 31)));
        assert!(!p.update(&AgendaMsg::ToggleTimeline, d(11, 26)));
        assert!(!p.timeline);
        assert!(p.update(&AgendaMsg::Jump(d(3, 4), Some(Scope::Week)), d(11, 26)));
        assert_eq!(p.title(&prefs), "1 – 7 Mar 2026");
    }

    #[test]
    fn cookies_leave_titles() {
        assert_eq!(plain_title("Q4 goal: beta [3/8]"), "Q4 goal: beta");
        assert_eq!(plain_title("[40%] Read [the] book"), "Read [the] book");
    }

    #[test]
    fn file_colours_go_round() {
        let t = Theme::Light;
        assert_eq!(colour(&t, 1), colour(&t, 7));
        assert_ne!(colour(&t, 0), colour(&t, 1));
        assert_eq!(file_label("a/work.org"), "work");
    }
}
