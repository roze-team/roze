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

pub fn to_snake_case(value: impl AsRef<str>) -> String {
    let value = value.as_ref().trim();
    let mut out = String::new();
    let mut chars = value.chars().peekable();
    let mut previous_was_lower_or_digit = false;

    while let Some(ch) = chars.next() {
        let next_is_lower = chars
            .peek()
            .map(|next| next.is_ascii_lowercase())
            .unwrap_or(false);
        match ch {
            'A'..='Z' => {
                if !out.is_empty() && (previous_was_lower_or_digit || next_is_lower) {
                    out.push('_');
                }
                out.push(ch.to_ascii_lowercase());
                previous_was_lower_or_digit = false;
            }
            'a'..='z' | '0'..='9' => {
                out.push(ch);
                previous_was_lower_or_digit = true;
            }
            '-' | ' ' | '.' | '/' | ':' => {
                if !out.ends_with('_') && !out.is_empty() {
                    out.push('_');
                }
                previous_was_lower_or_digit = false;
            }
            '_' => {
                if !out.ends_with('_') && !out.is_empty() {
                    out.push('_');
                }
                previous_was_lower_or_digit = false;
            }
            _ => {
                if !out.ends_with('_') && !out.is_empty() {
                    out.push('_');
                }
                previous_was_lower_or_digit = false;
            }
        }
    }

    out.trim_matches('_').to_string()
}

pub fn to_camel_case(value: impl AsRef<str>) -> String {
    let mut out = String::new();
    for part in value
        .as_ref()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
    {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            out.extend(chars.map(|ch| ch.to_ascii_lowercase()));
        }
    }
    out
}

pub fn normalize_path_segment(value: impl AsRef<str>) -> String {
    value
        .as_ref()
        .trim()
        .chars()
        .map(|ch| match ch {
            'A'..='Z' => ch.to_ascii_lowercase(),
            'a'..='z' | '0'..='9' | '-' | '_' => ch,
            _ => '-',
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

pub fn truncate_with_ellipsis(value: impl AsRef<str>, max_len: usize) -> String {
    let value = value.as_ref();
    if value.len() <= max_len {
        return value.to_string();
    }
    if max_len <= 3 {
        return "...".chars().take(max_len).collect();
    }
    let mut out = value.chars().take(max_len - 3).collect::<String>();
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_strings() {
        assert!(is_blank("   "));
        assert_eq!(normalize_name(" Roze Service "), "roze-service");
        assert_eq!(join_non_empty(&["a", "", "b"]), "a.b");
        assert_eq!(to_snake_case("RozeHTTPService"), "roze_http_service");
        assert_eq!(to_camel_case("roze-http_service"), "RozeHttpService");
        assert_eq!(normalize_path_segment(" /v1/Users "), "v1-users");
        assert_eq!(truncate_with_ellipsis("abcdefgh", 5), "ab...");
    }
}
