use crate::{
    generator::to_snake_case,
    parser::{Field, FieldSource, TypeDef},
};

pub fn render_types(types: &[TypeDef]) -> String {
    let mut out = String::from("#![allow(dead_code)]\n\nuse serde::{Deserialize, Serialize};\n\n");

    for ty in types {
        out.push_str("#[derive(Debug, Clone, Default, Serialize, Deserialize)]\n");
        out.push_str(&format!("pub struct {} {{\n", ty.name));
        for field in &ty.fields {
            if let Some(rename) = serde_rename(field) {
                out.push_str(&format!("    #[serde(rename = \"{}\")]\n", rename));
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
        "int" => "i64".to_string(),
        "uint" => "u64".to_string(),
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
    to_snake_case(&field.name)
}

fn serde_rename(field: &Field) -> Option<&str> {
    if matches!(field.source, FieldSource::Header) {
        return None;
    }

    let json_name = field.json_name.as_deref().or(field.wire_name.as_deref())?;
    if json_name == rust_field_name(field) {
        None
    } else {
        Some(json_name)
    }
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
                Tags []string `json:"tags"`
                Scores []int `json:"scores"`
                Labels map[string]string `json:"labels"`
                Weights map[string]int `json:"weights"`
            }
            "#,
        )
        .expect("valid api");

        let rendered = render_types(&spec.types);

        assert!(rendered.contains("#[derive(Debug, Clone, Default, Serialize, Deserialize)]"));
        assert!(rendered.contains("#[serde(rename = \"user-id\")]"));
        assert!(rendered.contains("pub user_i_d: u64,"));
        assert!(rendered.contains("pub created_at: i64,"));
        assert!(rendered.contains("pub tags: Vec<String>,"));
        assert!(rendered.contains("pub scores: Vec<i64>,"));
        assert!(rendered.contains("pub labels: std::collections::HashMap<String, String>,"));
        assert!(rendered.contains("pub weights: std::collections::HashMap<String, i64>,"));
        assert!(!rendered.contains("pub user-id"));
    }
}
