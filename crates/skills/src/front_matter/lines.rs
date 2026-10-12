//! Front matter lines with their indentation.

/// One line of front matter.
#[derive(Clone, Copy, Debug)]
pub(super) struct Line<'a> {
    /// 1-based line number within the file.
    pub number: usize,
    /// Leading spaces or tabs.
    pub indent: usize,
    /// The line without its `\r`, untrimmed (block scalars keep inner indentation).
    pub raw: &'a str,
    /// The line after its indentation, without trailing whitespace.
    pub content: &'a str,
}

impl Line<'_> {
    /// Blank or a comment.
    pub fn is_ignorable(&self) -> bool {
        self.content.is_empty() || self.content.starts_with('#')
    }

    /// `- item` (or a bare `-`).
    pub fn is_list_item(&self) -> bool {
        self.content == "-" || self.content.starts_with("- ") || self.content.starts_with("-\t")
    }
}

pub(super) fn split_lines(text: &str, first_line: usize) -> Vec<Line<'_>> {
    text.split('\n')
        .enumerate()
        .map(|(index, raw)| {
            let raw = raw.strip_suffix('\r').unwrap_or(raw);
            let content = raw.trim_start_matches([' ', '\t']);
            Line {
                number: first_line + index,
                indent: raw.len() - content.len(),
                raw,
                content: content.trim_end(),
            }
        })
        .collect()
}
