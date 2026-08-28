use regex::{NoExpand, Regex};
use std::collections::BTreeMap;
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

/// Builds the pattern for one spoken phrase.
///
/// Matching ignores diacritics as well as case: a user types "a la ligne" in
/// the settings while the engine writes "À la ligne", and both must be the
/// same command. Every character of the phrase is stripped of its combining
/// marks and allowed to carry any in the text, which works for every language
/// instead of a hand-written table of accented letters.
///
/// It also requires whole words, and absorbs the surrounding spaces along with
/// the punctuation the engine added after the phrase, so "... new line. Then"
/// yields a single break.
fn pattern(spoken: &str) -> String {
    let mut out = String::from(r"(?i)[ \t]*\b");
    for c in spoken.nfd().filter(|c| !is_combining_mark(*c)) {
        out.push_str(&regex::escape(c.encode_utf8(&mut [0u8; 4])));
        out.push_str(r"\p{Mn}*");
    }
    out.push_str(r"\b[[:punct:]]*[ \t]*");
    out
}

/// Applies the user's text replacements to the transcript: every recognised
/// phrase is swapped for the associated text, for instance "new line" for a
/// line break.
///
/// The text is matched in decomposed form (NFD) so the patterns above can see
/// accents as separate marks, and recomposed (NFC) on the way out.
pub fn apply_replacements(text: &str, replacements: &BTreeMap<String, String>) -> String {
    let mut out: String = text.nfd().collect();
    for (spoken, replacement) in replacements {
        if spoken.trim().is_empty() {
            continue;
        }
        match Regex::new(&pattern(spoken)) {
            // NoExpand: a replacement is literal text, "$1" is not a capture
            // reference the user meant to write.
            Ok(re) => out = re.replace_all(&out, NoExpand(replacement)).into_owned(),
            Err(e) => log::warn!("replacement {spoken:?} ignored: {e}"),
        }
    }
    out.nfc().collect::<String>().trim().to_string()
}

/// A replacement is usually a control character (`\n`, `\t`), which a
/// single-line entry cannot hold. The settings window shows it escaped and
/// converts back on save.
pub fn escape(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\n' => "\\n".to_string(),
            '\t' => "\\t".to_string(),
            '\\' => "\\\\".to_string(),
            c => c.to_string(),
        })
        .collect()
}

pub fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            // Unknown escape: keep it verbatim rather than swallow the
            // backslash the user typed.
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replacements() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("à la ligne".to_string(), "\n".to_string()),
            ("nouveau paragraphe".to_string(), "\n\n".to_string()),
        ])
    }

    #[test]
    fn replaces_a_spoken_phrase() {
        assert_eq!(
            apply_replacements("Bonjour à la ligne comment ça va", &replacements()),
            "Bonjour\ncomment ça va"
        );
    }

    #[test]
    fn absorbs_punctuation_added_by_the_engine() {
        assert_eq!(
            apply_replacements("Bonjour. À la ligne. Comment ça va ?", &replacements()),
            "Bonjour.\nComment ça va ?"
        );
    }

    #[test]
    fn leaves_text_untouched_without_replacements() {
        let text = "Le mot ligne apparaît ici.";
        assert_eq!(apply_replacements(text, &replacements()), text);
        assert_eq!(apply_replacements(text, &BTreeMap::new()), text);
    }

    /// The exact case reported: the phrase was typed unaccented in the
    /// settings, the engine wrote it accented, and nothing was replaced.
    #[test]
    fn ignores_accents_and_case() {
        let cmds = BTreeMap::from([("A la ligne".to_string(), "\n".to_string())]);
        assert_eq!(
            apply_replacements("Ceci est un test à la ligne. Voici un autre test.", &cmds),
            "Ceci est un test\nVoici un autre test."
        );
        assert_eq!(apply_replacements("Un À LA LIGNE deux", &cmds), "Un\ndeux");
    }

    /// An accented phrase must still match unaccented speech, not only the
    /// other way round.
    #[test]
    fn matches_in_both_directions() {
        let cmds = BTreeMap::from([("à la ligne".to_string(), "\n".to_string())]);
        assert_eq!(apply_replacements("un a la ligne deux", &cmds), "un\ndeux");
    }

    /// A replacement is literal: "$" must not be read as a capture reference.
    #[test]
    fn replacement_is_literal() {
        let cmds = BTreeMap::from([("dollar".to_string(), "$1".to_string())]);
        assert_eq!(apply_replacements("un dollar deux", &cmds), "un$1deux");
    }

    #[test]
    fn escaping_round_trips() {
        for s in ["\n", "\n\n", "\t", "a\\b", "plain", ""] {
            assert_eq!(unescape(&escape(s)), s, "{s:?}");
        }
        assert_eq!(escape("\n"), "\\n");
        assert_eq!(unescape("\\q"), "\\q");
    }
}
