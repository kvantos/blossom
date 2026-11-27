//! Mini calendars (the 3-month strip; Quarter and Year) and the month
//! grid. In a mini calendar a day with items gets dots in its files'
//! colours; clicking a day goes to it.

use blossom_index::calendar::Calendar;
use chrono::{Datelike, Days, NaiveDate};
use iced::widget::{Column, Row, button, column, container, space, text};
use iced::{Element, Fill, Theme};

use super::prefs::AgendaPrefs;
use super::{AgendaMsg, Scope, View, colour, month_end, month_start, week};
use crate::app::{Message, bold, dim};
use crate::style;

/// The weekday letters (`S M T W T F S`) from the week's first day.
fn letters(first: NaiveDate) -> Vec<String> {
    (0..7)
        .map(|i| {
            (first + Days::new(i))
                .format("%a")
                .to_string()
                .chars()
                .next()
                .map_or_else(String::new, String::from)
        })
        .collect()
}

/// How many items a day of the month grid shows.
pub const SHOWN: usize = 3;

/// Every day the month grid draws for `month`: whole weeks.
#[must_use]
pub fn grid_days(month: NaiveDate, prefs: &AgendaPrefs) -> Vec<NaiveDate> {
    weeks(month, prefs)
        .into_iter()
        .flat_map(|w| (0..7).map(move |i| w + Days::new(i)))
        .collect()
}

/// The weeks covering `month`'s days, as their first days.
fn weeks(month: NaiveDate, prefs: &AgendaPrefs) -> Vec<NaiveDate> {
    let mut out = Vec::new();
    let mut w = prefs.week_of(month_start(month));
    while w <= month_end(month) {
        out.push(w);
        w = w + Days::new(7);
    }
    out
}

/// The distinct files with items on `d`, at most three.
fn files_on(cal: &Calendar, d: NaiveDate) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for i in cal.on(d) {
        if !out.contains(&i.note.record.path) {
            out.push(i.note.record.path.clone());
        }
        if out.len() == 3 {
            break;
        }
    }
    out
}

/// One mini month with cells `cell` pixels wide; clicking a day shows it
/// (in `scope`, else the current one).
fn mini<'a>(
    month: NaiveDate,
    cal: &'a Calendar,
    v: &View<'a>,
    cell: f32,
    scope: Option<Scope>,
) -> Element<'a, Message> {
    let (from, to) = v.place.shown(v.prefs);
    let in_view = v.place.scope != Scope::Year && v.place.scope != Scope::Quarter;
    let weeks = weeks(month, v.prefs);
    let first = weeks.first().copied().unwrap_or(month);
    let head = Row::from_iter(letters(first).into_iter().map(|l| {
        container(text(l).size(10).style(dim))
            .width(cell)
            .center_x(cell)
            .into()
    }));
    let mut rows: Vec<Element<'a, Message>> = vec![
        text(month.format("%B %Y").to_string())
            .size(12)
            .font(bold())
            .into(),
        head.into(),
    ];
    for w in weeks {
        rows.push(
            Row::from_iter((0..7).map(|i| {
                let d = w + Days::new(i);
                if d.month() != month.month() {
                    return space().width(cell).height(cell * 0.92).into();
                }
                day_cell(d, cal, v, cell, scope, in_view && (from..=to).contains(&d))
            }))
            .into(),
        );
    }
    Column::with_children(rows).spacing(2).into()
}

fn day_cell<'a>(
    d: NaiveDate,
    cal: &'a Calendar,
    v: &View<'a>,
    cell: f32,
    scope: Option<Scope>,
    in_view: bool,
) -> Element<'a, Message> {
    let today = d == v.today;
    let num = text(d.day().to_string()).size(if cell > 32.0 { 12 } else { 11 });
    let num = if today {
        num.font(bold()).style(|theme: &Theme| text::Style {
            color: Some(theme.palette().primary),
        })
    } else {
        num
    };
    let dots = Row::from_iter(files_on(cal, d).into_iter().map(|path| {
        let slot = v.slot(&path);
        container(space())
            .width(4)
            .height(4)
            .style(move |theme: &Theme| container::Style {
                background: Some(colour(theme, slot).into()),
                border: iced::Border {
                    radius: 2.0.into(),
                    ..iced::Border::default()
                },
                ..container::Style::default()
            })
            .into()
    }))
    .spacing(2)
    .height(4);
    button(
        column![num, dots]
            .spacing(1)
            .align_x(iced::Center)
            .width(Fill),
    )
    .on_press(Message::Agenda(AgendaMsg::Jump(d, scope)))
    .width(cell)
    .height(cell * 0.92)
    .padding([2, 0])
    .style(move |theme: &Theme, status| {
        let p = theme.extended_palette();
        let mut s = button::text(theme, status);
        if in_view {
            s.background =
                Some(style::mix(p.background.base.color, p.primary.base.color, 0.1).into());
        }
        if today {
            s.border = iced::Border {
                color: p.primary.base.color,
                width: 1.5,
                radius: 6.0.into(),
            };
        } else {
            s.border.radius = 6.0.into();
        }
        s.text_color = p.background.base.text;
        s
    })
    .into()
}

/// The strip of three mini months above Week and Month.
pub fn strip<'a>(months: [NaiveDate; 3], cal: &'a Calendar, v: &View<'a>) -> Element<'a, Message> {
    container(Row::from_iter(months.map(|m| mini(m, cal, v, 24.0, None))).spacing(28))
        .center_x(Fill)
        .padding([10, 14])
        .into()
}

/// Quarter (three larger mini months) and Year (twelve).
pub fn many<'a>(cal: &'a Calendar, v: &View<'a>) -> Element<'a, Message> {
    let (from, _) = v.place.shown(v.prefs);
    let (count, cell) = if v.place.scope == Scope::Quarter {
        (3, 34.0)
    } else {
        (12, 26.0)
    };
    // As many months to a row as fit.
    let months = Row::from_iter((0..count).map(|i| {
        mini(
            from + chrono::Months::new(i),
            cal,
            v,
            cell,
            Some(Scope::Week),
        )
    }))
    .spacing(28)
    .wrap()
    .vertical_spacing(22);
    container(months).center_x(Fill).padding([16, 14]).into()
}

/// The month grid: a cell per day with its items.
pub fn grid<'a>(cal: &'a Calendar, v: &View<'a>) -> Element<'a, Message> {
    let (from, _) = v.place.shown(v.prefs);
    let weeks = weeks(from, v.prefs);
    let first = weeks.first().copied().unwrap_or(from);
    let head = Row::from_iter((0..7).map(|i| {
        container(
            text((first + Days::new(i)).format("%a").to_string())
                .size(12)
                .style(dim),
        )
        .width(Fill)
        .padding([4, 6])
        .into()
    }));
    let mut rows: Vec<Element<'a, Message>> = vec![head.into()];
    for w in weeks {
        rows.push(
            Row::from_iter((0..7).map(|i| {
                let d = w + Days::new(i);
                let outside = d.month() != from.month();
                let today = d == v.today;
                let num = text(d.day().to_string()).size(13);
                let num = if today {
                    num.font(bold()).style(|theme: &Theme| text::Style {
                        color: Some(theme.palette().primary),
                    })
                } else if outside {
                    num.style(dim)
                } else {
                    num
                };
                let items = cal.on(d);
                let mut parts: Vec<Element<'a, Message>> = vec![
                    button(num)
                        .on_press(Message::Agenda(AgendaMsg::Jump(d, Some(Scope::Week))))
                        .padding([0, 4])
                        .style(button::text)
                        .into(),
                ];
                parts.extend(items.iter().take(SHOWN).map(|i| week::chip(i, d, v)));
                if items.len() > SHOWN {
                    parts.push(
                        text(format!("+{} more", items.len() - SHOWN))
                            .size(11)
                            .style(dim)
                            .into(),
                    );
                }
                container(Column::with_children(parts).spacing(1))
                    .width(Fill)
                    .height(108)
                    .padding(3)
                    .clip(true)
                    .style(move |theme: &Theme| {
                        let p = theme.extended_palette();
                        let bg = p.background.base.color;
                        container::Style {
                            background: Some(
                                if outside {
                                    style::mix(bg, p.background.strong.color, 0.15)
                                } else {
                                    bg
                                }
                                .into(),
                            ),
                            border: iced::Border {
                                color: if today {
                                    p.primary.base.color
                                } else {
                                    style::mix(bg, p.background.strong.color, 0.6)
                                },
                                width: if today { 1.5 } else { 0.5 },
                                radius: 0.0.into(),
                            },
                            ..container::Style::default()
                        }
                    })
                    .into()
            }))
            .into(),
        );
    }
    container(Column::with_children(rows))
        .padding([8, 14])
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekday_letters_start_on_the_week_start() {
        // 22 Nov 2026 is a Sunday.
        let sun = NaiveDate::from_ymd_opt(2026, 11, 22).unwrap();
        assert_eq!(letters(sun).join(""), "SMTWTFS");
        assert_eq!(letters(sun + Days::new(1)).join(""), "MTWTFSS");
    }
}
