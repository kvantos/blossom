//! The "Ongoing" timeline: one bar per heading with a start and an end,
//! grouped by file, over the days shown. A bar's fill is its progress, a
//! line marks today, a late one gets a red tail to today. Closed, the
//! header keeps a one-line summary.

use std::fmt::Write as _;

use blossom_index::calendar::{Calendar, Span};
use chrono::{Datelike, Days, NaiveDate};
use iced::widget::{Column, Row, button, column, container, row, space, stack, text};
use iced::{Element, Fill, Length, Theme};

use super::{AgendaMsg, Scope, View, body_lines, card_box, colour, file_label, hoverable};
use crate::app::{Message, bold, dim};
use crate::style;

/// The width of the titles beside the bars.
const LABELS: f32 = 220.0;

/// How a span is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Its heading is done.
    Done,
    /// It starts later.
    Starts(NaiveDate),
    /// Past its end and still open.
    Late(i64),
    /// Progress behind the time gone by.
    Behind,
    /// Progress keeping up.
    OnTrack,
    /// No progress to go by: days to its end.
    DaysLeft(i64),
}

impl Status {
    /// The span's status on `today`.
    #[must_use]
    pub fn of(s: &Span, today: NaiveDate) -> Status {
        if s.note.record.done {
            return Status::Done;
        }
        if s.start > today {
            return Status::Starts(s.start);
        }
        if let Some(n) = s.late(today) {
            return Status::Late(n);
        }
        match s.note.record.progress {
            Some((done, total)) if total > 0 => {
                let length = (s.end - s.start).num_days().max(1);
                #[allow(clippy::cast_precision_loss)]
                let gone = (today - s.start).num_days() as f32 / length as f32;
                #[allow(clippy::cast_precision_loss)]
                let made = done as f32 / total as f32;
                if made + 0.1 < gone {
                    Status::Behind
                } else {
                    Status::OnTrack
                }
            }
            _ => Status::DaysLeft((s.end - today).num_days()),
        }
    }

    fn label(&self) -> String {
        match self {
            Status::Done => "Done".into(),
            Status::Starts(d) => format!("Starts {}", d.format("%-d %b")),
            Status::Late(n) => format!("{n} day{} late", plural(*n)),
            Status::Behind => "Behind".into(),
            Status::OnTrack => "On track".into(),
            Status::DaysLeft(0) => "Ends today".into(),
            Status::DaysLeft(n) => format!("{n} day{} left", plural(*n)),
        }
    }
}

fn plural(n: i64) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// `3/8` or `40%`.
fn progress_label(p: (u32, u32)) -> String {
    if p.1 == 100 {
        format!("{}%", p.0)
    } else {
        format!("{}/{}", p.0, p.1)
    }
}

/// The days shown, as a column count.
fn days(from: NaiveDate, to: NaiveDate) -> u16 {
    u16::try_from((to - from).num_days() + 1)
        .unwrap_or(1)
        .max(1)
}

fn offset(from: NaiveDate, d: NaiveDate) -> u16 {
    u16::try_from((d - from).num_days().max(0)).unwrap_or(0)
}

/// The column labels: each day in Week and Month, each month in Quarter
/// and Year, as (first day, days, label).
fn columns(scope: Scope, from: NaiveDate, to: NaiveDate) -> Vec<(NaiveDate, u16, String)> {
    let mut out = Vec::new();
    let mut d = from;
    while d <= to {
        let (len, label) = match scope {
            Scope::Week => (1, d.format("%a %-d").to_string()),
            Scope::Month => (1, d.day().to_string()),
            Scope::Quarter | Scope::Year => {
                let end = super::month_end(d).min(to);
                (days(d, end), d.format("%b").to_string())
            }
        };
        out.push((d, len, label));
        d = d + Days::new(u64::from(len));
    }
    out
}

/// Where the timeline draws its tick lines, as day offsets from `from`
/// with whether the line is a strong one: days in Week, weeks in Month and
/// Quarter (months strong in Quarter), months in Year (quarters strong).
fn ticks(v: &View<'_>, from: NaiveDate, to: NaiveDate) -> Vec<(u16, bool)> {
    let mut out = Vec::new();
    let mut d = from + Days::new(1);
    while d <= to {
        let week = v.prefs.week_of(d) == d;
        let month = d.day() == 1;
        let quarter = month && d.month0().is_multiple_of(3);
        let line = match v.place.scope {
            Scope::Week => Some(false),
            Scope::Month => week.then_some(false),
            Scope::Quarter => (week || month).then_some(month),
            Scope::Year => month.then_some(quarter),
        };
        if let Some(strong) = line {
            out.push((offset(from, d), strong));
        }
        d = d + Days::new(1);
    }
    out
}

/// The tick lines as a layer behind a row's bar.
fn grid<'a>(v: &View<'a>, from: NaiveDate, to: NaiveDate) -> Element<'a, Message> {
    let n = days(from, to);
    let mut cells: Vec<Element<'a, Message>> = Vec::new();
    let mut at = 0u16;
    let mut strong_line = None;
    for (off, strong) in ticks(v, from, to).into_iter().chain([(n, false)]) {
        if off > at {
            let mut cell = Row::new().height(34);
            if let Some(strong) = strong_line {
                cell = cell.push(container(space()).width(1).height(Fill).style(
                    move |theme: &Theme| {
                        let p = theme.extended_palette();
                        let bg = p.background.base.color;
                        let t = if strong { 0.9 } else { 0.4 };
                        container::Style {
                            background: Some(style::mix(bg, p.background.strong.color, t).into()),
                            ..container::Style::default()
                        }
                    },
                ));
            }
            cells.push(container(cell).width(Length::FillPortion(off - at)).into());
        }
        at = off;
        strong_line = Some(strong);
    }
    Row::from_iter(cells).height(34).into()
}

/// The spans touching the shown days, and the summary counts.
fn summary(spans: &[&Span], today: NaiveDate) -> (usize, usize, usize) {
    let open: Vec<Status> = spans
        .iter()
        .map(|s| Status::of(s, today))
        .filter(|s| *s != Status::Done)
        .collect();
    let behind = open.iter().filter(|s| **s == Status::Behind).count();
    let late = open.iter().filter(|s| matches!(s, Status::Late(_))).count();
    (open.len(), behind, late)
}

/// The timeline.
pub fn view<'a>(cal: &'a Calendar, v: &View<'a>) -> Element<'a, Message> {
    let (from, to) = v.place.shown(v.prefs);
    let visible = super::visible_spans(cal, v.place, v.prefs, v.today);
    let spans: Vec<&Span> = visible.iter().map(|i| &cal.ongoing[*i]).collect();
    let (open, behind, late) = summary(&spans, v.today);
    let period = match v.place.scope {
        Scope::Week => "this week",
        Scope::Month => "this month",
        Scope::Quarter => "this quarter",
        Scope::Year => "this year",
    };
    let line = if v.place.timeline {
        format!("{open} in {period} · the fill shows progress, the line is today")
    } else {
        let mut s = format!("{open} ongoing");
        if behind > 0 {
            let _ = write!(s, " · {behind} behind");
        }
        if late > 0 {
            let _ = write!(s, " · {late} late");
        }
        s
    };
    let header = button(
        row![
            text(if v.place.timeline { "▾" } else { "▸" }).size(12),
            text("Ongoing").size(13).font(bold()),
            text(line).size(12).style(dim),
        ]
        .spacing(8)
        .align_y(iced::Center),
    )
    .on_press(Message::Agenda(AgendaMsg::ToggleTimeline))
    .padding([4, 6])
    .style(button::text);
    let mut parts: Vec<Element<'a, Message>> = vec![header.into()];
    if v.place.timeline {
        if spans.is_empty() {
            parts.push(
                text(
                    "Nothing ongoing. A heading with both SCHEDULED and DEADLINE, or a date \
                     range like <2026-11-09 Mon>--<2026-12-20 Sun>, shows here.",
                )
                .size(12)
                .style(dim)
                .into(),
            );
        } else {
            parts.push(labels_row(v, from, to));
            let mut file: Option<&str> = None;
            for i in visible {
                let s = &cal.ongoing[i];
                let path = s.note.record.path.as_str();
                if file != Some(path) {
                    file = Some(path);
                    parts.push(file_row(path, v.slot(path)));
                }
                parts.push(span_row(s, v, from, to, v.span_focused(i)));
            }
        }
    }
    Column::with_children(parts)
        .spacing(4)
        .padding([8, 14])
        .into()
}

fn labels_row<'a>(v: &View<'a>, from: NaiveDate, to: NaiveDate) -> Element<'a, Message> {
    let today = v.today;
    let cells = columns(v.place.scope, from, to)
        .into_iter()
        .map(|(start, len, label)| {
            let here = (start..start + Days::new(u64::from(len))).contains(&today);
            let t = text(label).size(11).center().width(Fill);
            let t = if here {
                t.font(bold()).style(|theme: &Theme| text::Style {
                    color: Some(theme.palette().primary),
                })
            } else {
                t.style(dim)
            };
            container(t).width(Length::FillPortion(len)).into()
        });
    row![space().width(LABELS), Row::from_iter(cells)]
        .align_y(iced::Center)
        .into()
}

fn file_row<'a>(path: &str, slot: usize) -> Element<'a, Message> {
    row![
        super::dot_for(slot),
        text(format!("{}.ORG", file_label(path).to_uppercase()))
            .size(11)
            .font(bold())
            .style(dim),
    ]
    .spacing(6)
    .padding(iced::Padding::default().top(6))
    .align_y(iced::Center)
    .into()
}

fn status_text<'a>(s: &Span, today: NaiveDate) -> Element<'a, Message> {
    let status = Status::of(s, today);
    let mut label = status.label();
    if let Some(p) = s.note.record.progress {
        label = format!("{} · {label}", progress_label(p));
    }
    let t = text(label).size(11);
    match status {
        Status::Late(_) => t
            .style(|theme: &Theme| text::Style {
                color: Some(theme.extended_palette().danger.base.color),
            })
            .into(),
        _ => t.style(dim).into(),
    }
}

fn span_row<'a>(
    s: &'a Span,
    v: &View<'a>,
    from: NaiveDate,
    to: NaiveDate,
    focused: bool,
) -> Element<'a, Message> {
    let left = column![
        text(super::plain_title(&s.note.record.title)).size(13),
        status_text(s, v.today)
    ]
    .width(LABELS)
    .padding(iced::Padding::default().right(10));
    container(row![left, track(s, v, from, to)].align_y(iced::Center))
        .padding([0, 2])
        .style(move |theme: &Theme| container::Style {
            border: if focused {
                super::focus_ring(theme)
            } else {
                iced::Border::default()
            },
            ..container::Style::default()
        })
        .into()
}

/// A fixed-height strip of `n` units of space.
fn gap<'a>(n: u16) -> Element<'a, Message> {
    space().width(Length::FillPortion(n)).into()
}

fn track<'a>(s: &'a Span, v: &View<'a>, from: NaiveDate, to: NaiveDate) -> Element<'a, Message> {
    let n = days(from, to);
    let start = s.start.max(from);
    let end = s.end.min(to);
    let lead = offset(from, start);
    let len = if end >= start { days(start, end) } else { 0 };
    let tail = match s.late(v.today) {
        Some(_) => {
            let t0 = (s.end + Days::new(1)).max(from);
            let t1 = v.today.min(to);
            if t1 >= t0 { days(t0, t1) } else { 0 }
        }
        None => 0,
    };
    let lead = if len == 0 {
        offset(from, (s.end + Days::new(1)).max(from))
    } else {
        lead
    };
    let rest = n.saturating_sub(lead + len + tail);
    let mut bars: Vec<Element<'a, Message>> = Vec::new();
    if lead > 0 {
        bars.push(gap(lead));
    }
    if len > 0 {
        bars.push(
            container(bar(
                s,
                s.start < from,
                s.end > to,
                s.late(v.today).is_some(),
                v.slot(&s.note.record.path),
            ))
            .width(Length::FillPortion(len))
            .into(),
        );
    }
    if tail > 0 {
        bars.push(
            container(
                container(space())
                    .height(2)
                    .width(Fill)
                    .style(|theme: &Theme| container::Style {
                        background: Some(theme.extended_palette().danger.base.color.into()),
                        ..container::Style::default()
                    }),
            )
            .center_y(24)
            .width(Length::FillPortion(tail))
            .into(),
        );
    }
    if rest > 0 {
        bars.push(gap(rest));
    }
    // Tick lines, then today's line, under the bars so their names stay
    // readable.
    let mut layers: Vec<Element<'a, Message>> = vec![grid(v, from, to)];
    if (from..=to).contains(&v.today) {
        let off = offset(from, v.today);
        layers.push(
            row![
                gap(2 * off + 1),
                container(space())
                    .width(2)
                    .height(Fill)
                    .style(|theme: &Theme| {
                        container::Style {
                            background: Some(theme.palette().primary.into()),
                            ..container::Style::default()
                        }
                    }),
                gap((2 * (n - off)).saturating_sub(1).max(1)),
            ]
            .height(34)
            .into(),
        );
    }
    layers.push(
        container(Row::from_iter(bars).height(24).align_y(iced::Center))
            .center_y(34)
            .width(Fill)
            .into(),
    );
    container(stack(layers).width(Fill).height(34))
        .center_y(34)
        .width(Fill)
        .into()
}

/// The bar itself: tinted in its file's colour, filled to its progress.
fn bar<'a>(
    s: &'a Span,
    clipped_left: bool,
    clipped_right: bool,
    late: bool,
    slot: usize,
) -> Element<'a, Message> {
    let mut layers: Vec<Element<'a, Message>> = Vec::new();
    if let Some((done, total)) = s.note.record.progress.filter(|p| p.0 > 0 && p.1 > 0) {
        let done =
            u16::try_from((u64::from(done) * 1000 / u64::from(total)).min(1000)).unwrap_or(0);
        let mut fill = vec![
            container(space())
                .width(Length::FillPortion(done.max(1)))
                .height(Fill)
                .style(move |theme: &Theme| container::Style {
                    background: Some(
                        style::mix(theme.palette().background, colour(theme, slot), 0.38).into(),
                    ),
                    ..container::Style::default()
                })
                .into(),
        ];
        if done < 1000 {
            fill.push(gap(1000 - done));
        }
        layers.push(Row::from_iter(fill).height(Fill).into());
    }
    let mut label = String::new();
    if clipped_left {
        label.push_str("‹ ");
    }
    label.push_str(&super::plain_title(&s.note.record.title));
    let mut line = row![text(label).size(12).wrapping(text::Wrapping::None)]
        .spacing(6)
        .padding([0, 8])
        .height(Fill)
        .align_y(iced::Center);
    if clipped_right {
        line = line.push(space::horizontal()).push(
            text(format!("ends {}", s.end.format("%-d %b")))
                .size(11)
                .style(dim)
                .wrapping(text::Wrapping::None),
        );
    }
    layers.push(line.into());
    // A mark on the DEADLINE, red once it's passed.
    if s.planned && !clipped_right {
        layers.push(
            row![
                space::horizontal(),
                container(space())
                    .width(4)
                    .height(Fill)
                    .style(move |theme: &Theme| container::Style {
                        background: Some(
                            if late {
                                theme.extended_palette().danger.base.color
                            } else {
                                colour(theme, slot)
                            }
                            .into(),
                        ),
                        ..container::Style::default()
                    }),
            ]
            .into(),
        );
    }
    let done = s.note.record.done;
    let content = container(stack(layers).width(Fill).height(22))
        .width(Fill)
        .height(22)
        .clip(true)
        .style(move |theme: &Theme| {
            let c = colour(theme, slot);
            let bg = theme.palette().background;
            let alpha = if done { 0.5 } else { 1.0 };
            container::Style {
                background: Some(style::mix(bg, c, 0.16).scale_alpha(alpha).into()),
                border: iced::Border {
                    color: style::mix(bg, c, 0.5).scale_alpha(alpha),
                    width: 1.0,
                    radius: 5.0.into(),
                },
                ..container::Style::default()
            }
        });
    hoverable(content, span_card(s, slot), &s.note)
}

/// The hover card for a span.
pub fn span_card<'a>(s: &Span, slot: usize) -> Element<'a, Message> {
    let mut parts: Vec<Element<'a, Message>> = vec![
        text(super::title_line(&s.note))
            .size(14)
            .font(bold())
            .into(),
        row![
            super::dot_for(slot),
            text(s.note.record.path.clone()).size(12).style(dim)
        ]
        .spacing(6)
        .align_y(iced::Center)
        .into(),
        text(format!(
            "{} – {}",
            s.start.format("%a %-d %b %Y"),
            s.end.format("%a %-d %b %Y")
        ))
        .size(12)
        .into(),
    ];
    if let Some(p) = s.note.record.progress {
        parts.push(text(format!("{} done", progress_label(p))).size(12).into());
    }
    for l in body_lines(&s.note) {
        parts.push(text(l).size(12).into());
    }
    parts.push(super::org_line(s.org.clone()));
    card_box(parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use blossom_core::{Document, Keywords};
    use blossom_index::calendar::build;
    use blossom_index::records;

    fn d(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, day).unwrap()
    }

    #[test]
    fn statuses() {
        let text = "* TODO Beta [3/8]\nSCHEDULED: <2026-10-01 Thu> DEADLINE: <2026-12-31 Thu>\n\
                    * TODO Ahead [9/10]\nSCHEDULED: <2026-10-01 Thu> DEADLINE: <2026-12-31 Thu>\n\
                    * Kitchen\n<2026-11-09 Mon>--<2026-12-20 Sun>\n\
                    * TODO Late\nSCHEDULED: <2026-11-01 Sun> DEADLINE: <2026-11-24 Tue>\n\
                    * TODO Soon\nSCHEDULED: <2026-12-01 Tue> DEADLINE: <2026-12-24 Thu>\n\
                    * DONE Over\nSCHEDULED: <2026-11-01 Sun> DEADLINE: <2026-11-24 Tue>\n";
        let doc = Document::parse(text, Keywords::default());
        let notes: Vec<_> = records("a.org", &doc, &Keywords::default(), 0)
            .into_iter()
            .map(blossom_index::Note::from)
            .collect();
        let cal = build(&notes, d(1), d(30), d(26));
        let got: Vec<(String, Status)> = cal
            .ongoing
            .iter()
            .map(|s| (s.note.record.title.clone(), Status::of(s, d(26))))
            .collect();
        let want = [
            ("Beta [3/8]", Status::Behind),
            ("Ahead [9/10]", Status::OnTrack),
            ("Late", Status::Late(2)),
            ("Over", Status::Done),
            ("Kitchen", Status::DaysLeft(24)),
        ];
        for (title, status) in want {
            assert!(
                got.contains(&(title.to_string(), status.clone())),
                "{title}: {got:?}"
            );
        }
        let spans: Vec<&Span> = cal.ongoing.iter().collect();
        assert_eq!(summary(&spans, d(26)), (4, 1, 1));
        assert_eq!(progress_label((40, 100)), "40%");
        assert_eq!(progress_label((3, 8)), "3/8");
        let months = columns(
            Scope::Quarter,
            NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
            NaiveDate::from_ymd_opt(2026, 12, 31).unwrap(),
        );
        assert_eq!(months.iter().map(|m| m.1).collect::<Vec<_>>(), [31, 30, 31]);
    }
}
