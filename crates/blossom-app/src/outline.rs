//! Which headings of a file the outline shows.

use std::collections::HashSet;

use blossom_core::Document;

/// One visible outline row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    /// The heading's index.
    pub index: usize,
    /// How many ancestors it has (indentation).
    pub depth: usize,
    /// Whether it has children to fold.
    pub has_children: bool,
    /// Whether it is folded.
    pub folded: bool,
}

/// Headings in document order, skipping everything under a folded heading.
#[must_use]
pub fn rows(doc: &Document, folded: &HashSet<usize>) -> Vec<Row> {
    let headings = doc.headings();
    let mut out = Vec::new();
    let mut depths: Vec<usize> = Vec::with_capacity(headings.len());
    let mut skip_until = 0;
    for h in headings {
        let depth = h.parent.and_then(|p| depths.get(p)).map_or(0, |d| d + 1);
        depths.push(depth);
        if h.index < skip_until {
            continue;
        }
        let descendants = doc.descendants(h.index);
        let is_folded = folded.contains(&h.index) && !descendants.is_empty();
        if is_folded {
            skip_until = descendants.end;
        }
        out.push(Row {
            index: h.index,
            depth,
            has_children: !descendants.is_empty(),
            folded: is_folded,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use blossom_core::Keywords;

    #[test]
    fn folding_hides_descendants() {
        let doc = Document::parse(
            "* a\n** a1\n*** a11\n** a2\n* b\n*** deep\n",
            Keywords::default(),
        );
        let all = rows(&doc, &HashSet::new());
        let shape: Vec<(usize, usize, bool)> = all
            .iter()
            .map(|r| (r.index, r.depth, r.has_children))
            .collect();
        assert_eq!(
            shape,
            [
                (0, 0, true),
                (1, 1, true),
                (2, 2, false),
                (3, 1, false),
                (4, 0, true),
                (5, 1, false)
            ]
        );
        let folded = rows(&doc, &HashSet::from([1, 4]));
        let visible: Vec<usize> = folded.iter().map(|r| r.index).collect();
        assert_eq!(visible, [0, 1, 3, 4]);
        assert!(folded[1].folded);
        // Folding a leaf does nothing.
        assert_eq!(rows(&doc, &HashSet::from([2])).len(), 6);
    }
}
