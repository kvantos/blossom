//! The week: a row of untimed items ("Due & planned"), then a 24-hour
//! grid that opens scrolled to the working hours, shaded outside them.
//! Extra time zones get their own hour column beside the local one, as in
//! Outlook. A line marks the time now in today's column.

use std::fmt::Write as _;

use blossom_index::calendar::{Calendar, Item, Kind};
use chrono::{Days, NaiveDate, Offset, TimeZone, Timelike};
use iced::widget::{Column, Id, Row, column, container, row, scrollable, space, stack, text};
use iced::{Element, Fill, Length, Theme};

use super::{View, card, check_box, colour, dot_for, hoverable, prefs};
use crate::app::{Message, bold, dim};
use crate::style;

/// The hour grid's scrollable, to open it on the working hours.
pub const GRID: &str = "agenda-week-grid";

/// The height of one hour.
pub const HOUR: f32 = 40.0;

/// The width of each hour-label column.
const GUTTER: f32 = 58.0;

/// The id of the hour grid.
#[must_use]
pub fn grid_id() -> Id {
    Id::new(GRID)
}

/// `GMT+9`, `GMT-5`, `GMT+5:30`, `GMT`.
fn gmt(seconds: i32) -> String {
    if seconds == 0 {
        return "GMT".into();
    }
    let sign = if seconds < 0 { '-' } else { '+' };
    let m = seconds.unsigned_abs() / 60;
    if m.is_multiple_of(60) {
        format!("GMT{sign}{}", m / 60)
    } else {
        format!("GMT{sign}{}:{:02}", m / 60, m % 60)
    }
}

/// The day the hour labels are worked out for: today in this week, else
/// its first day.
fn reference(from: NaiveDate, today: NaiveDate) -> NaiveDate {
    if (from..from + Days::new(7)).contains(&today) {
        today
    } else {
        from
    }
}

/// One zone's hour labels: `04:00`, with the day under it where the day
/// there changes.
fn zone_column<'a>(tz: Option<chrono_tz::Tz>, day: NaiveDate) -> Element<'a, Message> {
    let cells = (0..24).map(|h| {
        let label = match tz {
            None => (format!("{h:02}:00"), None),
            Some(tz) => match chrono::Local
                .from_local_datetime(&day.and_hms_opt(h, 0, 0).unwrap_or_default())
                .earliest()
            {
                Some(local) => {
                    let there = local.with_timezone(&tz);
                    let changes = there.hour() == 0 || (h == 0 && there.date_naive() != day);
                    (
                        there.format("%H:%M").to_string(),
                        changes.then(|| there.format("%a %-d").to_string()),
                    )
                }
                None => (String::new(), None),
            },
        };
        let mut c = column![text(label.0).size(11).style(dim)].align_x(iced::Right);
        if let Some(d) = label.1 {
            c = c.push(text(d).size(10).style(|theme: &Theme| text::Style {
                color: Some(theme.palette().primary),
            }));
        }
        container(c)
            .width(GUTTER)
            .height(HOUR)
            .padding(iced::Padding::default().right(6).top(2))
            .into()
    });
    Column::from_iter(cells).into()
}

/// The header of a zone column: city and offset.
fn zone_header<'a>(tz: Option<chrono_tz::Tz>, day: NaiveDate) -> Element<'a, Message> {
    let noon = day.and_hms_opt(12, 0, 0).unwrap_or_default();
    let (name, offset) = match tz {
        None => {
            let off = chrono::Local
                .from_local_datetime(&noon)
                .earliest()
                .map_or(0, |d| d.offset().fix().local_minus_utc());
            ("Here".to_string(), gmt(off))
        }
        Some(tz) => {
            let off = chrono::Local
                .from_local_datetime(&noon)
                .earliest()
                .map_or(0, |d| d.with_timezone(&tz).offset().fix().local_minus_utc());
            (prefs::city(tz), gmt(off))
        }
    };
    container(
        column![
            text(name)
                .size(11)
                .font(bold())
                .wrapping(text::Wrapping::WordOrGlyph),
            text(offset).size(10).style(dim),
        ]
        .align_x(iced::Right),
    )
    .width(GUTTER)
    .padding(iced::Padding::default().right(6))
    .into()
}

/// The week view.
pub fn view<'a>(cal: &'a Calendar, v: &View<'a>) -> Element<'a, Message> {
    let (from, _) = v.place.shown(v.prefs);
    let days: Vec<NaiveDate> = (0..7).map(|i| from + Days::new(i)).collect();
    let refday = reference(from, v.today);
    let zones: Vec<Option<chrono_tz::Tz>> = v
        .prefs
        .zones
        .iter()
        .copied()
        .map(Some)
        .chain([None])
        .collect();
    let gutter = GUTTER * f32::from(u16::try_from(zones.len()).unwrap_or(1));

    let header = Row::from_iter(
        zones
            .iter()
            .map(|z| zone_header(*z, refday))
            .chain(days.iter().map(|d| day_header(*d, v.today))),
    )
    .align_y(iced::alignment::Vertical::Bottom)
    .padding([6, 0]);

    let all_day = row![
        container(text("Due & planned").size(11).style(dim))
            .width(gutter)
            .padding(iced::Padding::default().right(6).top(4))
            .align_x(iced::Right),
        Row::from_iter(days.iter().map(|d| untimed(cal, *d, v))),
    ];

    let grid = Row::from_iter(
        zones
            .iter()
            .map(|z| zone_column(*z, refday))
            .chain(days.iter().map(|d| day_column(cal, *d, v))),
    );
    // The working hours fit; the rest is a scroll away.
    let (w0, w1) = v.prefs.work;
    #[allow(clippy::cast_precision_loss)]
    let shown = (w1 - w0).clamp(8, 14) as f32 + 0.5;
    column![
        header,
        all_day,
        scrollable(grid).id(grid_id()).height(shown * HOUR)
    ]
    .padding(iced::Padding::default().left(8).right(14).bottom(10))
    .into()
}

fn day_header<'a>(d: NaiveDate, today: NaiveDate) -> Element<'a, Message> {
    let is_today = d == today;
    let num = text(d.format("%-d").to_string()).size(18).font(bold());
    let num = if is_today {
        num.style(|theme: &Theme| text::Style {
            color: Some(theme.palette().primary),
        })
    } else {
        num
    };
    container(
        row![text(d.format("%a").to_string()).size(12).style(dim), num]
            .spacing(5)
            .align_y(iced::Center),
    )
    .width(Fill)
    .padding([0, 8])
    .into()
}

/// The untimed items of a day, as chips.
/// How many untimed items a day shows in the week.
pub const UNTIMED_SHOWN: usize = 4;

fn untimed<'a>(cal: &'a Calendar, d: NaiveDate, v: &View<'a>) -> Element<'a, Message> {
    const SHOWN: usize = UNTIMED_SHOWN;
    let items: Vec<&Item> = cal.on(d).iter().filter(|i| i.time().is_none()).collect();
    let mut chips: Vec<Element<'a, Message>> =
        items.iter().take(SHOWN).map(|i| chip(i, d, v)).collect();
    if items.len() > SHOWN {
        chips.push(
            text(format!("+{} more", items.len() - SHOWN))
                .size(11)
                .style(dim)
                .into(),
        );
    }
    container(Column::with_children(chips).spacing(2))
        .width(Fill)
        .padding([3, 3])
        .style(move |theme: &Theme| shade(theme, false))
        .into()
}

/// A one-line item: dot, tag, time, title.
pub fn chip<'a>(i: &'a Item, d: NaiveDate, v: &View<'a>) -> Element<'a, Message> {
    let mut parts: Vec<Element<'a, Message>> = Vec::new();
    parts.extend(check_box(i));
    let slot = v.slot(&i.note.record.path);
    parts.push(dot_for(slot));
    let tag = match (i.late, i.kind) {
        (Some(_), _) => Some("LATE"),
        (None, Kind::Deadline) => Some("DUE"),
        _ => None,
    };
    if let Some(tag) = tag {
        parts.push(
            text(tag)
                .size(9)
                .font(bold())
                .style(|theme: &Theme| text::Style {
                    color: Some(theme.extended_palette().danger.base.color),
                })
                .into(),
        );
    }
    if let Some(t) = i.time() {
        parts.push(
            text(t.format("%H:%M").to_string())
                .size(11)
                .style(dim)
                .into(),
        );
    }
    let title = text(super::plain_title(&i.note.record.title))
        .size(11)
        .wrapping(text::Wrapping::None);
    parts.push(if i.note.record.done {
        title.style(dim).into()
    } else {
        title.into()
    });
    let focused = v.item_focused(d, i);
    let line = container(Row::with_children(parts).spacing(4).align_y(iced::Center))
        .padding([1, 4])
        .clip(true)
        .width(Fill)
        .style(move |theme: &Theme| container::Style {
            border: if focused {
                super::focus_ring(theme)
            } else {
                iced::Border::default()
            },
            ..container::Style::default()
        });
    hoverable(line, card(i, d, &v.prefs.zones, slot), &i.note)
}

/// Background for an hour slot: lighter in working hours.
fn shade(theme: &Theme, off: bool) -> container::Style {
    let p = theme.extended_palette();
    let bg = p.background.base.color;
    let line = p.background.strong.color;
    container::Style {
        background: Some(if off { style::mix(bg, line, 0.18) } else { bg }.into()),
        border: iced::Border {
            color: style::mix(bg, line, 0.6),
            width: 0.5,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

/// A timed item's start and end, as hours.
fn hours(i: &Item) -> Option<(f32, f32)> {
    let t = i.time()?;
    let h = |t: chrono::NaiveTime| {
        #[allow(clippy::cast_precision_loss)]
        let v = t.hour() as f32 + t.minute() as f32 / 60.0;
        v
    };
    let start = h(t);
    let end = i
        .stamp
        .end_time
        .map(h)
        .filter(|e| *e > start)
        .unwrap_or(start + 1.0);
    Some((start, end.max(start + 0.5).min(24.0)))
}

/// Side-by-side lanes for overlapping items: (lane, lanes in its group).
fn lanes(spans: &[(f32, f32)]) -> Vec<(u16, u16)> {
    let mut out = vec![(0u16, 1u16); spans.len()];
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by(|a, b| spans[*a].0.total_cmp(&spans[*b].0));
    let mut group: Vec<usize> = Vec::new();
    let mut ends: Vec<f32> = Vec::new();
    let mut group_end = f32::MIN;
    let close = |group: &mut Vec<usize>, ends: &mut Vec<f32>, out: &mut Vec<(u16, u16)>| {
        let n = u16::try_from(ends.len()).unwrap_or(1).max(1);
        for i in group.drain(..) {
            out[i].1 = n;
        }
        ends.clear();
    };
    for i in order {
        let (s, e) = spans[i];
        if s >= group_end && !group.is_empty() {
            close(&mut group, &mut ends, &mut out);
        }
        let lane = ends.iter().position(|end| *end <= s).unwrap_or(ends.len());
        if lane == ends.len() {
            ends.push(e);
        } else {
            ends[lane] = e;
        }
        out[i].0 = u16::try_from(lane).unwrap_or(0);
        group.push(i);
        group_end = if group.len() == 1 {
            e
        } else {
            group_end.max(e)
        };
    }
    close(&mut group, &mut ends, &mut out);
    out
}

fn portion<'a>(n: u16) -> Option<Element<'a, Message>> {
    (n > 0).then(|| space().width(Length::FillPortion(n)).into())
}

/// A day's column: the hour slots, the timed items over them, and the
/// line for now in today's column.
fn day_column<'a>(cal: &'a Calendar, d: NaiveDate, v: &View<'a>) -> Element<'a, Message> {
    let (w0, w1) = v.prefs.work;
    let slots = Column::from_iter((0..24).map(|h| {
        let off = h < w0 || h >= w1;
        container(space())
            .width(Fill)
            .height(HOUR)
            .style(move |theme: &Theme| shade(theme, off))
            .into()
    }));
    let mut layers: Vec<Element<'a, Message>> = vec![slots.into()];
    let timed: Vec<(&Item, (f32, f32))> = cal
        .on(d)
        .iter()
        .filter_map(|i| hours(i).map(|h| (i, h)))
        .collect();
    let spans: Vec<(f32, f32)> = timed.iter().map(|(_, h)| *h).collect();
    for ((item, (start, end)), (lane, n)) in timed.iter().zip(lanes(&spans)) {
        let mut cols: Vec<Element<'a, Message>> = Vec::new();
        cols.extend(portion(lane));
        cols.push(
            container(block(item, d, v, end - start))
                .width(Length::FillPortion(1))
                .padding([1, 2])
                .into(),
        );
        cols.extend(portion(n - lane - 1));
        layers.push(
            column![
                space().height(start * HOUR),
                Row::from_iter(cols).height((end - start) * HOUR)
            ]
            .into(),
        );
    }
    if d == v.today {
        layers.push(
            column![
                space().height((v.now * HOUR - 1.0).max(0.0)),
                container(space())
                    .height(2)
                    .width(Fill)
                    .style(|theme: &Theme| container::Style {
                        background: Some(theme.palette().primary.into()),
                        ..container::Style::default()
                    })
            ]
            .into(),
        );
    }
    stack(layers).width(Fill).height(HOUR * 24.0).into()
}

/// A timed item in the grid.
fn block<'a>(i: &'a Item, d: NaiveDate, v: &View<'a>, hours: f32) -> Element<'a, Message> {
    let slot = v.slot(&i.note.record.path);
    let mut title = Row::new().spacing(3).align_y(iced::Center);
    if let Some(b) = check_box(i) {
        title = title.push(b);
    }
    let title = title.push(
        text(super::plain_title(&i.note.record.title))
            .size(12)
            .font(bold()),
    );
    let mut body = column![title];
    if hours >= 0.9 {
        let mut time = i
            .time()
            .map_or_else(String::new, |t| t.format("%H:%M").to_string());
        if let Some(e) = i.stamp.end_time {
            let _ = write!(time, "–{}", e.format("%H:%M"));
        }
        body = body.push(text(time).size(11).style(dim));
    }
    let done = i.note.record.done;
    let focused = v.item_focused(d, i);
    let content = row![
        container(space())
            .width(3)
            .height(Fill)
            .style(move |theme: &Theme| container::Style {
                background: Some(colour(theme, slot).into()),
                ..container::Style::default()
            }),
        container(body).padding([2, 6]).width(Fill),
    ];
    let boxed = container(content)
        .width(Fill)
        .height(Fill)
        .clip(true)
        .style(move |theme: &Theme| {
            let bg = theme.palette().background;
            let alpha = if done { 0.55 } else { 1.0 };
            container::Style {
                background: Some(
                    style::mix(bg, colour(theme, slot), 0.2)
                        .scale_alpha(alpha)
                        .into(),
                ),
                border: if focused {
                    super::focus_ring(theme)
                } else {
                    iced::Border {
                        radius: 4.0.into(),
                        ..iced::Border::default()
                    }
                },
                ..container::Style::default()
            }
        });
    hoverable(boxed, card(i, d, &v.prefs.zones, slot), &i.note)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets() {
        assert_eq!(gmt(9 * 3600), "GMT+9");
        assert_eq!(gmt(-5 * 3600), "GMT-5");
        assert_eq!(gmt(19800), "GMT+5:30");
        assert_eq!(gmt(0), "GMT");
    }

    #[test]
    fn overlapping_items_share_the_width() {
        // 9–10 and 9:30–10:30 overlap; 11–12 stands alone.
        let got = lanes(&[(9.0, 10.0), (9.5, 10.5), (11.0, 12.0)]);
        assert_eq!(got, [(0, 2), (1, 2), (0, 1)]);
        // 9–10, 9–11, 10–11: the third reuses the first lane.
        let got = lanes(&[(9.0, 10.0), (9.0, 11.0), (10.0, 11.0)]);
        assert_eq!(got, [(0, 2), (1, 2), (0, 2)]);
    }
}
