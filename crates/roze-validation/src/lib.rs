use std::fmt::{self, Display};

pub use validator::{Validate, ValidationError, ValidationErrors, ValidationErrorsKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    pub field: String,
    pub code: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ValidationReport {
    issues: Vec<ValidationIssue>,
}

impl ValidationReport {
    pub fn new() -> Self {
        Self { issues: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.issues.is_empty()
    }

    pub fn len(&self) -> usize {
        self.issues.len()
    }

    pub fn issues(&self) -> &[ValidationIssue] {
        &self.issues
    }

    pub fn push(&mut self, issue: ValidationIssue) {
        self.issues.push(issue);
    }

    pub fn from_errors(errors: &ValidationErrors) -> Self {
        let mut report = Self::new();
        flatten_errors("", errors, &mut report.issues);
        report.issues.sort_by(|left, right| {
            left.field
                .cmp(&right.field)
                .then(left.code.cmp(&right.code))
                .then(left.message.cmp(&right.message))
        });
        report
    }

    pub fn messages(&self) -> Vec<String> {
        self.issues
            .iter()
            .map(|issue| match &issue.message {
                Some(message) => format!("{}: {}", issue.field, message),
                None => format!("{}: {}", issue.field, issue.code),
            })
            .collect()
    }
}

impl Display for ValidationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for message in self.messages() {
            if !first {
                write!(f, "; ")?;
            }
            first = false;
            write!(f, "{message}")?;
        }
        Ok(())
    }
}

pub fn validate<T: Validate>(value: &T) -> Result<(), ValidationErrors> {
    value.validate()
}

pub fn validate_report<T: Validate>(value: &T) -> Result<(), ValidationReport> {
    value.validate().map_err(|errors| ValidationReport::from_errors(&errors))
}

pub fn validation_message(errors: &ValidationErrors) -> String {
    ValidationReport::from_errors(errors).to_string()
}

pub fn validate_or<T, F, E>(value: &T, map_err: F) -> Result<(), E>
where
    T: Validate,
    F: FnOnce(ValidationReport) -> E,
{
    value
        .validate()
        .map_err(|errors| map_err(ValidationReport::from_errors(&errors)))
}

pub fn validate_or_message<T: Validate>(value: &T) -> Result<(), String> {
    value
        .validate()
        .map_err(|errors| ValidationReport::from_errors(&errors).to_string())
}

fn flatten_errors(prefix: &str, errors: &ValidationErrors, out: &mut Vec<ValidationIssue>) {
    for (field, kind) in errors.errors() {
        let path = join_path(prefix, field);
        match kind {
            ValidationErrorsKind::Field(field_errors) => {
                for error in field_errors {
                    out.push(ValidationIssue {
                        field: path.clone(),
                        code: error.code.to_string(),
                        message: error.message.as_ref().map(ToString::to_string),
                    });
                }
            }
            ValidationErrorsKind::Struct(child) => {
                flatten_errors(&path, child, out);
            }
            ValidationErrorsKind::List(children) => {
                for (index, child) in children {
                    let nested = format!("{path}[{index}]");
                    flatten_errors(&nested, child, out);
                }
            }
        }
    }
}

fn join_path(prefix: &str, field: &str) -> String {
    if prefix.is_empty() {
        field.to_string()
    } else {
        format!("{prefix}.{field}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field_error(code: &'static str, message: Option<&'static str>) -> ValidationError {
        let mut error = ValidationError::new(code);
        if let Some(message) = message {
            error = error.with_message(message.into());
        }
        error
    }

    #[test]
    fn flattens_simple_field_errors() {
        let mut errors = ValidationErrors::new();
        errors.add("username", field_error("length", Some("too short")));
        errors.add("password", field_error("required", None));

        let report = ValidationReport::from_errors(&errors);

        assert_eq!(report.len(), 2);
        assert_eq!(
            report.messages(),
            vec!["password: required".to_string(), "username: too short".to_string()]
        );
    }

    #[test]
    fn flattens_nested_struct_errors() {
        let mut child = ValidationErrors::new();
        child.add("name", field_error("length", Some("too short")));
        let nested = ValidationErrorsKind::Struct(Box::new(child));

        let mut errors = ValidationErrors::new();
        errors.errors_mut().insert("profile".into(), nested);

        let report = ValidationReport::from_errors(&errors);

        assert_eq!(report.messages(), vec!["profile.name: too short".to_string()]);
        assert_eq!(validation_message(&errors), "profile.name: too short");
    }

    #[test]
    fn validate_or_message_returns_flat_text() {
        #[derive(Debug, Validate)]
        struct Input {
            #[validate(length(min = 3))]
            name: String,
        }

        let input = Input {
            name: String::new(),
        };

        let err = validate_or_message(&input).expect_err("validation should fail");
        assert!(err.contains("name"));
    }
}
