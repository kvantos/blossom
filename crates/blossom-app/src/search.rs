//! The search screen: a query field (plain words, or Orgzly filters such as
//! `i.todo t.work`), then the matching headings grouped by file, in the
//! index's rank order. Plain words also find a file's intro (the text
//! before its first heading, or all of a file without headings), shown
//! first in its file's group. A result found by its body shows the line
//! that matched. Clicking one opens it.
//!
//! The "By meaning" switch ranks headings by meaning instead (see
//! [`crate::meaning`]): one list, closest first, each with its file. Above
//! it, what that needs or how far it is: the model download, reading the
//! headings, or what went wrong.

use blossom_index::{Note, Query};
use iced::widget::{
    Column, button, column, progress_bar, row, scrollable, text, text_input, toggler,
};
use iced::{Element, Fill};

use crate::app::{Message, bold, chip, dim};
use crate::meaning::Status;
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

/// Examples shown while the field is empty, by meaning.
const MEANING_EXAMPLES: [(&str, &str); 3] = [
    (
        "ideas for a birthday present",
        "finds gift notes even without those words",
    ),
    ("what to cook this week", "recipes, meal plans, shopping"),
    ("t.work how did the budget talks go", "filters still narrow"),
];

/// The whole search pane. `results` are for `input` and the switch (stale
/// ones are dropped before they get here); `None` while waiting for the
/// first. `status` is how search by meaning is getting on.
pub fn view<'a>(
    input: &'a str,
    meaning: bool,
    status: Option<&'a Status>,
    results: Option<&'a SearchResults>,
) -> Element<'a, Message> {
    let field = text_input(
        if meaning {
            "Describe what you're looking for…"
        } else {
            "Search your notes…"
        },
        input,
    )
    .id(INPUT)
    .on_input(Message::SearchInput)
    .size(15)
    .padding([6, 10]);
    let switch = toggler(meaning)
        .label("By meaning")
        .on_toggle(Message::SearchMeaning)
        .text_size(13)
        .size(18);
    let mut parts: Vec<Element<'a, Message>> =
        vec![row![field, switch].spacing(14).align_y(iced::Center).into()];
    if meaning {
        meaning_view(input, status, results, &mut parts);
    } else if input.trim().is_empty() {
        examples(
            "Type words to find headings, or filter:",
            &EXAMPLES,
            &mut parts,
        );
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

fn examples<'a>(lead: &'a str, list: &[(&'a str, &'a str)], parts: &mut Vec<Element<'a, Message>>) {
    parts.push(text(lead).size(13).style(dim).into());
    for &(q, what) in list {
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
}

/// The "By meaning" pane below the field.
fn meaning_view<'a>(
    input: &'a str,
    status: Option<&'a Status>,
    results: Option<&'a SearchResults>,
    parts: &mut Vec<Element<'a, Message>>,
) {
    if let Some(s) = status
        && let Some(panel) = status_view(s)
    {
        parts.push(panel);
    }
    if input.trim().is_empty() {
        examples(
            "Describe what you're looking for in your own words:",
            &MEANING_EXAMPLES,
            parts,
        );
        return;
    }
    if Query::parse(input).split_text().0.is_empty() {
        parts.push(
            text("Add some words to match by meaning; filters such as t.work only narrow.")
                .size(13)
                .style(dim)
                .into(),
        );
        return;
    }
    let Some(r) = results else { return };
    if r.notes.is_empty() {
        // While reading, the status above says so; results follow.
        if !matches!(status, Some(Status::Loading | Status::Embedding { .. })) {
            parts.push(text("Nothing found").size(13).style(dim).into());
        }
        return;
    }
    parts.push(text("Closest first").size(13).style(dim).into());
    for n in &r.notes {
        parts.push(meaning_row(n));
    }
}

/// What search by meaning needs or is doing; `None` once it is ready.
pub fn status_view(status: &Status) -> Option<Element<'_, Message>> {
    let note = |s: String| text(s).size(13).style(dim);
    let panel: Element<'_, Message> = match status {
        Status::Ready => return None,
        Status::NeedsDownload { model, mb } => column![
            text(format!(
                "Search by meaning finds notes about the same thing, even when they \
                 use other words. It needs the {model} model, a one-time download of \
                 {mb} MB; after that everything runs on this computer."
            ))
            .size(13),
            button(text(format!("Download ({mb} MB)")).size(13))
                .on_press(Message::DownloadModel)
                .padding([4, 12])
                .style(button::primary),
        ]
        .spacing(8)
        .into(),
        Status::Downloading { done, total } => column![
            row![
                note(format!("Downloading the model: {done} of {total} MB")),
                button(text("Cancel").size(12))
                    .on_press(Message::CancelDownload)
                    .padding([2, 8])
                    .style(button::secondary),
            ]
            .spacing(10)
            .align_y(iced::Center),
            bar(*done, *total),
        ]
        .spacing(6)
        .into(),
        Status::Loading => note("Loading the model…".into()).into(),
        Status::Embedding { done, total } => column![
            note(format!(
                "Reading your notes for meaning: {done} of {total} headings. \
                 Results get better as it goes."
            )),
            bar(
                u64::try_from(*done).unwrap_or(u64::MAX),
                u64::try_from(*total).unwrap_or(u64::MAX)
            ),
        ]
        .spacing(6)
        .into(),
        Status::Failed(e) => column![
            text(format!("Search by meaning isn't working: {e}")).size(13),
            button(text("Try again").size(13))
                .on_press(Message::RetryMeaning)
                .padding([4, 12])
                .style(button::secondary),
        ]
        .spacing(8)
        .into(),
    };
    Some(panel)
}

/// A thin progress bar at `done` of `total`.
fn bar<'a>(done: u64, total: u64) -> Element<'a, Message> {
    let total = total.max(1);
    let permille = u16::try_from(done.min(total) * 1000 / total).unwrap_or(1000);
    progress_bar(0.0..=1000.0, f32::from(permille))
        .girth(6)
        .length(Fill)
        .into()
}

/// A result by meaning: the heading, its file, and its first body line.
fn meaning_row(n: &Note) -> Element<'_, Message> {
    let r = &n.record;
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
    if !r.tags.is_empty() {
        title = title.push(chip(format!(":{}:", r.tags.join(":")), Chip::Tag));
    }
    title = title.push(text(r.path.as_str()).size(12).style(dim));
    let mut body = column![title].spacing(2);
    if let Some(line) = first_line(&r.text) {
        body = body.push(text(line).size(12).style(dim));
    }
    button(body)
        .on_press(Message::OpenHeading(r.path.clone(), r.heading))
        .padding([3, 6])
        .width(Fill)
        .style(style::row(false))
        .into()
}

/// The first non-blank body line (`text` is the title, a newline, then the
/// body), cut to [`SNIPPET`] characters.
fn first_line(text: &str) -> Option<String> {
    let line = text
        .split_once('\n')?
        .1
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())?;
    if line.chars().count() <= SNIPPET {
        Some(line.to_string())
    } else {
        Some(format!(
            "{}…",
            line.chars().take(SNIPPET).collect::<String>()
        ))
    }
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
