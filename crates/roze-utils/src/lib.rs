pub fn is_blank(value: impl AsRef<str>) -> bool {
    value.as_ref().trim().is_empty()
}

pub fn normalize_name(value: impl AsRef<str>) -> String {
    value
        .as_ref()
        .trim()
        .chars()
        .map(|ch| match ch {
            'A'..='Z' => ch.to_ascii_lowercase(),
            'a'..='z' | '0'..='9' | '-' | '_' => ch,
            _ => '-',
        })
        .collect()
}

pub fn join_non_empty(parts: &[impl AsRef<str>]) -> String {
    parts
        .iter()
        .map(|part| part.as_ref().trim())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_strings() {
        assert!(is_blank("   "));
        assert_eq!(normalize_name(" Roze Service "), "roze-service");
        assert_eq!(join_non_empty(&["a", "", "b"]), "a.b");
    }
}
