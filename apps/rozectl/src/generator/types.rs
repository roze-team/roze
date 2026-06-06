use crate::{
    generator::to_snake_case,
    parser::{Field, FieldSource, TypeDef},
};

pub fn render_types(types: &[TypeDef]) -> String {
    let mut out = String::from("#![allow(dead_code)]\n\nuse serde::{Deserialize, Serialize};\n\n");

    for ty in types {
        out.push_str("#[derive(Debug, Clone, Serialize, Deserialize)]\n");
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

fn map_type(ty: &str) -> &str {
    match ty {
        "string" => "String",
        "int" => "i64",
        "uint" => "u64",
        "bool" => "bool",
        other => other,
    }
}

fn rust_field_name(field: &Field) -> String {
    field
        .json_name
        .clone()
        .unwrap_or_else(|| to_snake_case(&field.name))
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
