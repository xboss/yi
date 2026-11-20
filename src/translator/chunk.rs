//! Splits only when a backend's measured input exceeds its budget.
use crate::translator::document;
use crate::translator::error::{Error, Result};
use unicode_segmentation::UnicodeSegmentation;

/// Returns protected markers in order. A malformed reserved marker is rejected.
pub(crate) fn markers(text: &str) -> Result<Vec<&str>> {
    Ok(document::marker_ranges(text)?
        .into_iter()
        .map(|range| &text[range])
        .collect())
}

pub(crate) fn verify_markers(source: &str, translation: &str) -> Result<()> {
    if markers(source)? != markers(translation)? {
        return Err(Error::InvalidProtectedText(
            "missing, duplicated, reordered or invented marker".into(),
        ));
    }
    Ok(())
}

/// Split near the middle, preferring sentence boundaries, then word boundaries,
/// then graphemes. Never bisect a protected marker or a grapheme cluster.
pub(crate) fn bisect(text: &str) -> Option<(&str, &str, &str)> {
    let protected: Vec<(usize, usize)> = markers(text)
        .ok()?
        .into_iter()
        .map(|m| {
            let start = m.as_ptr() as usize - text.as_ptr() as usize;
            (start, start + m.len())
        })
        .collect();
    let valid =
        |i: usize| i > 0 && i < text.len() && !protected.iter().any(|&(a, b)| a < i && i < b);
    let midpoint = text.len() / 2;
    let reasonable = |i: usize| i >= text.len() / 5 && i <= text.len() * 4 / 5;
    let nearest = |positions: Vec<usize>| {
        positions
            .into_iter()
            .filter(|&i| valid(i) && reasonable(i))
            .min_by_key(|i| i.abs_diff(midpoint))
    };
    let sentences: Vec<usize> = text
        .split_sentence_bound_indices()
        .map(|(i, s)| i + s.len())
        .collect();
    let words: Vec<usize> = text
        .split_word_bound_indices()
        .map(|(i, s)| i + s.len())
        .collect();
    let graphemes: Vec<usize> = text
        .grapheme_indices(true)
        .map(|(i, _)| i)
        .filter(|&i| valid(i))
        .collect();
    let i = nearest(sentences)
        .or_else(|| nearest(words))
        .or_else(|| graphemes.into_iter().min_by_key(|i| i.abs_diff(midpoint)))?;
    let left = text[..i].trim_end_matches(char::is_whitespace);
    let right = text[i..].trim_start_matches(char::is_whitespace);
    if left.is_empty() || right.is_empty() {
        return None;
    }
    Some((left, &text[left.len()..text.len() - right.len()], right))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_unicode_markers_and_source_bytes() {
        let text = "Hello 👩‍💻. ⟦ATabc_0⟧ 你好。Next paragraph words.";
        let (a, gap, b) = bisect(text).unwrap();
        assert_eq!(format!("{a}{gap}{b}"), text);
        assert!(markers(a).is_ok() && markers(b).is_ok());
        assert!(!a.ends_with('\u{200d}'));
        assert!(verify_markers("A ⟦ATa_0⟧ B", "A B").is_err());
    }
    #[test]
    fn a_marker_is_indivisible() {
        assert!(bisect("⟦ATabcdef123_0⟧").is_none());
    }
}
