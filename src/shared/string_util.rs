use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

pub(crate) trait CharExt {
    fn is_regex_special_char(&self) -> bool;
}

pub(crate) trait StringExt {
    fn escape_regex_chars(&self) -> String;
    fn from_utf8_lossy_as_owned(v: Vec<u8>) -> String;
    fn trim_end_in_place(&mut self);
}

/// Fold text for fast case-insensitive, diacritic-insensitive matching.
///
/// This preserves base characters while:
/// - lowercasing
/// - stripping combining marks
/// - folding Vietnamese `đ`/`Đ` to `d`
/// - removing control characters
/// - collapsing whitespace
#[must_use]
pub(crate) fn fold_for_match(input: &str) -> String {
    let mut folded = String::with_capacity(input.len());
    let mut last_was_whitespace = false;

    for ch in input.trim().nfkd().flat_map(char::to_lowercase) {
        if is_combining_mark(ch) || ch.is_control() {
            continue;
        }

        let folded_char = match ch {
            'đ' => 'd',
            _ => ch,
        };

        if folded_char.is_whitespace() {
            if !folded.is_empty() && !last_was_whitespace {
                folded.push(' ');
            }
            last_was_whitespace = true;
            continue;
        }

        folded.push(folded_char);
        last_was_whitespace = false;
    }

    folded
}

impl StringExt for String {
    fn escape_regex_chars(&self) -> String {
        let mut buf = String::with_capacity(self.len());
        for char in self.chars() {
            if char.is_regex_special_char() {
                buf.push('\\');
            }
            buf.push(char);
        }
        buf
    }

    fn from_utf8_lossy_as_owned(v: Vec<u8>) -> String {
        if let std::borrow::Cow::Owned(string) = String::from_utf8_lossy(&v) {
            string
        } else {
            // SAFETY: `String::from_utf8_lossy`'s guaranteec valid utf8 when a borrowed
            // variant is returned. Owned value, meaning invalid utf8, is handled above.
            unsafe { String::from_utf8_unchecked(v) }
        }
    }

    fn trim_end_in_place(&mut self) {
        let trimmed_len = str::trim_end(self).len();
        if trimmed_len < self.len() {
            self.truncate(trimmed_len);
        }
    }
}

impl CharExt for char {
    fn is_regex_special_char(&self) -> bool {
        matches!(
            self,
            '\\' | '.'
                | '+'
                | '*'
                | '?'
                | '('
                | ')'
                | '|'
                | '['
                | ']'
                | '{'
                | '}'
                | '^'
                | '$'
                | '#'
                | '&'
                | '-'
                | '~'
        )
    }
}

#[cfg(test)]
mod tests {
    use super::fold_for_match;

    #[test]
    fn fold_for_match_strips_vietnamese_diacritics() {
        assert_eq!(fold_for_match("Hoàng Dũng"), "hoang dung");
    }

    #[test]
    fn fold_for_match_folds_vietnamese_d_stroke() {
        assert_eq!(fold_for_match("Đặng Thái Sơn"), "dang thai son");
    }

    #[test]
    fn fold_for_match_collapses_whitespace_and_controls() {
        assert_eq!(fold_for_match("  hoàng\n\t dũng\u{0}  "), "hoang dung");
    }
}
