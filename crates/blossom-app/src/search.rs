//! The search screen: a query field (plain words, or Orgzly filters such as
//! `i.todo t.work`), then the matching headings grouped by file, in the
//! index's rank order. Plain words also find a file's intro (the text
//! before its first heading, or all of a file without headings), shown
//! first in its file's group. A result found by its body shows the line
//! that matched. Clicking one opens it.

use blossom_index::{Note, Query};
use iced::widget::{Column, button, column, row, scrollable, text, text_input};
use iced::{Element, Fill};

use crate::app::{Message, bold, chip, dim};
use crate::style::{self, Chip};
use crate::worker::SearchResults;

/// The query field's widget id, for focusing.
pub const INPUT: &str = "search-input";

/// Results shown at most; the rest are counted.
const LIMIT: usize = 200;

/// Examples shown while the field is empty.
const EXAMPLES: [(&str, &str); 5] = [
    ("meeting notes", "headings containing both words"),
    ("i.todo", "tasks in the TODO state"),
    ("t.work", "headings tagged work"),
    ("s.today", "scheduled for today or earlier"),
    ("i.todo t.work", "both: work tasks"),
];

/// The whole search pane. `results` are for `input` (stale ones are
/// dropped before they get here); `None` while waiting for the first.
pub fn view<'a>(input: &'a str, results: Option<&'a SearchResults>) -> Element<'a, Message> {
    let field = text_input("Search your notes…", input)
        .id(INPUT)
        .on_input(Message::SearchInput)
        .size(15)
        .padding([6, 10]);
    let mut parts: Vec<Element<'a, Message>> = vec![field.into()];
    if input.trim().is_empty() {
        parts.push(
            text("Type words to find headings, or filter:")
                .size(13)
                .style(dim)
                .into(),
        );
        for (q, what) in EXAMPLES {
            parts.push(
                row![
                    button(text(q).size(13).font(iced::Font::MONOSPACE))
                        .on_press(Message::SearchInput(q.into()))
                        .padding([2, 6])
                        .style(button::text),
                    text(what).size(13).style(dim),
                ]
                .spacing(8)
                .align_y(iced::Center)
                .into(),
            );
        }
    } else if let Some(r) = results {
        results_view(r, &mut parts);
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

fn results_view<'a>(r: &'a SearchResults, parts: &mut Vec<Element<'a, Message>>) {
    let groups = group(&r.notes[..r.notes.len().min(LIMIT)]);
    let files = group(&r.notes).len();
    parts.push(
        text(match r.notes.len() {
            0 => "Nothing found".into(),
            n => format!(
                "{n} {} in {files} {}",
                if n == 1 { "result" } else { "results" },
                if files == 1 { "file" } else { "files" }
            ),
        })
        .size(13)
        .style(dim)
        .into(),
    );
    let query = Query::parse(&r.query);
    let terms: Vec<String> = query
        .text_terms()
        .iter()
        .map(|t| t.to_lowercase())
        .collect();
    for (path, notes) in groups {
        parts.push(text(path).size(12).font(bold()).style(dim).into());
        for n in notes {
            parts.push(note_row(n, &terms));
        }
    }
    if r.notes.len() > LIMIT {
        parts.push(
            text(format!(
                "and {} more; add words or filters to narrow it down",
                r.notes.len() - LIMIT
            ))
            .size(13)
            .style(dim)
            .into(),
        );
    }
}

/// Notes grouped by file, files in order of their best-ranked note, a
/// file's intro first in its group.
fn group(notes: &[Note]) -> Vec<(&str, Vec<&Note>)> {
    let mut out: Vec<(&str, Vec<&Note>)> = Vec::new();
    for n in notes {
        let path = n.record.path.as_str();
        match out.iter_mut().find(|(p, _)| *p == path) {
            Some((_, v)) if n.record.is_intro() => v.insert(0, n),
            Some((_, v)) => v.push(n),
            None => out.push((path, vec![n])),
        }
    }
    out
}

fn note_row<'a>(n: &'a Note, terms: &[String]) -> Element<'a, Message> {
    let r = &n.record;
    let mut title = row![].spacing(6).align_y(iced::Center);
    if r.is_intro() {
        title = title.push(text("Top of file").size(12).style(dim));
    }
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
    if !r.tags.is_empty() {
        title = title.push(chip(format!(":{}:", r.tags.join(":")), Chip::Tag));
    }
    let mut body = column![title].spacing(2);
    if let Some(line) = snippet(&r.text, terms) {
        body = body.push(text(line).size(12).style(dim));
    }
    button(body)
        .on_press(if r.is_intro() {
            Message::OpenIntro(r.path.clone())
        } else {
            Message::OpenHeading(r.path.clone(), r.heading)
        })
        .padding([3, 6])
        .width(Fill)
        .style(style::row(false))
        .into()
}

/// Width of a snippet, in characters.
const SNIPPET: usize = 110;

/// The first body line (`text` is the title, a newline, then the body)
/// containing one of `terms` (lowercase), cut to [`SNIPPET`] characters
/// around the match. `None` when only the title matched.
fn snippet(text: &str, terms: &[String]) -> Option<String> {
    let body = text.split_once('\n')?.1;
    body.lines().find_map(|line| {
        let lower = line.to_lowercase();
        let at = terms.iter().filter_map(|t| lower.find(t.as_str())).min()?;
        let line = line.trim();
        let chars = line.chars().count();
        if chars <= SNIPPET {
            return Some(line.to_string());
        }
        // Byte offset in the lowercase line → characters, near enough.
        let pos = lower[..at].chars().count();
        let start = pos.saturating_sub(SNIPPET / 3).min(chars - SNIPPET);
        let cut: String = line.chars().skip(start).take(SNIPPET).collect();
        let pre = if start > 0 { "…" } else { "" };
        let post = if start + SNIPPET < chars { "…" } else { "" };
        Some(format!("{pre}{cut}{post}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intros_lead_their_file_group() {
        let note = |path: &str, level: usize, title: &str| {
            Note::from(blossom_index::NoteRecord {
                path: path.into(),
                heading: 0,
                line: 1,
                level,
                title: title.into(),
                keyword: None,
                done: false,
                priority: None,
                tags: Vec::new(),
                inherited_tags: Vec::new(),
                scheduled: None,
                deadline: None,
                closed: None,
                created: None,
                events: Vec::new(),
                id: None,
                custom_id: None,
                modified: 0,
                text: String::new(),
            })
        };
        let notes = [
            note("a.org", 1, "A1"),
            note("b.org", 0, "B"),
            note("a.org", 0, "A"),
        ];
        let titles: Vec<(&str, Vec<&str>)> = group(&notes)
            .into_iter()
            .map(|(p, v)| (p, v.iter().map(|n| n.record.title.as_str()).collect()))
            .collect();
        assert_eq!(titles, [("a.org", vec!["A", "A1"]), ("b.org", vec!["B"])]);
    }

    #[test]
    fn snippets_show_the_matching_body_line() {
        let terms = vec!["milk".to_string()];
        assert_eq!(
            snippet("Shopping\nbread\n  Buy MILK today\n", &terms).as_deref(),
            Some("Buy MILK today")
        );
        // Title-only matches and term-less queries have none.
        assert_eq!(snippet("milk\nbread\n", &terms), None);
        assert_eq!(snippet("a\nmilk\n", &[]), None);
        // Long lines are cut around the match.
        let long = format!("t\n{}milk{}", "x".repeat(200), "y".repeat(200));
        let s = snippet(&long, &terms).unwrap();
        assert!(s.starts_with('…') && s.ends_with('…') && s.contains("milk"));
        assert_eq!(s.chars().count(), SNIPPET + 2);
    }
}
