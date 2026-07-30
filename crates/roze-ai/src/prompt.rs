use std::collections::BTreeMap;

use crate::AiError;

/// A small deterministic `{{variable}}` prompt template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptTemplate {
    source: String,
}

impl PromptTemplate {
    pub fn new(source: impl Into<String>) -> Result<Self, AiError> {
        let source = source.into();
        validate_template(&source)?;
        Ok(Self { source })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn render(&self, variables: &BTreeMap<String, String>) -> Result<String, AiError> {
        let mut output = String::with_capacity(self.source.len());
        let mut rest = self.source.as_str();
        while let Some(start) = rest.find("{{") {
            output.push_str(&rest[..start]);
            let after_start = &rest[start + 2..];
            let end = after_start
                .find("}}")
                .ok_or_else(|| AiError::Prompt("unclosed template variable".to_string()))?;
            let name = after_start[..end].trim();
            let value = variables.get(name).ok_or_else(|| {
                AiError::Prompt(format!("missing prompt template variable `{name}`"))
            })?;
            output.push_str(value);
            rest = &after_start[end + 2..];
        }
        output.push_str(rest);
        Ok(output)
    }
}

fn validate_template(source: &str) -> Result<(), AiError> {
    let mut rest = source;
    loop {
        let opening = rest.find("{{");
        let closing = rest.find("}}");
        if matches!((opening, closing), (None, Some(_)))
            || matches!((opening, closing), (Some(opening), Some(closing)) if closing < opening)
        {
            return Err(AiError::Prompt(
                "template contains an unmatched closing delimiter".to_string(),
            ));
        }
        let Some(start) = opening else {
            break;
        };
        let after_start = &rest[start + 2..];
        let end = after_start
            .find("}}")
            .ok_or_else(|| AiError::Prompt("unclosed template variable".to_string()))?;
        let name = after_start[..end].trim();
        if name.is_empty()
            || !name
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            return Err(AiError::Prompt(format!(
                "invalid prompt template variable `{name}`"
            )));
        }
        rest = &after_start[end + 2..];
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_named_variables_and_rejects_missing_values() {
        let template = PromptTemplate::new("Question: {{ question }}\n{{context}}").expect("valid");
        let values = BTreeMap::from([
            ("question".to_string(), "What is Roze?".to_string()),
            ("context".to_string(), "A Rust framework.".to_string()),
        ]);
        assert_eq!(
            template.render(&values).expect("render"),
            "Question: What is Roze?\nA Rust framework."
        );
        assert!(template.render(&BTreeMap::new()).is_err());
        assert!(PromptTemplate::new("bad }} {{value}}").is_err());
    }
}
