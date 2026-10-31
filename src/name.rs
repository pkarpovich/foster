const MAX_LEN: usize = 80;

#[derive(Debug, PartialEq)]
pub struct SessionName(String);

impl SessionName {
    pub fn parse(text: &str) -> Result<SessionName, String> {
        if text.len() > MAX_LEN {
            return Err(format!(
                "name {text} is {} characters, at most {MAX_LEN} fit",
                text.len()
            ));
        }
        let valid_length = !text.is_empty();
        let mut valid_chars = true;
        for character in text.chars() {
            if !(character.is_ascii_alphanumeric() || "-_.:".contains(character)) {
                valid_chars = false;
            }
        }
        if !valid_length || !valid_chars || text == "." || text == ".." || text.starts_with('-') {
            return Err(format!("invalid name {text}: use letters, digits, - _ . :"));
        }
        Ok(SessionName(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_names() {
        let long = "a".repeat(80);
        for text in ["web", "content-oracle:ui:dev", "a_b.c", long.as_str()] {
            let name = SessionName::parse(text).expect(text);
            assert_eq!(name.as_str(), text);
        }
    }

    #[test]
    fn rejects_invalid_names() {
        for text in ["", ".", "..", "a b", "a/b", "é", "-h", "--force"] {
            let error = SessionName::parse(text).expect_err(text);
            assert_eq!(
                error,
                format!("invalid name {text}: use letters, digits, - _ . :")
            );
        }
    }

    #[test]
    fn rejects_a_long_name_with_its_length() {
        let long = "a".repeat(81);
        assert_eq!(
            SessionName::parse(&long),
            Err(format!("name {long} is 81 characters, at most 80 fit"))
        );
    }
}
