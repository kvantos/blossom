//! A small display model of org text for read views: blocks of styled spans.
//! Built from orgize's tree so the app never needs orgize itself. Anything
//! without a dedicated block shows as its raw text, so nothing is hidden by
//! accident; only comments and `#+KEY:` lines are left out.

use orgize::{Org, SyntaxElement, SyntaxKind, SyntaxNode};

/// How a span of text is styled. Markup nests (`*/bold italic/*`), so these
/// are independent flags rather than one enum.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct Style {
    /// `*bold*`
    pub bold: bool,
    /// `/italic/`
    pub italic: bool,
    /// `_underline_`
    pub underline: bool,
    /// `+strike+`
    pub strike: bool,
    /// `~code~` or `=verbatim=`
    pub code: bool,
    /// A timestamp.
    pub date: bool,
}

/// A run of text with one style, maybe a link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    /// The text shown.
    pub text: String,
    /// Its style.
    pub style: Style,
    /// The link target (`https://…`, `file:…`, `id:…`), if it is a link.
    pub link: Option<String>,
    /// A link without a description to an image file: shown as the image.
    pub image: bool,
}

/// One block of a read view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// A paragraph; line breaks are kept as `\n` in the spans.
    Paragraph(Vec<Span>),
    /// A list item and what it contains (its text, nested lists, …).
    ListItem {
        /// The bullet as written, trimmed: `-`, `+`, `1.`, `2)`.
        bullet: String,
        /// The checkbox mark: `' '`, `'X'` or `'-'`.
        checkbox: Option<char>,
        /// Byte offset of the item in the source text (for toggling).
        offset: usize,
        /// Its content.
        children: Vec<Block>,
    },
    /// A source, example or fixed-width block.
    Code {
        /// The language of a source block.
        language: Option<String>,
        /// The text, without its last line ending.
        text: String,
    },
    /// A quote, center, verse or special block.
    Quote(Vec<Block>),
    /// A table: rows of cells of spans. Rule rows are left out.
    Table(Vec<Vec<Vec<Span>>>),
    /// `-----`
    Rule,
    /// An image: a link without a description to an image file, as org
    /// shows inline. Split out of its paragraph.
    Image {
        /// The link target as written, e.g. `file:img/a.png`.
        target: String,
        /// The file path: the target without `file:` and any `::search`.
        path: String,
    },
    /// A drawer other than properties.
    Drawer {
        /// Its name.
        name: String,
        /// Its raw content.
        text: String,
    },
}

/// The blocks of `text` (a heading's body, or an intro).
#[must_use]
pub fn blocks(text: &str) -> Vec<Block> {
    let org = Org::parse(text);
    let root = SyntaxNode::new_root(org.green().clone());
    let mut out = Vec::new();
    for child in root.children() {
        match child.kind() {
            SyntaxKind::SECTION => element_blocks(&child, &mut out),
            // A body never has headlines; render one as text if it appears.
            _ => out.push(Block::Paragraph(vec![plain(child.to_string().trim_end())])),
        }
    }
    out
}

fn plain(text: &str) -> Span {
    Span {
        text: text.to_string(),
        style: Style::default(),
        link: None,
        image: false,
    }
}

/// Extensions shown inline as images.
const IMAGE_EXTENSIONS: [&str; 8] = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "svgz"];

/// The file path of a link target that points at a local image file
/// (`file:a.png`, `./a.png`, `/abs/a.svg`, `~/a.jpg`, with any `::search`
/// dropped), or `None`.
#[must_use]
pub fn image_path(target: &str) -> Option<String> {
    let path = if let Some(p) = target.strip_prefix("file:") {
        p
    } else if target.starts_with(['.', '/', '~']) {
        target
    } else {
        return None;
    };
    let path = path.split("::").next().unwrap_or(path);
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    (!path.is_empty() && IMAGE_EXTENSIONS.contains(&ext.as_str())).then(|| path.to_string())
}

/// A paragraph's spans split around its image links: text before an image
/// becomes a paragraph above it, text after one below.
fn split_images(spans: Vec<Span>, out: &mut Vec<Block>) {
    let mut text: Vec<Span> = Vec::new();
    let flush = |text: &mut Vec<Span>, out: &mut Vec<Block>| {
        trim_trailing_newlines(text);
        while text.first().is_some_and(|s| s.text.trim_start().is_empty()) {
            text.remove(0);
        }
        if let Some(first) = text.first_mut() {
            first.text = first.text.trim_start_matches(['\n', '\r']).to_string();
        }
        if !text.iter().all(|s| s.text.trim().is_empty()) {
            out.push(Block::Paragraph(std::mem::take(text)));
        }
        text.clear();
    };
    for s in spans {
        if s.image
            && let Some(target) = s.link.clone()
            && let Some(path) = image_path(&target)
        {
            flush(&mut text, out);
            out.push(Block::Image { target, path });
        } else {
            text.push(s);
        }
    }
    flush(&mut text, out);
}

fn element_blocks(parent: &SyntaxNode, out: &mut Vec<Block>) {
    for node in parent.children() {
        if node.kind() == SyntaxKind::PARAGRAPH {
            let mut spans = Vec::new();
            inline_spans(&node, Style::default(), &mut spans);
            split_images(spans, out);
        } else if let Some(b) = element_block(&node) {
            out.push(b);
        } else if node.kind() == SyntaxKind::LIST {
            list_items(&node, out);
        }
    }
}

fn child(node: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxNode> {
    node.children().find(|c| c.kind() == kind)
}

fn token_text(node: &SyntaxNode, kind: SyntaxKind) -> Option<String> {
    node.children_with_tokens()
        .filter_map(SyntaxElement::into_token)
        .find(|t| t.kind() == kind)
        .map(|t| t.text().to_string())
}

fn strip_eol(s: &str) -> String {
    s.strip_suffix('\n')
        .map_or(s, |s| s.strip_suffix('\r').unwrap_or(s))
        .to_string()
}

fn element_block(node: &SyntaxNode) -> Option<Block> {
    use SyntaxKind as K;
    Some(match node.kind() {
        K::SOURCE_BLOCK | K::EXAMPLE_BLOCK | K::EXPORT_BLOCK => Block::Code {
            language: child(node, K::BLOCK_BEGIN)
                .and_then(|b| token_text(&b, K::SRC_BLOCK_LANGUAGE))
                .map(|l| l.trim().to_string()),
            text: child(node, K::BLOCK_CONTENT)
                .map_or_else(String::new, |c| strip_eol(&c.to_string())),
        },
        K::FIXED_WIDTH => Block::Code {
            language: None,
            text: strip_eol(
                &node
                    .to_string()
                    .lines()
                    .map(fixed_line)
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        },
        K::QUOTE_BLOCK | K::CENTER_BLOCK | K::VERSE_BLOCK | K::SPECIAL_BLOCK => {
            let mut inner = Vec::new();
            if let Some(content) = child(node, K::BLOCK_CONTENT) {
                if content.children().next().is_some() {
                    element_blocks(&content, &mut inner);
                } else {
                    inner.push(Block::Paragraph(vec![plain(&strip_eol(
                        &content.to_string(),
                    ))]));
                }
            }
            Block::Quote(inner)
        }
        K::ORG_TABLE => Block::Table(
            node.children()
                .filter(|r| r.kind() == K::ORG_TABLE_STANDARD_ROW)
                .map(|row| {
                    row.children()
                        .filter(|c| c.kind() == K::ORG_TABLE_CELL)
                        .map(|cell| {
                            let mut spans = Vec::new();
                            inline_spans(&cell, Style::default(), &mut spans);
                            spans
                        })
                        .collect()
                })
                .collect(),
        ),
        K::RULE => Block::Rule,
        K::DRAWER => Block::Drawer {
            name: child(node, K::DRAWER_BEGIN)
                .and_then(|b| token_text(&b, K::TEXT))
                .unwrap_or_default(),
            text: child(node, K::DRAWER_CONTENT)
                .map_or_else(String::new, |c| strip_eol(&c.to_string())),
        },
        K::KEYWORD
        | K::AFFILIATED_KEYWORD
        | K::COMMENT
        | K::COMMENT_BLOCK
        | K::BABEL_CALL
        | K::PROPERTY_DRAWER => {
            return None;
        }
        K::LIST => return None,
        _ => {
            let text = strip_eol(&node.to_string());
            if text.trim().is_empty() {
                return None;
            }
            Block::Code {
                language: None,
                text,
            }
        }
    })
}

/// `: text` → `text`.
fn fixed_line(line: &str) -> &str {
    let t = line.trim_start();
    t.strip_prefix(": ")
        .or_else(|| t.strip_prefix(':'))
        .unwrap_or(t)
}

fn list_items(list: &SyntaxNode, out: &mut Vec<Block>) {
    use SyntaxKind as K;
    for item in list.children().filter(|c| c.kind() == K::LIST_ITEM) {
        let bullet = token_text(&item, K::LIST_ITEM_BULLET)
            .unwrap_or_default()
            .trim()
            .to_string();
        let checkbox = child(&item, K::LIST_ITEM_CHECK_BOX)
            .and_then(|c| token_text(&c, K::TEXT).or_else(|| token_text(&c, K::WHITESPACE)))
            .and_then(|t| t.chars().next())
            .map(|c| if c == 'x' { 'X' } else { c })
            .or_else(|| child(&item, K::LIST_ITEM_CHECK_BOX).map(|_| ' '));
        let mut children = Vec::new();
        if let Some(tag) = child(&item, K::LIST_ITEM_TAG) {
            let mut spans = Vec::new();
            inline_spans(
                &tag,
                Style {
                    bold: true,
                    ..Style::default()
                },
                &mut spans,
            );
            children.push(Block::Paragraph(spans));
        }
        if let Some(content) = child(&item, K::LIST_ITEM_CONTENT) {
            element_blocks(&content, &mut children);
        }
        out.push(Block::ListItem {
            bullet,
            checkbox,
            offset: usize::from(item.text_range().start()),
            children,
        });
    }
}

fn inline_spans(node: &SyntaxNode, style: Style, out: &mut Vec<Span>) {
    use SyntaxKind as K;
    for el in node.children_with_tokens() {
        match el {
            SyntaxElement::Token(t) => {
                if matches!(t.kind(), K::TEXT | K::WHITESPACE | K::NEW_LINE) {
                    push(out, t.text(), style, None);
                }
            }
            SyntaxElement::Node(n) => match n.kind() {
                K::BOLD => inline_spans(
                    &n,
                    Style {
                        bold: true,
                        ..style
                    },
                    out,
                ),
                K::ITALIC => inline_spans(
                    &n,
                    Style {
                        italic: true,
                        ..style
                    },
                    out,
                ),
                K::UNDERLINE => inline_spans(
                    &n,
                    Style {
                        underline: true,
                        ..style
                    },
                    out,
                ),
                K::STRIKE => inline_spans(
                    &n,
                    Style {
                        strike: true,
                        ..style
                    },
                    out,
                ),
                K::CODE | K::VERBATIM => {
                    let inner = token_text(&n, K::TEXT).unwrap_or_default();
                    push(
                        out,
                        &inner,
                        Style {
                            code: true,
                            ..style
                        },
                        None,
                    );
                }
                K::LINK => {
                    let target = token_text(&n, K::LINK_PATH).unwrap_or_default();
                    let mut label = Vec::new();
                    inline_spans(&n, style, &mut label);
                    if label.iter().all(|s| s.text.is_empty()) {
                        if image_path(&target).is_some() {
                            out.push(Span {
                                text: target.clone(),
                                style,
                                link: Some(target),
                                image: true,
                            });
                        } else {
                            push(out, &target, style, Some(&target));
                        }
                    } else {
                        for s in label {
                            push(out, &s.text, s.style, Some(&target));
                        }
                    }
                }
                K::TIMESTAMP_ACTIVE | K::TIMESTAMP_INACTIVE | K::TIMESTAMP_DIARY => {
                    push(
                        out,
                        &n.to_string(),
                        Style {
                            date: true,
                            ..style
                        },
                        None,
                    );
                }
                K::LINE_BREAK => push(out, "\n", style, None),
                _ => push(out, &n.to_string(), style, None),
            },
        }
    }
}

/// Appends text, merging with the previous span when nothing differs.
fn push(out: &mut Vec<Span>, text: &str, style: Style, link: Option<&str>) {
    if text.is_empty() {
        return;
    }
    let link = link.map(String::from);
    if let Some(last) = out.last_mut()
        && last.style == style
        && last.link == link
        && !last.image
    {
        last.text.push_str(text);
        return;
    }
    out.push(Span {
        text: text.to_string(),
        style,
        link,
        image: false,
    });
}

fn trim_trailing_newlines(spans: &mut Vec<Span>) {
    while let Some(last) = spans.last_mut() {
        let trimmed = last.text.trim_end_matches(['\n', '\r']).len();
        last.text.truncate(trimmed);
        if last.text.is_empty() {
            spans.pop();
        } else {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn paragraph_markup_and_links() {
        let b = blocks(
            "Para *b* /i/ ~code~ [[https://x.org][the *label*]] [[file:a.txt]] <2026-11-23 Mon>\nline two\n",
        );
        let [Block::Paragraph(spans)] = b.as_slice() else {
            panic!("{b:?}")
        };
        assert_eq!(
            text(spans),
            "Para b i code the label file:a.txt <2026-11-23 Mon>\nline two"
        );
        let find = |t: &str| spans.iter().find(|s| s.text == t).unwrap();
        assert!(find("b").style.bold);
        assert!(find("i").style.italic);
        assert!(find("code").style.code);
        assert!(find("label").style.bold);
        assert_eq!(find("label").link.as_deref(), Some("https://x.org"));
        assert_eq!(find("file:a.txt").link.as_deref(), Some("file:a.txt"));
        assert!(find("<2026-11-23 Mon>").style.date);
    }

    #[test]
    fn images_split_paragraphs() {
        let b = blocks(
            "See [[file:img/a.png]] here\n[[./b.SVG]]\n[[file:c.png][described]] [[https://x/y.png]]\n- [[~/d.jpg]]\n",
        );
        let img = |t: &str, p: &str| Block::Image {
            target: t.into(),
            path: p.into(),
        };
        assert_eq!(b[0], Block::Paragraph(vec![plain("See ")]));
        assert_eq!(b[1], img("file:img/a.png", "img/a.png"));
        assert_eq!(b[2], Block::Paragraph(vec![plain(" here")]));
        assert_eq!(b[3], img("./b.SVG", "./b.SVG"));
        let Block::Paragraph(rest) = &b[4] else {
            panic!("{b:?}")
        };
        assert_eq!(text(rest), "described https://x/y.png");
        let Block::ListItem { children, .. } = &b[5] else {
            panic!("{b:?}")
        };
        assert_eq!(children, &[img("~/d.jpg", "~/d.jpg")]);
        assert_eq!(image_path("file:a.png::search").as_deref(), Some("a.png"));
        assert_eq!(image_path("file:notes.org"), None);
        assert_eq!(image_path("id:abc.png"), None);
    }

    #[test]
    fn lists_blocks_tables() {
        let src = "- [X] done\n  - nested\n1. one\n#+BEGIN_SRC rust\nfn x() {}\n#+END_SRC\n#+begin_quote\nq *b*\n#+end_quote\n| a | b |\n|---+---|\n| 1 | 2 |\n-----\n: fixed\n#+TITLE: hidden\n# hidden\n:NOTES:\nx\n:END:\n";
        let b = blocks(src);
        let Block::ListItem {
            bullet,
            checkbox,
            offset,
            children,
        } = &b[0]
        else {
            panic!("{b:?}")
        };
        assert_eq!((bullet.as_str(), *checkbox, *offset), ("-", Some('X'), 0));
        assert!(matches!(&children[1], Block::ListItem { bullet, .. } if bullet == "-"));
        assert!(matches!(&b[1], Block::ListItem { bullet, checkbox: None, .. } if bullet == "1."));
        assert_eq!(
            b[2],
            Block::Code {
                language: Some("rust".into()),
                text: "fn x() {}".into()
            }
        );
        assert!(matches!(&b[3], Block::Quote(inner) if inner.len() == 1));
        let Block::Table(rows) = &b[4] else {
            panic!("{:?}", b[4])
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(text(&rows[1][1]), "2");
        assert_eq!(b[5], Block::Rule);
        assert_eq!(
            b[6],
            Block::Code {
                language: None,
                text: "fixed".into()
            }
        );
        assert_eq!(
            b[7],
            Block::Drawer {
                name: "NOTES".into(),
                text: "x".into()
            }
        );
        assert_eq!(b.len(), 8);
    }

    #[test]
    fn empty_and_blank() {
        assert_eq!(blocks(""), []);
        assert_eq!(blocks("\n\n"), []);
        let [Block::ListItem { checkbox, .. }] = blocks("- [ ] open\n").as_slice().to_owned()[..]
        else {
            panic!()
        };
        assert_eq!(checkbox, Some(' '));
    }
}
