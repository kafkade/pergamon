//! Reading time estimation and text analytics.
//!
//! Pure-computation helpers for word counting and reading-time estimates.
//! No I/O — safe for WASM and all platforms.

/// Average adult reading speed in words per minute.
///
/// Based on Brysbaert (2019) meta-analysis: 238 WPM for non-fiction.
const READING_WPM: f64 = 238.0;

/// Count the number of words in a text string.
///
/// Uses Unicode-aware whitespace splitting. Returns 0 for empty/whitespace-only input.
#[must_use]
pub fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Estimate reading time in minutes from a word count.
///
/// Uses 238 WPM (average adult non-fiction reading speed).
/// Returns at least 1 minute for any non-zero word count.
#[must_use]
pub fn estimate_reading_minutes(words: usize) -> u32 {
    if words == 0 {
        return 0;
    }
    #[allow(clippy::cast_precision_loss)]
    let minutes = words as f64 / READING_WPM;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let result = (minutes.ceil() as u32).max(1);
    result
}

/// Estimate reading time in minutes directly from text content.
#[must_use]
pub fn reading_time_from_text(text: &str) -> u32 {
    estimate_reading_minutes(word_count(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_count_empty() {
        assert_eq!(word_count(""), 0);
        assert_eq!(word_count("   "), 0);
        assert_eq!(word_count("\n\t "), 0);
    }

    #[test]
    fn word_count_basic() {
        assert_eq!(word_count("hello world"), 2);
        assert_eq!(word_count("one"), 1);
        assert_eq!(word_count("  spaced   out  "), 2);
    }

    #[test]
    fn word_count_unicode() {
        assert_eq!(word_count("日本語 テスト"), 2);
        assert_eq!(word_count("café résumé"), 2);
    }

    #[test]
    fn reading_time_zero_words() {
        assert_eq!(estimate_reading_minutes(0), 0);
    }

    #[test]
    fn reading_time_short_text() {
        // 100 words = 0.42 min → ceil → 1 min
        assert_eq!(estimate_reading_minutes(100), 1);
    }

    #[test]
    fn reading_time_medium_text() {
        // 500 words = 2.1 min → ceil → 3 min
        assert_eq!(estimate_reading_minutes(500), 3);
    }

    #[test]
    fn reading_time_long_text() {
        // 2000 words = 8.4 min → ceil → 9 min
        assert_eq!(estimate_reading_minutes(2000), 9);
    }

    #[test]
    fn reading_time_from_text_integration() {
        let text = "word ".repeat(238); // exactly 238 words = 1 minute
        assert_eq!(reading_time_from_text(&text), 1);

        let text2 = "word ".repeat(239); // 239 words → ceil(1.004) = 2
        assert_eq!(reading_time_from_text(&text2), 2);
    }

    #[test]
    fn reading_time_within_tolerance() {
        // Verify ±20% accuracy for a 10-minute article (~2380 words).
        // At 238 WPM, 2380 words = exactly 10 minutes.
        let minutes = estimate_reading_minutes(2380);
        assert_eq!(minutes, 10);

        // A real 10-min article at 200-280 WPM range → 2000-2800 words.
        // Our estimate for 2400 words: ceil(2400/238) = ceil(10.08) = 11.
        // 11 vs actual 10 → 10% error, well within ±20%.
        let estimate = estimate_reading_minutes(2400);
        assert!(
            (8..=12).contains(&estimate),
            "estimate {estimate} not within ±20% of 10"
        );
    }
}
