//! The sidebar's "New file" form: a name (`garden`, or `projects/garden`
//! for a folder; `.org` is added) and an optional title, written as
//! `#+TITLE:`. Below the fields it says which file it will make, or why it
//! can't.

use blossom_vault::ops::new_file_path;
use iced::widget::{button, column, row, text, text_input};
use iced::{Element, Fill, Theme};

use crate::app::{Message, dim};
use crate::tree::FileInfo;

/// The name field's widget id, for focusing.
pub const NAME: &str = "new-file-name";

/// What the form does.
#[derive(Clone, Debug)]
pub enum NewFileMsg {
    /// Show the form.
    Open,
    /// The name field.
    Name(String),
    /// The title field.
    Title(String),
    /// Make the file.
    Create,
    /// Close the form.
    Cancel,
}

/// The open form.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewFile {
    /// The name as typed.
    pub name: String,
    /// The title as typed.
    pub title: String,
}

impl NewFile {
    /// The file to make, or why not: a bad name, or one that exists (in
    /// any letter case, as some disks don't tell them apart).
    pub fn target(&self, files: &[FileInfo]) -> Result<String, String> {
        let path = new_file_path(&self.name)?;
        let lower = path.to_lowercase();
        if files.iter().any(|f| f.path.to_lowercase() == lower) {
            return Err(format!("{path} exists already"));
        }
        Ok(path)
    }
}

fn send(m: NewFileMsg) -> Message {
    Message::NewFile(m)
}

/// The form, or the button that opens it.
pub fn view<'a>(form: Option<&'a NewFile>, files: &[FileInfo]) -> Element<'a, Message> {
    let Some(form) = form else {
        return button(text("+ New file").size(13))
            .on_press(send(NewFileMsg::Open))
            .width(Fill)
            .padding([6, 12])
            .style(button::text)
            .into();
    };
    let target = form.target(files);
    let note = match &target {
        Ok(path) => text(format!("Creates {path}")).size(12).style(dim),
        // Nothing typed yet: no complaint.
        Err(_) if form.name.trim().is_empty() => text("One file per project or topic works well.")
            .size(12)
            .style(dim),
        Err(why) => text(why.clone())
            .size(12)
            .style(|theme: &Theme| text::Style {
                color: Some(theme.extended_palette().danger.base.color),
            }),
    };
    let mut create = button(text("Create").size(13))
        .padding([3, 10])
        .style(button::primary);
    if target.is_ok() {
        create = create.on_press(send(NewFileMsg::Create));
    }
    column![
        text_input("Name, e.g. garden or work/plans", &form.name)
            .id(NAME)
            .on_input(|s| send(NewFileMsg::Name(s)))
            .on_submit(send(NewFileMsg::Create))
            .size(13)
            .padding([4, 8]),
        text_input("Title (optional)", &form.title)
            .on_input(|s| send(NewFileMsg::Title(s)))
            .on_submit(send(NewFileMsg::Create))
            .size(13)
            .padding([4, 8]),
        note.width(Fill),
        row![
            create,
            button(text("Cancel").size(13))
                .on_press(send(NewFileMsg::Cancel))
                .padding([3, 10])
                .style(button::text),
        ]
        .spacing(6),
    ]
    .spacing(6)
    .padding([8, 10])
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(path: &str) -> FileInfo {
        FileInfo {
            path: path.into(),
            headings: 0,
            open_tasks: 0,
            conflict: false,
        }
    }

    #[test]
    fn target() {
        let files = [info("inbox.org"), info("work/Plans.org")];
        let form = |name: &str| NewFile {
            name: name.into(),
            title: String::new(),
        };
        assert_eq!(form("garden").target(&files), Ok("garden.org".into()));
        assert!(form("inbox").target(&files).is_err());
        assert!(
            form("work/plans")
                .target(&files)
                .unwrap_err()
                .contains("exists")
        );
        assert!(form("").target(&files).is_err());
    }
}
