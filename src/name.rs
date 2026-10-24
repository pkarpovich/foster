const MAX_LEN: usize = 50;

#[derive(Debug, PartialEq)]
pub struct SessionName(String);

impl SessionName {
    pub fn parse(text: &str) -> Result<SessionName, String> {
        let valid_length = !text.is_empty() && text.len() <= MAX_LEN;
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
        let long = "a".repeat(50);
        for text in ["web", "content-oracle:ui:dev", "a_b.c", long.as_str()] {
            let name = SessionName::parse(text).expect(text);
            assert_eq!(name.as_str(), text);
        }
    }

    #[test]
    fn rejects_invalid_names() {
        let long = "a".repeat(51);
        for text in [
            "",
            long.as_str(),
            ".",
            "..",
            "a b",
            "a/b",
            "é",
            "-h",
            "--force",
        ] {
            let error = SessionName::parse(text).expect_err(text);
            assert_eq!(
                error,
                format!("invalid name {text}: use letters, digits, - _ . :")
            );
        }
    }
}
