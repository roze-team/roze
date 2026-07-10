use crate::{
    generator::{rust_identifier, to_snake_case},
    parser::{Field, FieldSource, TypeDef},
};

pub fn render_types(types: &[TypeDef]) -> String {
    let mut out = String::from(
        "#![allow(dead_code)]\n\nuse roze_validation::Validate;\nuse serde::{Deserialize, Serialize};\n\n",
    );

    for ty in types {
        out.push_str("#[derive(Debug, Clone, Default, Serialize, Deserialize, Validate)]\n");
        out.push_str(&format!("pub struct {} {{\n", ty.name));
        for field in &ty.fields {
            if field.embedded {
                out.push_str("    #[serde(flatten)]\n");
            } else if let Some(rename) = serde_rename(field) {
                out.push_str(&format!("    #[serde(rename = \"{}\")]\n", rename));
            }
            if let Some(validate) = validation_attr(field) {
                out.push_str(&format!("    #[validate({validate})]\n"));
            }
            out.push_str(&format!(
                "    pub {}: {},\n",
                rust_field_name(field),
                map_type(&field.ty)
            ));
        }
        out.push_str("}\n\n");
    }

    out
}

fn map_type(ty: &str) -> String {
    let ty = ty.trim();
    if let Some((key, value)) = map_key_value_types(ty) {
        return format!(
            "std::collections::HashMap<{}, {}>",
            map_type(&key),
            map_type(&value)
        );
    }
    if let Some(inner) = collection_element_type(ty) {
        return format!("Vec<{}>", map_type(&inner));
    }

    match ty {
        "string" => "String".to_string(),
        "int" | "int64" => "i64".to_string(),
        "int32" => "i32".to_string(),
        "int16" => "i16".to_string(),
        "int8" => "i8".to_string(),
        "uint" | "uint64" => "u64".to_string(),
        "uint32" => "u32".to_string(),
        "uint16" => "u16".to_string(),
        "uint8" => "u8".to_string(),
        "float" => "f32".to_string(),
        "double" => "f64".to_string(),
        "bool" => "bool".to_string(),
        other => other.to_string(),
    }
}

fn map_key_value_types(ty: &str) -> Option<(String, String)> {
    let ty = ty.trim();
    if let Some(rest) = ty.strip_prefix("map[") {
        let (key, value) = rest.split_once(']')?;
        return Some((
            key.trim_start_matches('*').trim().to_string(),
            value.trim_start_matches('*').trim().to_string(),
        ));
    }
    if let Some(inner) = ty
        .strip_prefix("HashMap<")
        .and_then(|raw| raw.strip_suffix('>'))
    {
        let (key, value) = inner.split_once(',')?;
        return Some((
            key.trim_start_matches('*').trim().to_string(),
            value.trim_start_matches('*').trim().to_string(),
        ));
    }
    None
}

fn collection_element_type(ty: &str) -> Option<String> {
    let ty = ty.trim();
    if let Some(inner) = ty.strip_prefix("[]") {
        return Some(inner.trim_start_matches('*').trim().to_string());
    }
    if let Some(inner) = ty
        .strip_prefix("Vec<")
        .and_then(|raw| raw.strip_suffix('>'))
    {
        return Some(inner.trim_start_matches('*').trim().to_string());
    }
    None
}

fn rust_field_name(field: &Field) -> String {
    rust_identifier(&field.name)
}

fn serde_rename(field: &Field) -> Option<&str> {
    if matches!(field.source, FieldSource::Header) {
        return None;
    }

    let json_name = field.json_name.as_deref().or(field.wire_name.as_deref())?;
    if json_name == to_snake_case(&field.name) {
        None
    } else {
        Some(json_name)
    }
}

fn validation_attr(field: &Field) -> Option<String> {
    let rules = field.validate.as_deref()?;
    if has_rule(rules, "optional") || has_rule(rules, "omitempty") {
        return None;
    }

    if map_key_value_types(&field.ty).is_some() || collection_element_type(&field.ty).is_some() {
        return collection_validation_attr(rules_before_dive(rules));
    }

    match map_type(&field.ty).as_str() {
        "String" => string_validation_attr(rules),
        "i64" | "u64" | "i32" | "u32" => number_validation_attr(rules),
        _ => None,
    }
}

fn string_validation_attr(rules: &str) -> Option<String> {
    let mut attrs = Vec::new();
    if has_rule(rules, "email") {
        attrs.push("email".to_string());
    }
    if has_rule(rules, "url") || has_rule(rules, "uri") {
        attrs.push("url".to_string());
    }
    if has_rule(rules, "ip") {
        attrs.push("ip".to_string());
    } else if has_rule(rules, "ipv4") {
        attrs.push("ip(v4 = true)".to_string());
    } else if has_rule(rules, "ipv6") {
        attrs.push("ip(v6 = true)".to_string());
    }
    if let Some(pattern) = rule_value(rules, "contains") {
        attrs.push(format!("contains = {pattern:?}"));
    }
    if let Some(pattern) = rule_value(rules, "excludes") {
        attrs.push(format!("does_not_contain = {pattern:?}"));
    }

    let (mut min, max) = min_max_rules(rules);
    let equal = rule_value(rules, "len").and_then(parse_usize);
    if has_rule(rules, "required") {
        min.get_or_insert(1usize);
    }

    if let Some(equal) = equal {
        attrs.push(format!("length(equal = {equal})"));
    } else {
        match (min, max) {
            (Some(min), Some(max)) => attrs.push(format!("length(min = {min}, max = {max})")),
            (Some(min), None) => attrs.push(format!("length(min = {min})")),
            (None, Some(max)) => attrs.push(format!("length(max = {max})")),
            (None, None) => {}
        }
    }

    (!attrs.is_empty()).then(|| attrs.join(", "))
}

fn number_validation_attr(rules: &str) -> Option<String> {
    let mut min = rule_value(rules, "min")
        .or_else(|| rule_value(rules, "gte"))
        .filter(|value| is_number_literal(value));
    let mut max = rule_value(rules, "max")
        .or_else(|| rule_value(rules, "lte"))
        .filter(|value| is_number_literal(value));
    if min.is_none() && has_rule(rules, "nonnegative") {
        min = Some("0");
    }
    if has_rule(rules, "page") {
        min.get_or_insert("1");
    }
    if has_rule(rules, "limit") {
        min.get_or_insert("1");
        max.get_or_insert("1000");
    }
    let exclusive_min = rule_value(rules, "gt").filter(|value| is_number_literal(value));
    let exclusive_max = rule_value(rules, "lt").filter(|value| is_number_literal(value));

    let mut parts = Vec::new();
    if let Some(min) = min {
        parts.push(format!("min = {min}"));
    }
    if let Some(max) = max {
        parts.push(format!("max = {max}"));
    }
    if let Some(exclusive_min) = exclusive_min {
        parts.push(format!("exclusive_min = {exclusive_min}"));
    }
    if let Some(exclusive_max) = exclusive_max {
        parts.push(format!("exclusive_max = {exclusive_max}"));
    }

    (!parts.is_empty()).then(|| format!("range({})", parts.join(", ")))
}

fn collection_validation_attr(rules: &str) -> Option<String> {
    let (mut min, max) = min_max_rules(rules);
    let equal = rule_value(rules, "len").and_then(parse_usize);
    if has_rule(rules, "required") {
        min.get_or_insert(1usize);
    }

    if let Some(equal) = equal {
        Some(format!("length(equal = {equal})"))
    } else {
        match (min, max) {
            (Some(min), Some(max)) => Some(format!("length(min = {min}, max = {max})")),
            (Some(min), None) => Some(format!("length(min = {min})")),
            (None, Some(max)) => Some(format!("length(max = {max})")),
            (None, None) => None,
        }
    }
}

fn min_max_rules(rules: &str) -> (Option<usize>, Option<usize>) {
    let min = rule_value(rules, "min")
        .or_else(|| rule_value(rules, "gte"))
        .or_else(|| rule_value(rules, "min_items"))
        .and_then(parse_usize);
    let max = rule_value(rules, "max")
        .or_else(|| rule_value(rules, "lte"))
        .or_else(|| rule_value(rules, "max_items"))
        .and_then(parse_usize);
    (min, max)
}

fn has_rule(rules: &str, name: &str) -> bool {
    rules
        .split(',')
        .map(str::trim)
        .filter(|rule| !rule.is_empty())
        .any(|rule| rule == name)
}

fn rules_before_dive(rules: &str) -> &str {
    rules
        .split_once(",dive")
        .map(|(before, _)| before)
        .unwrap_or(rules)
        .trim()
}

fn rule_value<'a>(rules: &'a str, name: &str) -> Option<&'a str> {
    for rule in rules
        .split(',')
        .map(str::trim)
        .filter(|rule| !rule.is_empty())
    {
        if let Some((key, value)) = rule.split_once('=') {
            if key.trim() == name {
                return Some(value.trim());
            }
        }
    }
    None
}

fn parse_usize(value: &str) -> Option<usize> {
    value.parse::<usize>().ok()
}

fn is_number_literal(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value
            .chars()
            .enumerate()
            .all(|(idx, ch)| ch.is_ascii_digit() || ch == '.' || (idx == 0 && ch == '-'))
}

#[cfg(test)]
mod tests {
    use crate::parser::parse_api;

    use super::*;

    #[test]
    fn renders_default_and_stable_rust_field_names() {
        let spec = parse_api(
            r#"
            service user-api

            type UserResp {
                UserID u64 `json:"user-id"`
                CreatedAt int `json:"created_at"`
                LoginCount int32 `json:"login_count"`
                Balance int64 `json:"balance"`
                Level uint32 `json:"level"`
                Total uint64 `json:"total"`
                Score float `json:"score"`
                Ratio double `json:"ratio"`
                type string `json:"type"`
                Tags []string `json:"tags"`
                Scores []int64 `json:"scores"`
                Labels map[string]string `json:"labels"`
                Weights map[string]uint64 `json:"weights"`
            }
            "#,
        )
        .expect("valid api");

        let rendered = render_types(&spec.types);

        assert!(
            rendered.contains("#[derive(Debug, Clone, Default, Serialize, Deserialize, Validate)]")
        );
        assert!(rendered.contains("#[serde(rename = \"user-id\")]"));
        assert!(rendered.contains("pub user_i_d: u64,"));
        assert!(rendered.contains("pub created_at: i64,"));
        assert!(rendered.contains("pub login_count: i32,"));
        assert!(rendered.contains("pub balance: i64,"));
        assert!(rendered.contains("pub level: u32,"));
        assert!(rendered.contains("pub total: u64,"));
        assert!(rendered.contains("pub score: f32,"));
        assert!(rendered.contains("pub ratio: f64,"));
        assert!(rendered.contains("pub r#type: String,"));
        assert!(rendered.contains("pub tags: Vec<String>,"));
        assert!(rendered.contains("pub scores: Vec<i64>,"));
        assert!(rendered.contains("pub labels: std::collections::HashMap<String, String>,"));
        assert!(rendered.contains("pub weights: std::collections::HashMap<String, u64>,"));
        assert!(!rendered.contains(": int64,"));
        assert!(!rendered.contains(": uint64,"));
        assert!(!rendered.contains(": float,"));
        assert!(!rendered.contains(": double,"));
        assert!(!rendered.contains("pub user-id"));
    }

    #[test]
    fn renders_validation_attributes() {
        let spec = parse_api(
            r#"
            service user-api

            type CreateUserReq {
                nickname String `json:"nickname" validate:"required,min=2,max=16"`
                age int `json:"age" validate:"gte=1,lte=120"`
                tags []string `json:"tags" validate:"min=1,dive,required"`
                offset int `json:"offset" validate:"nonnegative"`
                page int `json:"page" validate:"page"`
                limit int `json:"limit" validate:"limit"`
                codes []string `json:"codes" validate:"min_items=1,max_items=3,dive,code"`
            }
            "#,
        )
        .expect("valid api");

        let rendered = render_types(&spec.types);

        assert!(rendered.contains("use roze_validation::Validate;"));
        assert!(rendered.contains("#[validate(length(min = 2, max = 16))]"));
        assert!(rendered.contains("#[validate(range(min = 1, max = 120))]"));
        assert!(rendered.contains("#[validate(length(min = 1))]"));
        assert!(rendered.contains("#[validate(range(min = 0))]"));
        assert!(rendered.contains("#[validate(range(min = 1))]"));
        assert!(rendered.contains("#[validate(range(min = 1, max = 1000))]"));
        assert!(rendered.contains("#[validate(length(min = 1, max = 3))]"));
    }

    #[test]
    fn renders_anonymous_embedded_types_as_flattened_fields() {
        let spec = parse_api(
            r#"
            service user-api

            type (
                BaseReq {
                    traceId string `json:"traceId,optional" validate:"optional"`
                }
                CreateUserReq {
                    BaseReq
                    name string `json:"name"`
                }
            )
            "#,
        )
        .expect("valid api");

        let rendered = render_types(&spec.types);

        assert!(rendered.contains("    #[serde(flatten)]\n    pub base_req: BaseReq,"));
        assert!(!rendered.contains("#[serde(rename = \"base_req\")]"));
    }
}
