//! The agenda view: Overdue, Due soon, Today, then each later day with
//! something on it, laid out by [`blossom_index::agenda`] with Bloom's rules.
//! A row's box marks a task done (or repeats it) or moves an event's
//! repeating date on; a plain event has no box. Clicking a title opens the
//! heading.

use blossom_core::{Repeater, RepeaterKind, TimeUnit, Timestamp};
use blossom_index::agenda::{Agenda, Completion, Entry, Placement};
use blossom_vault::ops::{Complete, HeadingRef};
use chrono::{Datelike, NaiveDate};
use iced::widget::{Column, button, column, container, row, scrollable, space, text};
use iced::{Element, Fill};

use crate::app::{Message, bold, chip, dim, placeholder};
use crate::style::{self, Chip};
use crate::worker::Op;

/// How many days the agenda shows.
pub const DAYS: u32 = 14;

/// The whole agenda pane.
pub fn view(agenda: Option<&Agenda>, today: NaiveDate) -> Element<'_, Message> {
    let Some(a) = agenda else {
        return placeholder("Loading the agenda…");
    };
    let mut parts: Vec<Element<'_, Message>> = vec![
        row![
            column![
                text(today.format("%A").to_string()).size(24).font(bold()),
                text(today.format("%B %-d").to_string()).size(13).style(dim),
            ]
            .width(Fill),
            button(text("Capture…").size(13))
                .on_press(Message::OpenCapture)
                .padding([3, 10]),
        ]
        .align_y(iced::Center)
        .into(),
    ];
    if !a.overdue.is_empty() {
        section(
            &format!("Overdue · {}", a.overdue.len()),
            &a.overdue,
            today,
            &mut parts,
        );
    }
    if !a.due_soon.is_empty() {
        section("Due soon", &a.due_soon, today, &mut parts);
    }
    for d in &a.days {
        section(&day_label(d.date, today), &d.entries, today, &mut parts);
    }
    scrollable(
        Column::with_children(parts)
            .spacing(8)
            .padding([20, 28])
            .max_width(860),
    )
    .height(Fill)
    .width(Fill)
    .into()
}

/// A section title and its rows.
fn section<'a>(
    title: &str,
    entries: &'a [Entry],
    today: NaiveDate,
    parts: &mut Vec<Element<'a, Message>>,
) {
    parts.push(
        text(title.to_uppercase())
            .size(12)
            .font(bold())
            .style(dim)
            .into(),
    );
    if entries.is_empty() {
        parts.push(text("Nothing planned").size(13).style(dim).into());
    }
    for e in entries {
        parts.push(entry_row(e, today));
    }
}

/// "Today", "Tomorrow", "Friday" within a week, else "Friday, Dec 4".
#[must_use]
pub fn day_label(date: NaiveDate, today: NaiveDate) -> String {
    match (date - today).num_days() {
        0 => "Today".into(),
        1 => "Tomorrow".into(),
        -1 => "Yesterday".into(),
        n if n.abs() <= 6 => date.format("%A").to_string(),
        _ if date.year() == today.year() => date.format("%A, %b %-d").to_string(),
        _ => date.format("%A, %b %-d, %Y").to_string(),
    }
}

fn heading_ref(e: &Entry) -> HeadingRef {
    HeadingRef {
        path: e.note.record.path.clone(),
        index: e.note.record.heading,
        title: e.note.record.title.clone(),
    }
}

fn entry_row(e: &Entry, today: NaiveDate) -> Element<'_, Message> {
    let r = &e.note.record;
    let check: Element<'_, Message> = match e.completion() {
        Some(c) => {
            let how = match c {
                Completion::Task => Complete::Toggle,
                Completion::Advance(s) => Complete::Advance(s),
            };
            button(text("☐").size(16))
                .on_press(Message::Run(Op::Complete(heading_ref(e), how)))
                .padding([0, 4])
                .width(26)
                .style(button::text)
                .into()
        }
        None => space::horizontal().width(26).into(),
    };
    let mut title = row![].spacing(6).align_y(iced::Center);
    if let Some(k) = &r.keyword {
        title = title.push(chip(
            k.clone(),
            if r.done { Chip::Done } else { Chip::Todo },
        ));
    }
    if let Some(p) = &r.priority {
        title = title.push(chip(format!("#{p}"), Chip::Priority));
    }
    title = title.push(text(r.title.as_str()).size(14));
    let open = button(title)
        .on_press(Message::OpenHeading(r.path.clone(), r.heading))
        .padding([3, 6])
        .width(Fill)
        .style(style::row(false));
    let mut meta = row![].spacing(6).align_y(iced::Center);
    for (label, kind) in meta_chips(e, today) {
        meta = meta.push(chip(label, kind));
    }
    if !r.inherited_tags.is_empty() {
        meta = meta.push(chip(format!(":{}:", r.inherited_tags.join(":")), Chip::Tag));
    }
    meta = meta.push(text(r.path.as_str()).size(11).style(dim));
    container(
        row![
            check,
            column![open, meta.padding([0, 6])].spacing(2).width(Fill)
        ]
        .spacing(4)
        .align_y(iced::Top),
    )
    .width(Fill)
    .into()
}

/// The chips after a row's title: time, where it comes from, repeater.
fn meta_chips(e: &Entry, today: NaiveDate) -> Vec<(String, Chip)> {
    let mut out = Vec::new();
    let ts = &e.stamp;
    let short = |d: NaiveDate| {
        if d.year() == today.year() {
            d.format("%b %-d").to_string()
        } else {
            d.format("%b %-d, %Y").to_string()
        }
    };
    match e.placement {
        Placement::Scheduled | Placement::Deadline if ts.date < today => {
            let late = (today - ts.date).num_days();
            let flag = if e.placement == Placement::Deadline {
                "⚑ "
            } else {
                ""
            };
            out.push((
                format!("{flag}{} · {late}d late", short(ts.date)),
                Chip::Overdue,
            ));
        }
        Placement::Deadline | Placement::DeadlineRepeat => {
            out.push(("⚑ due".into(), Chip::Overdue));
        }
        Placement::DueSoon => {
            let left = (ts.date - today).num_days();
            out.push((format!("⚑ {} · in {left}d", short(ts.date)), Chip::Overdue));
        }
        Placement::Scheduled | Placement::Event | Placement::ScheduledRepeat => {}
    }
    if let Some(t) = time_range(ts) {
        out.push((t, Chip::Date));
    }
    if e.placement == Placement::Scheduled
        && let Some(d) = &e.note.deadline
    {
        out.push((format!("⚑ {}", short(d.date)), Chip::Overdue));
    }
    if let Some(r) = ts.repeater.filter(|_| ts.repeats()) {
        out.push((format!("↻ {}", repeater_text(r)), Chip::Date));
    }
    out
}

/// `09:00` or `09:00–10:30`.
fn time_range(ts: &Timestamp) -> Option<String> {
    let start = ts.time?.format("%H:%M").to_string();
    Some(match ts.end_time {
        Some(end) => format!("{start}–{}", end.format("%H:%M")),
        None => start,
    })
}

/// `+1w`, `++2d`, `.+1m`.
fn repeater_text(r: Repeater) -> String {
    let mark = match r.kind {
        RepeaterKind::Cumulate => "+",
        RepeaterKind::CatchUp => "++",
        RepeaterKind::Restart => ".+",
    };
    let unit = match r.unit {
        TimeUnit::Hour => 'h',
        TimeUnit::Day => 'd',
        TimeUnit::Week => 'w',
        TimeUnit::Month => 'm',
        TimeUnit::Year => 'y',
    };
    format!("{mark}{}{unit}", r.value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        let d = |m, day| NaiveDate::from_ymd_opt(2026, m, day).unwrap();
        let today = d(11, 23);
        assert_eq!(day_label(today, today), "Today");
        assert_eq!(day_label(d(11, 24), today), "Tomorrow");
        assert_eq!(day_label(d(11, 27), today), "Friday");
        assert_eq!(day_label(d(12, 4), today), "Friday, Dec 4");
        let ts = Timestamp::parse("<2026-11-23 Mon 09:00-10:30 .+2w>").unwrap();
        assert_eq!(time_range(&ts).unwrap(), "09:00–10:30");
        assert_eq!(repeater_text(ts.repeater.unwrap()), ".+2w");
    }
}
