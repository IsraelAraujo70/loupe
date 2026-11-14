/// Linear issue identifiers mentioned in free text such as a branch name, a
/// pull request title, or a body: a team key (a letter, then up to nine
/// letters or digits) followed by `-` and a number, as in `ENG-123` or the
/// lowercase `eng-123` Linear puts in branch names. Matches are uppercased,
/// deduplicated, and returned in order of first appearance. A key must start
/// at a word boundary and runs to the `-`, so `abcENG-1` reads as the key
/// `ABCENG`, never as `ENG-1`, and a word longer than ten characters or
/// starting with a digit matches nothing.
pub fn extract_identifiers(text: &str) -> Vec<String> {
    const MAX_KEY_LEN: usize = 10;

    let chars: Vec<char> = text.chars().collect();
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut found: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_alphanumeric() || (i > 0 && is_word(chars[i - 1])) {
            i += 1;
            continue;
        }
        let word_end = chars[i..]
            .iter()
            .position(|c| !c.is_alphanumeric())
            .map_or(chars.len(), |offset| i + offset);
        let key = &chars[i..word_end];
        let key_ok = key.len() <= MAX_KEY_LEN
            && key[0].is_ascii_alphabetic()
            && key.iter().all(char::is_ascii_alphanumeric);
        if key_ok && chars.get(word_end) == Some(&'-') {
            let digits_end = chars[word_end + 1..]
                .iter()
                .position(|c| !c.is_ascii_digit())
                .map_or(chars.len(), |offset| word_end + 1 + offset);
            let has_digits = digits_end > word_end + 1;
            let ends_word = chars.get(digits_end).is_none_or(|c| !c.is_alphanumeric());
            if has_digits && ends_word {
                let identifier: String = chars[i..digits_end]
                    .iter()
                    .collect::<String>()
                    .to_ascii_uppercase();
                if !found.contains(&identifier) {
                    found.push(identifier);
                }
                i = digits_end;
                continue;
            }
        }
        i = word_end.max(i + 1);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::extract_identifiers;

    fn ids(text: &str) -> Vec<String> {
        extract_identifiers(text)
    }

    #[test]
    fn lowercase_branch_name() {
        assert_eq!(ids("israel/eng-123-fix-login"), ["ENG-123"]);
    }

    #[test]
    fn uppercase_and_title() {
        assert_eq!(
            ids("[ENG-42] Fix the thing (closes DES-7)"),
            ["ENG-42", "DES-7"]
        );
    }

    #[test]
    fn dedupes_in_first_appearance_order() {
        assert_eq!(
            ids("ENG-1 then des-2 then eng-1 and ENG-3, DES-2"),
            ["ENG-1", "DES-2", "ENG-3"]
        );
    }

    #[test]
    fn requires_a_word_boundary_before_the_key() {
        assert_eq!(ids("abcENG-1"), ["ABCENG-1"], "the whole word is the key");
        assert!(!ids("abcENG-1").contains(&"ENG-1".to_owned()));
        assert!(ids("1ENG-1").is_empty());
        assert!(ids("_ENG-1").is_empty());
        assert_eq!(ids("(ENG-1)"), ["ENG-1"]);
        assert_eq!(ids("fix/ENG-1"), ["ENG-1"]);
    }

    #[test]
    fn requires_a_number() {
        assert!(ids("ENG-").is_empty());
        assert!(ids("ENG- 12").is_empty());
        assert!(ids("ENG-abc").is_empty());
    }

    #[test]
    fn rejects_a_number_glued_to_letters() {
        assert!(ids("ENG-12abc").is_empty());
        assert_eq!(ids("ENG-12 abc"), ["ENG-12"]);
    }

    #[test]
    fn key_is_one_to_ten_characters_and_starts_with_a_letter() {
        assert_eq!(ids("ABCDEFGHIJ-5"), ["ABCDEFGHIJ-5"]);
        assert!(ids("ABCDEFGHIJK-5").is_empty());
        assert_eq!(ids("A1B2-9"), ["A1B2-9"]);
        assert!(ids("1AB-5").is_empty());
        assert_eq!(ids("X-1"), ["X-1"]);
    }

    #[test]
    fn number_may_be_followed_by_more_segments() {
        assert_eq!(ids("eng-123-456"), ["ENG-123"]);
        assert_eq!(ids("ENG-9."), ["ENG-9"]);
    }

    #[test]
    fn empty_and_unrelated_text() {
        assert!(ids("").is_empty());
        assert!(ids("just words and 1234").is_empty());
    }

    #[test]
    fn non_ascii_text_is_safe() {
        assert_eq!(ids("correção — ENG-5 é isso"), ["ENG-5"]);
        assert!(ids("éENG-5").is_empty());
    }
}
