//! The open file as the UI holds it: the working document (with unsaved
//! edits), the section editor, undo history, and what is known about the
//! file on disk.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use blossom_core::Document;
use blossom_core::editing::Merge;
use blossom_core::render::{self, Block, Span};
use blossom_vault::Revision;
use iced::widget::text_editor::{Content, Cursor, Position};

/// Undo keeps this many steps.
const UNDO_LIMIT: usize = 200;

/// Keystrokes closer together than this undo as one step.
const TYPING_GROUP: Duration = Duration::from_secs(1);

/// What the read view shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selected {
    /// Nothing yet.
    Nothing,
    /// The file's top: keywords, file drawer and intro.
    Intro,
    /// A heading.
    Heading(usize),
}

/// How a change joins the undo history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    /// Typing: joins the previous step while keys keep coming.
    Typing,
    /// Its own step.
    Single,
}

/// One undo step: the text and view before a change.
#[derive(Clone, Debug)]
pub struct Snapshot {
    text: String,
    selected: Selected,
    editor: Option<(Range<usize>, Cursor)>,
}

/// The section editor: a text editor over `range` of the document.
pub struct SectionEditor {
    /// The editor's text.
    pub content: Content,
    /// The bytes of the document it shows.
    pub range: Range<usize>,
}

impl SectionEditor {
    /// An editor over `range` of `doc`.
    #[must_use]
    pub fn new(doc: &Document, range: Range<usize>) -> SectionEditor {
        SectionEditor {
            content: Content::with_text(&doc.text()[range.clone()]),
            range,
        }
    }

    /// Shows `range` of `doc`, keeping the cursor where it was if possible.
    fn reload(&mut self, doc: &Document, range: Range<usize>, cursor: Cursor) {
        let text = &doc.text()[range.clone()];
        if self.content.text() != text {
            self.content = Content::with_text(text);
        }
        self.range = range;
        let cursor = clamp(&self.content, cursor);
        self.content.move_to(cursor);
    }
}

/// `cursor` moved inside `content`.
fn clamp(content: &Content, cursor: Cursor) -> Cursor {
    let fix = |p: Position| {
        let line = p.line.min(content.line_count().saturating_sub(1));
        let text = content
            .line(line)
            .map_or_else(String::new, |l| l.text.into_owned());
        let mut column = p.column.min(text.len());
        while !text.is_char_boundary(column) {
            column -= 1;
        }
        Position { line, column }
    };
    Cursor {
        position: fix(cursor.position),
        selection: cursor.selection.map(fix),
    }
}

/// What the UI knows about a file on disk, kept per path while it has
/// unsaved edits (also after the user moved on to another file).
#[derive(Clone, Debug)]
pub struct DiskState {
    /// The revision `base_text` was read or written at; `None` when the
    /// file doesn't exist.
    pub base: Option<Revision>,
    /// The disk text the working text derives from.
    pub base_text: Arc<String>,
    /// The latest working text, sent to be saved.
    pub latest: Arc<String>,
    /// A disk version that couldn't be merged with the unsaved edits.
    pub conflict: Option<DiskCopy>,
}

/// The disk side of a conflict.
#[derive(Clone, Debug)]
pub enum DiskCopy {
    /// Another program changed the file.
    Changed(Arc<Document>, Revision),
    /// The file was deleted.
    Deleted,
}

impl DiskState {
    /// A file whose working text is what's on disk.
    #[must_use]
    pub fn clean(base: Option<Revision>, text: &str) -> DiskState {
        let text = Arc::new(text.to_string());
        DiskState {
            base,
            base_text: Arc::clone(&text),
            latest: text,
            conflict: None,
        }
    }

    /// Whether some working text isn't on disk yet.
    #[must_use]
    pub fn dirty(&self) -> bool {
        !Arc::ptr_eq(&self.latest, &self.base_text) && self.latest != self.base_text
    }
}

/// The file shown in the outline.
pub struct OpenFile {
    /// Vault-relative path.
    pub path: String,
    /// The working document, unsaved edits included.
    pub doc: Option<Arc<Document>>,
    /// Rendered heading titles, by heading index.
    pub titles: Vec<Vec<Span>>,
    /// Folded headings.
    pub folded: HashSet<usize>,
    /// What the read view shows.
    pub selected: Selected,
    /// The read view of `selected`.
    pub blocks: Vec<Block>,
    /// The file was deleted or moved.
    pub missing: bool,
    /// The section editor, while editing.
    pub editor: Option<SectionEditor>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_typing: Option<Instant>,
    /// Metadata panel fields.
    pub inputs: Inputs,
}

/// The metadata panel's text fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    /// Tags, space-separated.
    pub tags: String,
    /// SCHEDULED.
    pub scheduled: String,
    /// DEADLINE.
    pub deadline: String,
}

impl OpenFile {
    /// A file whose content is still loading.
    #[must_use]
    pub fn loading(path: String) -> OpenFile {
        OpenFile {
            path,
            doc: None,
            titles: Vec::new(),
            folded: HashSet::new(),
            selected: Selected::Nothing,
            blocks: Vec::new(),
            missing: false,
            editor: None,
            undo: Vec::new(),
            redo: Vec::new(),
            last_typing: None,
            inputs: Inputs::default(),
        }
    }

    /// The state to restore when undoing the next change.
    #[must_use]
    pub fn snapshot(&self) -> Option<Snapshot> {
        Some(Snapshot {
            text: self.doc.as_ref()?.text().to_string(),
            selected: self.selected,
            editor: self
                .editor
                .as_ref()
                .map(|e| (e.range.clone(), e.content.cursor())),
        })
    }

    /// Records `before` for undo (unless it joins the current typing step)
    /// and shows `doc`, a local change.
    pub fn change(&mut self, before: Option<Snapshot>, doc: Document, group: Group) {
        let now = Instant::now();
        let joins = group == Group::Typing
            && self
                .last_typing
                .is_some_and(|t| now.duration_since(t) < TYPING_GROUP);
        if !joins && let Some(s) = before {
            self.undo.push(s);
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
        }
        self.last_typing = (group == Group::Typing).then_some(now);
        self.redo.clear();
        self.show(Arc::new(doc));
    }

    /// Steps back (or forward with `redo`); false when there is nothing to do.
    pub fn undo(&mut self, redo: bool) -> bool {
        let (Some(doc), Some(current)) = (self.doc.clone(), self.snapshot()) else {
            return false;
        };
        let step = if redo {
            self.redo.pop()
        } else {
            self.undo.pop()
        };
        let Some(step) = step else { return false };
        if redo {
            self.undo.push(current);
        } else {
            self.redo.push(current);
        }
        let restored = Document::parse(step.text, doc.keywords().clone());
        match (step.editor, self.editor.as_mut()) {
            (Some((range, cursor)), Some(ed)) if range.end <= restored.text().len() => {
                ed.reload(&restored, range, cursor);
            }
            (_, Some(_)) => self.editor = None,
            _ => {}
        }
        self.selected = step.selected;
        self.last_typing = None;
        self.show(Arc::new(restored));
        true
    }

    /// Whether there is something to undo / redo.
    #[must_use]
    pub fn history(&self) -> (bool, bool) {
        (!self.undo.is_empty(), !self.redo.is_empty())
    }

    /// Shows a disk version that replaces the working text (an outside
    /// change with nothing unsaved). Undo history is dropped: stepping back
    /// would silently revert the other program's edit.
    pub fn adopt(&mut self, doc: Arc<Document>) {
        if self.doc.as_ref().is_some_and(|d| d.text() == doc.text()) {
            return;
        }
        self.undo.clear();
        self.redo.clear();
        self.last_typing = None;
        if let Some(ed) = self.editor.as_mut() {
            let range = match self.selected {
                Selected::Heading(i) => doc.section_range(i),
                Selected::Intro => Some(doc.top_range()),
                Selected::Nothing => None,
            };
            match range {
                Some(r) => {
                    let cursor = ed.content.cursor();
                    ed.reload(&doc, r, cursor);
                }
                None => self.editor = None,
            }
        }
        self.show(doc);
    }

    /// Shows the result of merging the unsaved edits with an outside change.
    pub fn merged(&mut self, doc: Document, merge: &Merge) {
        self.undo.clear();
        self.redo.clear();
        self.last_typing = None;
        let start = self.selected_start();
        if let Some(ed) = self.editor.as_mut() {
            let range = merge.map(ed.range.start)..merge.map(ed.range.end);
            let cursor = ed.content.cursor();
            ed.reload(&doc, range, cursor);
        } else if let Some(start) = start {
            self.selected = doc
                .heading_at(merge.map(start))
                .map_or(Selected::Intro, Selected::Heading);
        }
        self.show(Arc::new(doc));
    }

    fn selected_start(&self) -> Option<usize> {
        let doc = self.doc.as_ref()?;
        match self.selected {
            Selected::Heading(i) => doc.heading(i).map(|h| h.spans.line.start),
            _ => None,
        }
    }

    /// Shows `doc`: titles, selection (following the editor), read view.
    pub fn show(&mut self, doc: Arc<Document>) {
        let old = self.doc.take();
        self.titles = doc
            .headings()
            .iter()
            .map(|h| {
                // Re-render only titles that changed.
                match old.as_ref().and_then(|o| o.heading(h.index)) {
                    Some(o) if o.title == h.title && h.index < self.titles.len() => {
                        std::mem::take(&mut self.titles[h.index])
                    }
                    _ => title_spans(&h.title),
                }
            })
            .collect();
        let before = self.selected;
        self.selected = if let Some(ed) = &self.editor {
            doc.heading_at(ed.range.start)
                .map_or(Selected::Intro, Selected::Heading)
        } else {
            match self.selected {
                Selected::Heading(i) if i < doc.headings().len() => Selected::Heading(i),
                Selected::Intro if has_top(&doc) => Selected::Intro,
                _ if doc.preamble().intro.is_some() => Selected::Intro,
                _ if !doc.headings().is_empty() => Selected::Heading(0),
                _ => Selected::Intro,
            }
        };
        self.folded.retain(|i| *i < doc.headings().len());
        self.doc = Some(doc);
        self.missing = false;
        self.refresh_blocks();
        if before != self.selected || self.editor.is_none() {
            self.fill_inputs();
        }
    }

    /// Re-renders the read view.
    pub fn refresh_blocks(&mut self) {
        let Some(doc) = &self.doc else { return };
        self.blocks = match self.selected {
            Selected::Nothing => Vec::new(),
            Selected::Intro => doc.intro_text().map_or_else(Vec::new, render::blocks),
            Selected::Heading(i) => render::blocks(doc.body_text(i)),
        };
    }

    /// Fills the metadata panel from the selected heading.
    pub fn fill_inputs(&mut self) {
        let h = match (self.selected, &self.doc) {
            (Selected::Heading(i), Some(doc)) => doc.heading(i),
            _ => None,
        };
        self.inputs = h.map_or_else(Inputs::default, |h| Inputs {
            tags: h.tags.join(" "),
            scheduled: h
                .planning
                .scheduled
                .as_ref()
                .map_or_else(String::new, blossom_core::Timestamp::format),
            deadline: h
                .planning
                .deadline
                .as_ref()
                .map_or_else(String::new, blossom_core::Timestamp::format),
        });
    }

    /// Where the read view's byte offsets start in the document.
    #[must_use]
    pub fn blocks_base(&self) -> Option<usize> {
        let doc = self.doc.as_ref()?;
        match self.selected {
            Selected::Heading(i) => doc.heading(i).map(|h| h.spans.body.start),
            Selected::Intro => doc.preamble().intro.as_ref().map(|r| r.start),
            Selected::Nothing => None,
        }
    }

    /// The editor range for what is selected.
    #[must_use]
    pub fn edit_range(&self) -> Option<Range<usize>> {
        let doc = self.doc.as_ref()?;
        match self.selected {
            Selected::Heading(i) => doc.section_range(i),
            Selected::Intro | Selected::Nothing => Some(doc.top_range()),
        }
    }
}

/// Whether the file has anything before its first heading.
fn has_top(doc: &Document) -> bool {
    !doc.top_range().is_empty() || doc.headings().is_empty()
}

/// A heading title's inline markup as spans.
#[must_use]
pub fn title_spans(title: &str) -> Vec<Span> {
    match render::blocks(title).into_iter().next() {
        Some(Block::Paragraph(spans)) => spans,
        _ => vec![Span {
            text: title.to_string(),
            style: render::Style::default(),
            link: None,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blossom_core::Keywords;

    fn doc(s: &str) -> Document {
        Document::parse(s, Keywords::default())
    }

    #[test]
    fn typing_groups_into_one_undo_step() {
        let mut f = OpenFile::loading("a.org".into());
        f.show(Arc::new(doc("* a\n")));
        for text in ["* ab\n", "* abc\n"] {
            let before = f.snapshot();
            f.change(before, doc(text), Group::Typing);
        }
        let before = f.snapshot();
        f.change(before, doc("* TODO abc\n"), Group::Single);
        assert!(f.undo(false));
        assert_eq!(f.doc.as_ref().unwrap().text(), "* abc\n");
        assert!(f.undo(false));
        assert_eq!(f.doc.as_ref().unwrap().text(), "* a\n");
        assert!(!f.undo(false));
        assert!(f.undo(true));
        assert!(f.undo(true));
        assert_eq!(f.doc.as_ref().unwrap().text(), "* TODO abc\n");
        assert_eq!(f.history(), (true, false));
    }

    #[test]
    fn outside_change_drops_history_and_follows_the_editor() {
        let mut f = OpenFile::loading("a.org".into());
        f.show(Arc::new(doc("* a\nbody\n* b\n")));
        f.selected = Selected::Heading(1);
        let range = f.edit_range().unwrap();
        f.editor = Some(SectionEditor::new(f.doc.as_ref().unwrap(), range));
        let before = f.snapshot();
        f.change(before, doc("* a\nbody\n* b\n"), Group::Single);
        f.adopt(Arc::new(doc("* a\nlonger body\n* b\nmore\n")));
        assert_eq!(f.history(), (false, false));
        let ed = f.editor.as_ref().unwrap();
        assert_eq!(ed.content.text(), "* b\nmore\n");
        assert_eq!(f.selected, Selected::Heading(1));
    }

    #[test]
    fn dirty_tracks_text_not_identity() {
        let mut d = DiskState::clean(None, "x");
        assert!(!d.dirty());
        d.latest = Arc::new("x".into());
        assert!(!d.dirty());
        d.latest = Arc::new("y".into());
        assert!(d.dirty());
    }
}
