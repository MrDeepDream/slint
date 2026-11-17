// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0

use alloc::string::String;
use core::ops::Range;
pub use i_slint_common::styled_text::{FormattedSpan, Style, StyledTextParagraph};

/// Styled text that has been parsed and separated into paragraphs
#[repr(transparent)]
#[derive(Debug, PartialEq, Clone, Default)]
pub struct StyledText {
    /// Paragraphs of styled text
    pub(crate) paragraphs: crate::SharedVector<i_slint_common::styled_text::StyledTextParagraph>,
}

/// Byte offsets into a styled text, as the `StyledText` element's selection and the methods
/// below use them, index its plain text: the paragraphs' texts joined with one `'\n'`.
impl StyledText {
    /// Creates styled text from paragraphs that the application built itself, for example from
    /// the events of its own Markdown parser.
    ///
    /// The byte ranges of spans and links are relative to their paragraph's text. A range that
    /// reaches past the text is cut at its end, an offset inside a character moves to the start
    /// of that character, and ranges left empty are dropped, so no input can make the layout
    /// panic. Links are sorted by where they start, which is the order Tab moves through them.
    pub fn from_paragraphs(paragraphs: impl IntoIterator<Item = StyledTextParagraph>) -> Self {
        Self { paragraphs: paragraphs.into_iter().map(sanitized).collect() }
    }

    /// The plain text in `range`, leaving out the spans styled [`Style::Ornament`]. The range is
    /// clamped to the text and to character boundaries, so any range is accepted.
    pub fn plain_text(&self, range: Range<usize>) -> String {
        let mut result = String::new();
        let mut start = 0;
        let count = self.paragraphs.len();
        for (index, paragraph) in self.paragraphs.iter().enumerate() {
            let end = start + paragraph.text.len();
            let local = range.start.clamp(start, end) - start..range.end.clamp(start, end) - start;
            if !local.is_empty() {
                push_without_ornaments(&mut result, paragraph, local);
            }
            if range.end <= end {
                break;
            }
            if index + 1 < count && range.start <= end {
                result.push('\n');
            }
            start = end + 1;
        }
        result
    }

    /// The links in the order of the text, each with its byte range in the plain text and its
    /// target.
    pub fn links(&self) -> impl Iterator<Item = (Range<usize>, &str)> {
        self.paragraphs_with_start().flat_map(|(start, paragraph)| {
            paragraph
                .links
                .iter()
                .map(move |(range, url)| (start + range.start..start + range.end, url.as_str()))
        })
    }

    /// `offset` clamped to the length of the plain text and moved back to the start of the
    /// character it falls in.
    pub fn clamp_offset(&self, offset: usize) -> usize {
        let mut end = 0;
        for (start, paragraph) in self.paragraphs_with_start() {
            end = start + paragraph.text.len();
            if offset <= end {
                return start + floor_char_boundary(&paragraph.text, offset - start);
            }
        }
        end
    }

    /// The byte range in the plain text of the paragraph that holds `offset`, and its text.
    #[cfg(feature = "shared-parley")]
    pub(crate) fn paragraph_at(&self, offset: usize) -> (Range<usize>, &str) {
        self.paragraphs_with_start()
            .find(|(start, paragraph)| offset <= start + paragraph.text.len())
            .or_else(|| self.paragraphs_with_start().last())
            .map_or((0..0, ""), |(start, paragraph)| {
                (start..start + paragraph.text.len(), paragraph.text.as_str())
            })
    }

    fn paragraphs_with_start(&self) -> impl Iterator<Item = (usize, &StyledTextParagraph)> {
        let mut next_start = 0;
        self.paragraphs.iter().map(move |paragraph| {
            let start = next_start;
            next_start += paragraph.text.len() + 1;
            (start, paragraph)
        })
    }
}

fn floor_char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn sanitized(mut paragraph: StyledTextParagraph) -> StyledTextParagraph {
    let text = &paragraph.text;
    let fix = |range: &mut Range<usize>| {
        *range = floor_char_boundary(text, range.start)..floor_char_boundary(text, range.end);
        range.start < range.end
    };
    paragraph.formatting.retain_mut(|span| fix(&mut span.range));
    paragraph.links.retain_mut(|(range, _)| fix(range));
    paragraph.links.sort_by_key(|(range, _)| range.start);
    paragraph
}

fn push_without_ornaments(out: &mut String, paragraph: &StyledTextParagraph, range: Range<usize>) {
    let text = paragraph.text.as_str();
    let mut ornaments = paragraph
        .formatting
        .iter()
        .filter(|span| matches!(span.style, Style::Ornament))
        .map(|span| span.range.clone())
        .collect::<alloc::vec::Vec<_>>();
    ornaments.sort_by_key(|ornament| ornament.start);
    let mut pos = range.start;
    for ornament in ornaments.iter().chain(core::iter::once(&(range.end..range.end))) {
        let piece_end = ornament.start.clamp(pos, range.end);
        let piece = floor_char_boundary(text, pos)..floor_char_boundary(text, piece_end);
        out.push_str(text.get(piece).unwrap_or_default());
        pos = pos.max(ornament.end.min(range.end));
    }
}

/// Error returned when [`StyledText::from_markdown`] cannot parse the provided markdown input.
#[cfg(feature = "std")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledTextFromMarkdownError {
    message: alloc::string::String,
}

#[cfg(feature = "std")]
impl StyledTextFromMarkdownError {
    fn new(errors: alloc::vec::Vec<i_slint_common::styled_text::StyledTextParseError>) -> Self {
        Self {
            message: errors
                .iter()
                .map(alloc::string::ToString::to_string)
                .collect::<alloc::vec::Vec<_>>()
                .join("\n"),
        }
    }
}

#[cfg(feature = "std")]
impl core::fmt::Display for StyledTextFromMarkdownError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.message)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for StyledTextFromMarkdownError {}

#[cfg(feature = "std")]
impl StyledText {
    /// Creates styled text from plain text without applying markdown parsing.
    pub fn from_plain_text(text: &str) -> Self {
        Self {
            paragraphs: [i_slint_common::styled_text::paragraph_from_plain_text(text.into())]
                .into(),
        }
    }

    /// Parses markdown into styled text.
    pub fn from_markdown(markdown: &str) -> Result<Self, StyledTextFromMarkdownError> {
        let (paragraphs, errors) = i_slint_common::styled_text::parse_interpolated::<
            &[i_slint_common::styled_text::StyledTextParagraph],
        >(markdown, &[]);

        if errors.is_empty() {
            Ok(Self { paragraphs: paragraphs.as_slice().into() })
        } else {
            Err(StyledTextFromMarkdownError::new(errors))
        }
    }
}

pub fn get_raw_text(styled_text: &StyledText) -> alloc::borrow::Cow<'_, str> {
    match styled_text.paragraphs.as_slice() {
        [] => "".into(),
        [paragraph] => paragraph.text.as_str().into(),
        _ => {
            let mut result = alloc::string::String::new();
            for paragraph in styled_text.paragraphs.iter() {
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str(paragraph.text.as_str());
            }
            result.into()
        }
    }
}

/// Bindings for cbindgen
#[cfg(feature = "ffi")]
pub mod ffi {
    #![allow(unsafe_code)]

    use super::*;

    #[unsafe(no_mangle)]
    /// Create a new default styled text
    pub unsafe extern "C" fn slint_styled_text_new(out: *mut StyledText) {
        unsafe {
            core::ptr::write(out, Default::default());
        }
    }

    #[unsafe(no_mangle)]
    /// Destroy the shared string
    pub unsafe extern "C" fn slint_styled_text_drop(text: *const StyledText) {
        unsafe {
            core::ptr::read(text);
        }
    }

    #[unsafe(no_mangle)]
    /// Returns true if \a a is equal to \a b; otherwise returns false.
    pub extern "C" fn slint_styled_text_eq(a: &StyledText, b: &StyledText) -> bool {
        a == b
    }

    #[unsafe(no_mangle)]
    /// Clone the styled text
    pub unsafe extern "C" fn slint_styled_text_clone(out: *mut StyledText, ss: &StyledText) {
        unsafe { core::ptr::write(out, ss.clone()) }
    }

    #[cfg(feature = "std")]
    #[unsafe(no_mangle)]
    /// Create a styled text from plain text
    pub extern "C" fn slint_styled_text_from_plain_text(
        text: crate::slice::Slice<u8>,
        out: &mut StyledText,
    ) {
        let text = unsafe { core::str::from_utf8_unchecked(text.as_slice()) };
        *out = StyledText::from_plain_text(text);
    }

    #[cfg(feature = "std")]
    #[unsafe(no_mangle)]
    /// Parse markdown into styled text. Returns true on success.
    /// On failure, resets `out` to default.
    pub extern "C" fn slint_styled_text_from_markdown(
        markdown: crate::slice::Slice<u8>,
        out: &mut StyledText,
    ) -> bool {
        let markdown = unsafe { core::str::from_utf8_unchecked(markdown.as_slice()) };
        match StyledText::from_markdown(markdown) {
            Ok(styled) => {
                *out = styled;
                true
            }
            Err(_) => false,
        }
    }
}

pub fn parse_markdown(_format_string: &str, _args: &[StyledText]) -> StyledText {
    #[cfg(feature = "std")]
    {
        let paragraph_slices = _args
            .iter()
            .map(|styled_text| styled_text.paragraphs.as_slice())
            .collect::<alloc::vec::Vec<_>>();

        let (paragraphs, errors) =
            i_slint_common::styled_text::parse_interpolated(_format_string, &paragraph_slices);

        for e in &errors {
            crate::debug_log!("@markdown: {e}");
        }

        StyledText { paragraphs: paragraphs.as_slice().into() }
    }
    #[cfg(not(feature = "std"))]
    Default::default()
}

pub fn color_to_styled_text(_color: crate::Color) -> StyledText {
    #[cfg(feature = "std")]
    {
        let hex = alloc::format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            _color.red(),
            _color.green(),
            _color.blue(),
            _color.alpha()
        );
        StyledText::from_plain_text(&hex)
    }
    #[cfg(not(feature = "std"))]
    Default::default()
}

pub fn string_to_styled_text(_string: alloc::string::String) -> StyledText {
    #[cfg(feature = "std")]
    {
        if _string.is_empty() {
            return Default::default();
        }
        StyledText::from_plain_text(&_string)
    }
    #[cfg(not(feature = "std"))]
    Default::default()
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;

    #[test]
    fn string_to_styled_text_returns_default_for_empty_string() {
        assert_eq!(super::string_to_styled_text(Default::default()), StyledText::default());
    }

    fn span(range: Range<usize>, style: Style) -> FormattedSpan {
        FormattedSpan { range, style }
    }

    #[test]
    fn from_paragraphs_equals_from_markdown() {
        let built = StyledText::from_paragraphs([StyledTextParagraph {
            text: "Hello bold code link".into(),
            formatting: alloc::vec![
                span(6..10, Style::Strong),
                span(11..15, Style::Code),
                span(16..20, Style::Link),
            ],
            links: alloc::vec![(16..20, "https://slint.dev".into())],
        }]);
        let parsed =
            StyledText::from_markdown("Hello **bold** `code` [link](https://slint.dev)").unwrap();
        assert_eq!(built, parsed);
    }

    #[test]
    fn from_paragraphs_fixes_ranges_and_sorts_links() {
        // "é" is two bytes, so offset 1 is inside it.
        let built = StyledText::from_paragraphs([
            StyledTextParagraph {
                text: "é!".into(),
                formatting: alloc::vec![span(1..9, Style::Strong), span(0..1, Style::Code)],
                links: alloc::vec![(5..1, "reversed".into())],
            },
            StyledTextParagraph {
                text: "one two".into(),
                formatting: alloc::vec![],
                links: alloc::vec![(4..7, "two".into()), (0..3, "one".into())],
            },
        ]);
        assert_eq!(built.paragraphs[0].formatting, [span(0..3, Style::Strong)]);
        let links: alloc::vec::Vec<_> = built.links().collect();
        assert_eq!(links, [(4..7, "one"), (8..11, "two")]);
    }

    #[test]
    fn plain_text_leaves_out_ornaments() {
        // The icon glyph U+E8A5 takes the bytes 4..7.
        let text = StyledText::from_paragraphs([
            StyledTextParagraph {
                text: "See \u{e8a5} main.rs".into(),
                formatting: alloc::vec![span(4..7, Style::Ornament), span(4..15, Style::Link)],
                links: alloc::vec![(4..15, "file:///main.rs".into())],
            },
            StyledTextParagraph {
                text: "next".into(),
                formatting: alloc::vec![],
                links: alloc::vec![],
            },
        ]);
        assert_eq!(text.plain_text(0..usize::MAX), "See  main.rs\nnext");
        let (link, _) = text.links().next().unwrap();
        assert_eq!(text.plain_text(link), " main.rs");
        // A selection starting inside the ornament.
        assert_eq!(text.plain_text(5..9), " m");
        assert_eq!(text.plain_text(13..18), "rs\nne");
        assert_eq!(text.plain_text(9..5), "");
    }

    #[test]
    fn clamp_offset_stays_on_char_boundaries() {
        let text = StyledText::from_paragraphs([
            i_slint_common::styled_text::paragraph_from_plain_text("ab".into()),
            i_slint_common::styled_text::paragraph_from_plain_text("ö".into()),
        ]);
        // The plain text is "ab\nö": "ö" takes the bytes 3..5.
        assert_eq!(text.clamp_offset(2), 2);
        assert_eq!(text.clamp_offset(3), 3);
        assert_eq!(text.clamp_offset(4), 3);
        assert_eq!(text.clamp_offset(99), 5);
        assert_eq!(StyledText::default().clamp_offset(7), 0);
    }
}
