//! Small outline icons (24×24, drawn in the text colour) for the diagram
//! tools.

use std::sync::LazyLock;

use iced::widget::svg;
use iced::{Element, Theme};

use crate::app::Message;

/// Starts an outline icon.
macro_rules! icon {
    ($name:ident, $body:literal) => {
        pub static $name: LazyLock<svg::Handle> = LazyLock::new(|| {
            svg::Handle::from_memory(
                concat!(
                    r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">"##,
                    $body,
                    "</svg>"
                )
                .as_bytes(),
            )
        });
    };
}

// Diagram tools.
icon!(
    SELECT,
    r##"<path d="M5 3.5l13 7.2-5.6 1.6-2.6 5.6z"/><path d="M12.6 12.4l4.4 6.1"/>"##
);
icon!(
    PEN,
    r##"<path d="M4 18c2.5-4 4.5-9 7-9s1.5 7 4 7 3-4 5-6"/>"##
);
icon!(LINE, r##"<path d="M5 19L19 5"/>"##);
icon!(ARROW, r##"<path d="M5 19L19 5"/><path d="M10 5h9v9"/>"##);
icon!(
    RECT,
    r##"<rect x="4" y="6" width="16" height="12" rx="1.5"/>"##
);
icon!(ELLIPSE, r##"<ellipse cx="12" cy="12" rx="8.5" ry="6.5"/>"##);
icon!(DIAMOND, r##"<path d="M12 3.5l8.5 8.5-8.5 8.5L3.5 12z"/>"##);
icon!(TEXT, r##"<path d="M5 6V4.5h14V6M12 4.5v15M9 19.5h6"/>"##);

/// `handle` at `size` pixels, in the text colour (or the primary button's
/// text colour when `on_primary`).
pub fn view<'a>(handle: &svg::Handle, size: f32, on_primary: bool) -> Element<'a, Message> {
    svg(handle.clone())
        .width(size)
        .height(size)
        .style(move |theme: &Theme, _| svg::Style {
            color: Some(if on_primary {
                theme.extended_palette().primary.base.text
            } else {
                theme.palette().text
            }),
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_build() {
        for h in [
            &SELECT, &PEN, &LINE, &ARROW, &RECT, &ELLIPSE, &DIAMOND, &TEXT,
        ] {
            let _ = LazyLock::force(h);
        }
    }
}
