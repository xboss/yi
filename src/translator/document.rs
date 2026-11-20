//! Source-preserving paragraph planning for plain text and CommonMark/GFM.
//!
//! Markdown is edited by source byte ranges, never serialized back from an AST.
//! Immutable inline syntax is represented by validated opaque markers while a
//! model translates the surrounding paragraph. A final structural check rejects
//! newly introduced Markdown syntax. Raw HTML, code, metadata, math, image alt
//! text and reference definitions are deliberately preserved, not translated.

use std::ops::Range;

use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};
use serde::{Deserialize, Serialize};

use crate::translator::error::{Error, Result};

/// The supported source formats. HTML and MDX are intentionally not accepted
/// as separate formats: extract prose/Markdown before translating them.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputFormat {
    PlainText,
    Markdown,
}

/// A source document. Its original bytes remain available throughout a job.
#[derive(Clone, Debug)]
pub struct Document {
    text: String,
    format: InputFormat,
}

impl Document {
    /// Plain text: paragraphs are separated by blank lines.
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            format: InputFormat::PlainText,
        }
    }

    /// CommonMark with tables, task lists, footnotes, strikethrough, math,
    /// heading attributes, and YAML/TOML-style metadata blocks.
    pub fn markdown(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            format: InputFormat::Markdown,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn format(&self) -> InputFormat {
        self.format
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ProtectedSpan {
    pub marker: String,
    pub original: String,
}

#[derive(Clone, Debug)]
pub(crate) struct Block {
    pub range: Range<usize>,
    pub text: String,
    pub protected: Vec<ProtectedSpan>,
}

impl Block {
    /// The markers must occur exactly once, unchanged, and in source order.
    /// Never guess a missing code span's location or silently repair a marker.
    pub(crate) fn restore(&self, translated: &str) -> Result<String> {
        let observed = marker_ranges(translated)?;
        if observed.len() != self.protected.len() {
            return Err(Error::InvalidProtectedText(format!(
                "expected {} protected markers, received {}",
                self.protected.len(),
                observed.len()
            )));
        }
        for (range, expected) in observed.iter().zip(&self.protected) {
            if translated[range.clone()] != expected.marker {
                return Err(Error::InvalidProtectedText(format!(
                    "missing, unknown, duplicated or reordered marker; expected {}",
                    expected.marker
                )));
            }
        }

        let mut restored = String::with_capacity(translated.len());
        let mut cursor = 0;
        for (range, span) in observed.iter().zip(&self.protected) {
            restored.push_str(&translated[cursor..range.start]);
            restored.push_str(&span.original);
            cursor = range.end;
        }
        restored.push_str(&translated[cursor..]);
        Ok(restored)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DocumentPlan {
    pub source: String,
    pub blocks: Vec<Block>,
    pub format: InputFormat,
}

pub(crate) fn plan(document: &Document) -> Result<DocumentPlan> {
    let hash = source_hash(document.text());
    let blocks = match document.format {
        InputFormat::PlainText => plain_blocks(document.text(), hash),
        InputFormat::Markdown => markdown_blocks(document.text(), hash),
    };
    validate_ranges(document.text(), &blocks)?;
    Ok(DocumentPlan {
        source: document.text.clone(),
        blocks,
        format: document.format,
    })
}

/// Replace only planned blocks. All separators and untargeted source bytes are
/// copied verbatim. Reject malformed output rather than introducing Markdown
/// structures that the original document did not have.
pub(crate) fn render(plan: &DocumentPlan, replacements: &[String]) -> Result<String> {
    if replacements.len() != plan.blocks.len() {
        return Err(Error::InvalidConfig(format!(
            "expected {} block replacements, received {}",
            plan.blocks.len(),
            replacements.len()
        )));
    }
    validate_ranges(&plan.source, &plan.blocks)?;
    let mut output = String::with_capacity(plan.source.len());
    let mut cursor = 0;
    for (block, replacement) in plan.blocks.iter().zip(replacements) {
        output.push_str(&plan.source[cursor..block.range.start]);
        output.push_str(replacement);
        cursor = block.range.end;
    }
    output.push_str(&plan.source[cursor..]);
    if plan.format == InputFormat::Markdown && structure(&plan.source) != structure(&output) {
        return Err(Error::InvalidProtectedText(
            "translation changed the Markdown structure; no output was accepted".into(),
        ));
    }
    Ok(output)
}

/// Validate one proposed replacement in its original document context before
/// publishing a completed-block event. Parsing a block by itself is inadequate
/// for table cells, tight lists and other constructs whose grammar depends on
/// surrounding source. The final combined document is still checked by render.
pub(crate) fn validate_block(plan: &DocumentPlan, index: usize, replacement: &str) -> Result<()> {
    if plan.format == InputFormat::PlainText {
        return Ok(());
    }
    let block = plan.blocks.get(index).ok_or_else(|| {
        Error::InvalidConfig(format!("document block index {index} is out of range"))
    })?;
    let before = plan
        .source
        .get(..block.range.start)
        .ok_or_else(|| Error::InvalidConfig("invalid document block start offset".into()))?;
    let after = plan
        .source
        .get(block.range.end..)
        .ok_or_else(|| Error::InvalidConfig("invalid document block end offset".into()))?;
    let mut output = String::with_capacity(before.len() + replacement.len() + after.len());
    output.push_str(before);
    output.push_str(replacement);
    output.push_str(after);
    if structure(&plan.source) != structure(&output) {
        return Err(Error::InvalidProtectedText(format!(
            "block {index} translation changed the Markdown structure"
        )));
    }
    Ok(())
}

/// Complete marker byte ranges. The article chunker uses these as indivisible
/// spans. Original marker-looking prose is itself protected during planning.
pub(crate) fn marker_ranges(text: &str) -> Result<Vec<Range<usize>>> {
    let mut ranges = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = text[cursor..].find("⟦AT") {
        let start = cursor + relative;
        let Some(close) = text[start..].find('⟧') else {
            return Err(Error::InvalidProtectedText(
                "unterminated protected marker".into(),
            ));
        };
        let end = start + close + '⟧'.len_utf8();
        if text[start + "⟦AT".len()..end - '⟧'.len_utf8()].contains('⟦') {
            return Err(Error::InvalidProtectedText(
                "nested protected marker".into(),
            ));
        }
        ranges.push(start..end);
        cursor = end;
    }
    Ok(ranges)
}

fn markdown_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_MATH
        | Options::ENABLE_HEADING_ATTRIBUTES
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS
        | Options::ENABLE_GFM
}

#[derive(Default)]
struct Candidate {
    range: Option<Range<usize>>,
    editable: Vec<Range<usize>>,
}

impl Candidate {
    fn include(&mut self, range: Range<usize>) {
        if range.is_empty() {
            return;
        }
        match &mut self.range {
            Some(current) => {
                current.start = current.start.min(range.start);
                current.end = current.end.max(range.end);
            }
            None => self.range = Some(range),
        }
    }

    fn finish(&mut self, source: &str, hash: u64, blocks: &mut Vec<Block>) {
        let candidate = std::mem::take(self);
        if let Some(range) = candidate.range {
            if let Some(block) = make_block(source, range, candidate.editable, hash, blocks.len()) {
                blocks.push(block);
            }
        }
    }
}

fn is_inline_tag(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Emphasis
            | Tag::Strong
            | Tag::Strikethrough
            | Tag::Superscript
            | Tag::Subscript
            | Tag::Link { .. }
            | Tag::Image { .. }
    )
}

fn is_inline_end(tag: TagEnd) -> bool {
    matches!(
        tag,
        TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Superscript
            | TagEnd::Subscript
            | TagEnd::Link
            | TagEnd::Image
    )
}

fn opaque_inline(tag: &Tag<'_>) -> bool {
    match tag {
        Tag::Image { .. } => true,
        Tag::Link { link_type, .. } => matches!(
            link_type,
            LinkType::Autolink
                | LinkType::Email
                | LinkType::Collapsed
                | LinkType::CollapsedUnknown
                | LinkType::Shortcut
                | LinkType::ShortcutUnknown
        ),
        _ => false,
    }
}

fn markdown_blocks(source: &str, hash: u64) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut candidate = Candidate::default();
    let mut opaque_block_depth = 0_usize;
    let mut opaque_inline_depth = 0_usize;
    let mut inline_stack = Vec::new();

    for (event, range) in Parser::new_ext(source, markdown_options()).into_offset_iter() {
        match event {
            Event::Start(tag) if is_inline_tag(&tag) => {
                if opaque_block_depth == 0 {
                    candidate.include(range);
                }
                let opaque = opaque_inline(&tag);
                inline_stack.push(opaque);
                opaque_inline_depth += usize::from(opaque);
            }
            Event::End(tag) if is_inline_end(tag) => {
                if opaque_block_depth == 0 {
                    candidate.include(range);
                }
                if inline_stack.pop().unwrap_or(false) {
                    opaque_inline_depth = opaque_inline_depth.saturating_sub(1);
                }
            }
            Event::Start(tag) => {
                candidate.finish(source, hash, &mut blocks);
                if matches!(
                    tag,
                    Tag::CodeBlock(_) | Tag::HtmlBlock | Tag::MetadataBlock(_)
                ) {
                    opaque_block_depth += 1;
                }
            }
            Event::End(tag) => {
                candidate.finish(source, hash, &mut blocks);
                if matches!(
                    tag,
                    TagEnd::CodeBlock | TagEnd::HtmlBlock | TagEnd::MetadataBlock(_)
                ) {
                    opaque_block_depth = opaque_block_depth.saturating_sub(1);
                }
            }
            Event::Text(text) if opaque_block_depth == 0 => {
                candidate.include(range.clone());
                // Entity spellings and backslash escapes are preserved instead
                // of translating decoded text against incompatible byte offsets.
                if opaque_inline_depth == 0 && source.get(range.clone()) == Some(text.as_ref()) {
                    candidate.editable.push(range);
                }
            }
            Event::Rule | Event::Html(_) => candidate.finish(source, hash, &mut blocks),
            _ if opaque_block_depth == 0 => candidate.include(range),
            _ => {}
        }
    }
    candidate.finish(source, hash, &mut blocks);
    blocks
}

fn plain_blocks(source: &str, hash: u64) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut paragraph_start = None;
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        if line.trim().is_empty() {
            if let Some(start) = paragraph_start.take() {
                add_plain_block(source, start..offset, hash, &mut blocks);
            }
        } else if paragraph_start.is_none() {
            paragraph_start = Some(offset);
        }
        offset += line.len();
    }
    if let Some(start) = paragraph_start {
        add_plain_block(source, start..source.len(), hash, &mut blocks);
    }
    blocks
}

fn add_plain_block(source: &str, range: Range<usize>, hash: u64, blocks: &mut Vec<Block>) {
    let raw = &source[range.clone()];
    let start = range.start + raw.len() - raw.trim_start().len();
    let end = range.start + raw.trim_end().len();
    if start < end {
        let range = start..end;
        if let Some(block) = make_block(source, range.clone(), vec![range], hash, blocks.len()) {
            blocks.push(block);
        }
    }
}

fn make_block(
    source: &str,
    range: Range<usize>,
    editable: Vec<Range<usize>>,
    hash: u64,
    block_index: usize,
) -> Option<Block> {
    let mut editable: Vec<_> = editable
        .into_iter()
        .flat_map(|part| subtract_literal_tokens(source, part))
        .filter(|part| !part.is_empty())
        .collect();
    editable.sort_by_key(|part| (part.start, part.end));
    let mut merged: Vec<Range<usize>> = Vec::new();
    for part in editable {
        if let Some(previous) = merged.last_mut() {
            if part.start <= previous.end {
                previous.end = previous.end.max(part.end);
                continue;
            }
        }
        merged.push(part);
    }
    // A block consisting entirely of code, URLs, numbers or punctuation needs
    // no translation. Leaving it intact also avoids model-generated commentary.
    if !merged
        .iter()
        .any(|part| source[part.clone()].chars().any(char::is_alphabetic))
    {
        return None;
    }

    let mut nonce = hash.wrapping_add(block_index as u64);
    let prefix = loop {
        let prefix = format!("⟦AT{nonce:016x}_");
        if !source.contains(&prefix) {
            break prefix;
        }
        nonce = nonce.wrapping_add(1);
    };
    let mut text = String::new();
    let mut protected = Vec::new();
    let mut cursor = range.start;
    for part in merged {
        if cursor < part.start {
            push_protected(
                &source[cursor..part.start],
                &prefix,
                &mut text,
                &mut protected,
            );
        }
        text.push_str(&source[part.clone()]);
        cursor = part.end;
    }
    if cursor < range.end {
        push_protected(
            &source[cursor..range.end],
            &prefix,
            &mut text,
            &mut protected,
        );
    }
    Some(Block {
        range,
        text,
        protected,
    })
}

fn push_protected(original: &str, prefix: &str, text: &mut String, spans: &mut Vec<ProtectedSpan>) {
    let marker = format!("{prefix}{}⟧", spans.len());
    text.push_str(&marker);
    spans.push(ProtectedSpan {
        marker,
        original: original.to_owned(),
    });
}

/// Preserve bare URLs and literal text that resembles our marker namespace.
fn subtract_literal_tokens(source: &str, range: Range<usize>) -> Vec<Range<usize>> {
    let text = &source[range.clone()];
    let mut exclusions = Vec::new();
    let mut cursor = 0;
    while cursor < text.len() {
        let tail = &text[cursor..];
        if tail.starts_with("⟦AT") {
            let end = tail
                .find('⟧')
                .map(|at| cursor + at + '⟧'.len_utf8())
                .unwrap_or(text.len());
            exclusions.push(cursor..end);
            cursor = end;
        } else if ["https://", "http://", "ftp://", "mailto:"]
            .iter()
            .any(|prefix| tail.starts_with(prefix))
        {
            let length = tail
                .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"'))
                .unwrap_or(tail.len());
            exclusions.push(cursor..cursor + length);
            cursor += length;
        } else {
            cursor += tail.chars().next().expect("nonempty UTF-8 tail").len_utf8();
        }
    }
    let mut remaining = Vec::new();
    let mut cursor = range.start;
    for exclusion in exclusions {
        let start = range.start + exclusion.start;
        let end = range.start + exclusion.end;
        if cursor < start {
            remaining.push(cursor..start);
        }
        cursor = end;
    }
    if cursor < range.end {
        remaining.push(cursor..range.end);
    }
    remaining
}

fn source_hash(source: &str) -> u64 {
    // Stable, non-security FNV-1a namespace; an explicit collision check above
    // guarantees that generated markers cannot already occur in the source.
    source.bytes().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}

fn validate_ranges(source: &str, blocks: &[Block]) -> Result<()> {
    let mut previous_end = 0;
    for block in blocks {
        if block.range.start < previous_end
            || block.range.start > block.range.end
            || source.get(block.range.clone()).is_none()
        {
            return Err(Error::InvalidConfig(
                "overlapping or invalid document byte ranges".into(),
            ));
        }
        previous_end = block.range.end;
    }
    Ok(())
}

#[derive(PartialEq)]
struct Structure {
    events: Vec<Event<'static>>,
    table_pipes: Vec<usize>,
    definitions: std::collections::BTreeMap<String, (String, Option<String>)>,
}

fn structure(source: &str) -> Structure {
    let parser = Parser::new_ext(source, markdown_options());
    let definitions = parser
        .reference_definitions()
        .iter()
        .map(|(label, definition)| {
            (
                label.to_owned(),
                (
                    definition.dest.to_string(),
                    definition.title.as_ref().map(ToString::to_string),
                ),
            )
        })
        .collect();
    let mut events = Vec::new();
    let mut table_pipes = Vec::new();
    for (event, range) in parser.into_offset_iter() {
        // GFM discards extra body cells after the header's column count. Merely
        // comparing AST tag counts would miss a new pipe that pushes an existing
        // cell's text into that discarded area. Original pipes (including escaped
        // ones inside text/code) are immutable, so their row counts cannot change.
        if matches!(event, Event::Start(Tag::TableHead | Tag::TableRow)) {
            table_pipes.push(source[range].bytes().filter(|byte| *byte == b'|').count());
        }
        if !matches!(event, Event::Text(_)) {
            events.push(event.into_static());
        }
    }
    Structure {
        events,
        table_pipes,
        definitions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(document: Document) -> DocumentPlan {
        let plan = plan(&document).unwrap();
        let replacements: Vec<_> = plan
            .blocks
            .iter()
            .map(|block| block.restore(&block.text).unwrap())
            .collect();
        assert_eq!(render(&plan, &replacements).unwrap(), document.text());
        plan
    }

    fn replace(document: Document, from: &str, to: &str) -> String {
        let plan = plan(&document).unwrap();
        let replacements: Vec<_> = plan
            .blocks
            .iter()
            .map(|block| block.restore(&block.text.replace(from, to)).unwrap())
            .collect();
        render(&plan, &replacements).unwrap()
    }

    #[test]
    fn markdown_roundtrips_real_article_structures() {
        let input = concat!(
            "---\r\ntitle: Unchanged metadata\r\n---\r\n\r\n",
            "# A heading {#stable .class}\r\n\r\n",
            "A **bold *nested* phrase**, [label](https://example.test/a \"title\") ",
            "and `let x = 1;` remain in one paragraph.  \r\nNext line.\r\n\r\n",
            "> Quote first\r\n> continued\r\n>\r\n> - Nested item\r\n\r\n",
            "- [x] First item\r\n  continued\r\n  - Inner item\r\n",
            "- Another item\r\n\r\n",
            "| Name | Explanation |\r\n| :--- | ---: |\r\n| value | A **description** |\r\n\r\n",
            "```rust\r\n// Do not translate code\r\nprintln!(\"Hello\");\r\n```\r\n\r\n",
            "    // Indented code is untouched\r\n\r\n",
            "A ![diagram alt](diagram.png) and $x + y$ with $$z = 1$$.\r\n\r\n",
            "<div>HTML block stays unchanged.</div>\r\n\r\n",
            "A <span title=\"unchanged\">visible phrase</span>.\r\n\r\n",
            "A [named reference][ref] and [ref][] and [ref], also <https://example.test>.\r\n\r\n",
            "[ref]: https://example.test \"untouched title\"\r\n\r\n",
            "Footnote here[^one].\r\n\r\n[^one]: Footnote text.\r\n\r\n",
            "中文 café 👩‍💻 and literal &amp; escaped \\* syntax.\r\n",
        );
        let plan = identity(Document::markdown(input));
        assert!(plan.blocks.len() > 10);
        assert!(
            plan.blocks
                .iter()
                .all(|b| !b.text.contains("Do not translate code"))
        );
        assert!(
            plan.blocks
                .iter()
                .all(|b| !b.text.contains("Unchanged metadata"))
        );
    }

    #[test]
    fn groups_a_whole_paragraph_including_inline_formatting() {
        let plan = identity(Document::markdown(
            "A **bold** word, [a link](https://example.test) and `code`.\n\nSecond paragraph.",
        ));
        assert_eq!(plan.blocks.len(), 2);
        let paragraph = &plan.blocks[0];
        assert!(paragraph.text.contains("bold"));
        assert!(paragraph.text.contains("a link"));
        assert!(!paragraph.text.contains("https://"));
        assert!(!paragraph.text.contains("`code`"));
        assert!(!paragraph.protected.is_empty());
    }

    #[test]
    fn can_translate_text_while_preserving_original_syntax_and_urls() {
        let input = "# Hello\n\nHello **Hello** and [Hello](https://example.test/Hello).\n";
        assert_eq!(
            replace(Document::markdown(input), "Hello", "你好"),
            "# 你好\n\n你好 **你好** and [你好](https://example.test/Hello).\n"
        );
    }

    #[test]
    fn tight_nested_lists_and_blockquotes_keep_prefixes_and_breaks() {
        let input = "- Hello\n  continued Hello\n  - Hello\n\n> Hello\n> Hello\n";
        assert_eq!(
            replace(Document::markdown(input), "Hello", "你好"),
            "- 你好\n  continued 你好\n  - 你好\n\n> 你好\n> 你好\n"
        );
    }

    #[test]
    fn opaque_documents_have_no_model_work() {
        for input in [
            "```txt\nHello\n```\n",
            "    Hello\n",
            "<div>Hello</div>\n",
            "![Hello](image.png)\n",
            "$$Hello$$\n",
            "---\ntitle: Hello\n---\n",
            "+++\ntitle = 'Hello'\n+++\n",
            "<https://example.test/Hello>\n",
        ] {
            assert!(
                identity(Document::markdown(input)).blocks.is_empty(),
                "{input}"
            );
        }
    }

    #[test]
    fn links_that_use_display_text_as_id_are_preserved() {
        let input = "Hello [Hello][] [Hello] [Hello][id] <https://example.test/Hello>\n\n[Hello]: https://example.test\n[id]: https://example.test\n";
        let output = replace(Document::markdown(input), "Hello", "你好");
        assert_eq!(
            output,
            "你好 [Hello][] [Hello] [你好][id] <https://example.test/Hello>\n\n[Hello]: https://example.test\n[id]: https://example.test\n"
        );
    }

    #[test]
    fn plain_paragraphs_preserve_separators_edges_and_bare_urls() {
        let input = " \r\n  Hello world.\r\nContinued Hello.  \r\n\t\r\nHello https://example.test/Hello\r\n";
        let plan = identity(Document::plain(input));
        assert_eq!(plan.blocks.len(), 2);
        assert_eq!(
            replace(Document::plain(input), "Hello", "你好"),
            " \r\n  你好 world.\r\nContinued 你好.  \r\n\t\r\n你好 https://example.test/Hello\r\n"
        );
    }

    #[test]
    fn source_marker_literals_cannot_collide_with_generated_markers() {
        let input = "Hello literal ⟦AT0000000000000000_0⟧ and code `example`.";
        let plan = identity(Document::markdown(input));
        assert!(!plan.blocks[0].text.contains("⟦AT0000000000000000_0⟧"));
        assert_eq!(
            replace(Document::markdown(input), "Hello", "你好"),
            "你好 literal ⟦AT0000000000000000_0⟧ and code `example`."
        );
    }

    #[test]
    fn rejects_lost_duplicated_reordered_unknown_or_unclosed_markers() {
        let plan = plan(&Document::markdown("Hello `one` and `two`. ")).unwrap();
        let block = &plan.blocks[0];
        let one = &block.protected[0].marker;
        let two = &block.protected[1].marker;
        for bad in [
            block.text.replace(one, ""),
            format!("{} {one}", block.text),
            block
                .text
                .replace(one, "SWAP")
                .replace(two, one)
                .replace("SWAP", two),
            format!("{} ⟦ATforeign_0⟧", block.text),
            format!("{} ⟦ATunterminated", block.text),
        ] {
            assert!(matches!(
                block.restore(&bad),
                Err(Error::InvalidProtectedText(_))
            ));
        }
    }

    #[test]
    fn rejects_new_markdown_structure_even_when_existing_markers_survive() {
        let plan = plan(&Document::markdown("Hello world.\n")).unwrap();
        for bad in [
            "# A new heading",
            "[link](https://malicious.test)",
            "`new code`",
            "first\n\nsecond",
        ] {
            assert!(matches!(
                render(&plan, &[bad.into()]),
                Err(Error::InvalidProtectedText(_))
            ));
        }
    }

    #[test]
    fn validates_a_block_before_reporting_it_as_complete() {
        let plan = plan(&Document::markdown("Hello world.\n\nAnother paragraph.\n")).unwrap();
        assert!(validate_block(&plan, 0, "你好，世界。").is_ok());
        assert!(matches!(
            validate_block(&plan, 0, "# Introduced heading"),
            Err(Error::InvalidProtectedText(_))
        ));
        assert!(validate_block(&plan, 5, "out of range").is_err());
        let plain = super::plan(&Document::plain("Hello world.")).unwrap();
        assert!(validate_block(&plain, 0, "# Just plain text").is_ok());
    }

    #[test]
    fn rejects_table_cells_that_would_silently_displace_existing_content() {
        let source = "| First | Second |\n| --- | --- |\n| Hello | Preserve me |\n";
        let plan = plan(&Document::markdown(source)).unwrap();
        let index = plan
            .blocks
            .iter()
            .position(|block| source[block.range.clone()].trim() == "Hello")
            .unwrap();
        assert!(validate_block(&plan, index, "你好").is_ok());
        assert!(matches!(
            validate_block(&plan, index, "你好 | Unexpected extra cell"),
            Err(Error::InvalidProtectedText(_))
        ));
    }

    #[test]
    fn rejects_new_invisible_reference_definitions() {
        let plan = plan(&Document::markdown("Hello world.\n")).unwrap();
        assert!(matches!(
            validate_block(&plan, 0, "[new]: https://example.test\nTranslated prose."),
            Err(Error::InvalidProtectedText(_))
        ));
    }

    #[test]
    fn unicode_byte_offsets_and_empty_input_are_safe() {
        for text in ["", "\n\r\n", "你好 **世界** 👩‍💻 café.\n", "# 2026\n\n42\n"] {
            identity(Document::markdown(text));
            identity(Document::plain(text));
        }
    }
}
