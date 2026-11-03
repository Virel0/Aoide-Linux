//! Whether what was typed is a description worth sending to the server's AI,
//! or a name the plain search handles better.

/// Below this many words a phrase is a name, which the plain search handles
/// better, and every call counts against the hour's AI searches.
pub const MINIMUM_PHRASE_WORDS: usize = 3;

/// Whether `text` is worth sending to a model at all.
pub fn looks_like_a_phrase(text: &str) -> bool {
    text.split_whitespace().count() >= MINIMUM_PHRASE_WORDS
}

#[cfg(test)]
mod tests {
    use super::*;

    // When to ask a model at all.

    #[test]
    fn leaves_one_or_two_words_to_the_plain_search() {
        assert!(!looks_like_a_phrase("Radiohead"));
        assert!(!looks_like_a_phrase("kid a"));
        assert_eq!(MINIMUM_PHRASE_WORDS, 3);
    }

    #[test]
    fn offers_to_translate_a_real_phrase() {
        assert!(looks_like_a_phrase("upbeat rock from the nineties"));
    }

    #[test]
    fn is_not_fooled_by_spacing() {
        assert!(!looks_like_a_phrase("   jazz   "));
        assert!(looks_like_a_phrase("  quiet   jazz   for   evening "));
    }
}
