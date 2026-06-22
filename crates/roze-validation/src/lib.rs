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

    pub fn messages_i18n(&self, locale: Option<&str>) -> Vec<String> {
        self.issues
            .iter()
            .map(|issue| match &issue.message {
                Some(message) => format!("{}: {}", issue.field, message),
                None => format!(
                    "{}: {}",
                    issue.field,
                    validation_message_i18n(&issue.code, locale)
                ),
            })
            .collect()
    }

    pub fn to_string_i18n(&self, locale: Option<&str>) -> String {
        self.messages_i18n(locale).join("; ")
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
    value
        .validate()
        .map_err(|errors| ValidationReport::from_errors(&errors))
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

pub fn validate_or_message_i18n<T: Validate>(
    value: &T,
    locale: Option<&str>,
) -> Result<(), String> {
    value
        .validate()
        .map_err(|errors| ValidationReport::from_errors(&errors).to_string_i18n(locale))
}

pub fn validation_message_i18n(code: &str, locale: Option<&str>) -> &'static str {
    match normalize_locale(locale).as_deref() {
        Some("zh-CN") => match code {
            "required" => "不能为空",
            "length" => "长度不合法",
            "range" => "数值范围不合法",
            "oneof" => "必须是允许的取值",
            "email" => "邮箱格式不合法",
            "url" => "URL 格式不合法",
            "uri" => "URI 格式不合法",
            "ip" => "IP 地址格式不合法",
            "ipv4" => "IPv4 地址格式不合法",
            "ipv6" => "IPv6 地址格式不合法",
            "contains" => "缺少必需内容",
            "does_not_contain" => "包含禁止内容",
            "startswith" => "前缀不合法",
            "endswith" => "后缀不合法",
            "alpha" => "只能包含字母",
            "alphanum" => "只能包含字母和数字",
            "ascii" => "只能包含 ASCII 字符",
            "numeric" => "必须是数字",
            "lowercase" => "必须是小写",
            "uppercase" => "必须是大写",
            "eqfield" => "必须与指定字段相等",
            "nefield" => "不能与指定字段相等",
            "gtfield" => "必须大于指定字段",
            "gtefield" => "必须大于或等于指定字段",
            "ltfield" => "必须小于指定字段",
            "ltefield" => "必须小于或等于指定字段",
            "required_if" => "条件满足时不能为空",
            "required_unless" => "条件不满足时不能为空",
            "required_with" => "关联字段存在时不能为空",
            "required_without" => "关联字段不存在时不能为空",
            "dive" => "集合元素不合法",
            "keys" => "集合键不合法",
            "endkeys" => "集合值不合法",
            _ => "参数不合法",
        },
        _ => match code {
            "required" => "required",
            "length" => "invalid length",
            "range" => "out of range",
            "oneof" => "not an allowed value",
            "email" => "invalid email",
            "url" => "invalid url",
            "uri" => "invalid uri",
            "ip" => "invalid ip address",
            "ipv4" => "invalid ipv4 address",
            "ipv6" => "invalid ipv6 address",
            "contains" => "missing required content",
            "does_not_contain" => "contains forbidden content",
            "startswith" => "invalid prefix",
            "endswith" => "invalid suffix",
            "alpha" => "letters only",
            "alphanum" => "letters and numbers only",
            "ascii" => "ascii only",
            "numeric" => "numeric value required",
            "lowercase" => "lowercase required",
            "uppercase" => "uppercase required",
            "eqfield" => "must equal another field",
            "nefield" => "must not equal another field",
            "gtfield" => "must be greater than another field",
            "gtefield" => "must be greater than or equal to another field",
            "ltfield" => "must be less than another field",
            "ltefield" => "must be less than or equal to another field",
            "required_if" => "required by condition",
            "required_unless" => "required unless condition matches",
            "required_with" => "required with another field",
            "required_without" => "required without another field",
            "dive" => "invalid collection element",
            "keys" => "invalid collection key",
            "endkeys" => "invalid collection value",
            _ => "invalid value",
        },
    }
}

pub fn is_alpha(value: &str) -> bool {
    !value.is_empty() && value.chars().all(char::is_alphabetic)
}

pub fn is_alphanum(value: &str) -> bool {
    !value.is_empty() && value.chars().all(char::is_alphanumeric)
}

pub fn is_ascii(value: &str) -> bool {
    value.is_ascii()
}

pub fn is_numeric(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|ch| ch.is_ascii_digit())
}

pub fn is_lowercase(value: &str) -> bool {
    !value.is_empty() && !value.chars().any(char::is_uppercase)
}

pub fn is_uppercase(value: &str) -> bool {
    !value.is_empty() && !value.chars().any(char::is_lowercase)
}

pub fn one_of(value: &str, allowed: &[&str]) -> bool {
    allowed.contains(&value)
}

pub fn starts_with(value: &str, prefix: &str) -> bool {
    value.starts_with(prefix)
}

pub fn ends_with(value: &str, suffix: &str) -> bool {
    value.ends_with(suffix)
}

fn normalize_locale(locale: Option<&str>) -> Option<String> {
    let normalized = locale?.trim().replace('_', "-");
    if normalized.is_empty() || normalized == "*" {
        return None;
    }
    let lower = normalized.to_ascii_lowercase();
    if lower == "zh" || lower.starts_with("zh-cn") || lower.starts_with("zh-hans") {
        return Some("zh-CN".to_string());
    }
    if lower == "en" || lower.starts_with("en-us") || lower.starts_with("en-") {
        return Some("en-US".to_string());
    }
    None
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
            vec![
                "password: required".to_string(),
                "username: too short".to_string()
            ]
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

        assert_eq!(
            report.messages(),
            vec!["profile.name: too short".to_string()]
        );
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

    #[test]
    fn validate_or_message_i18n_returns_localized_text() {
        #[derive(Debug, Validate)]
        struct Input {
            #[validate(length(min = 3))]
            name: String,
        }

        let input = Input {
            name: String::new(),
        };

        let err =
            validate_or_message_i18n(&input, Some("zh-CN")).expect_err("validation should fail");
        assert_eq!(err, "name: 长度不合法");
    }

    #[test]
    fn validation_message_i18n_covers_generated_validator_codes() {
        assert_eq!(
            validation_message_i18n("required_with", Some("zh-CN")),
            "关联字段存在时不能为空"
        );
        assert_eq!(
            validation_message_i18n("lowercase", Some("zh-CN")),
            "必须是小写"
        );
        assert_eq!(
            validation_message_i18n("uppercase", Some("en-US")),
            "uppercase required"
        );
        assert_eq!(
            validation_message_i18n("oneof", None),
            "not an allowed value"
        );
    }

    #[test]
    fn helper_validators_cover_common_string_rules() {
        assert!(is_alpha("用户"));
        assert!(is_alphanum("user123"));
        assert!(is_ascii("trace-123"));
        assert!(is_numeric("123456"));
        assert!(is_lowercase("user_123"));
        assert!(is_uppercase("USER_123"));
        assert!(one_of("active", &["active", "disabled"]));
        assert!(starts_with("user_123", "user_"));
        assert!(ends_with("order_id", "_id"));

        assert!(!is_alpha("user123"));
        assert!(!is_alphanum("user-123"));
        assert!(!is_numeric("12.3"));
        assert!(!is_lowercase("User"));
        assert!(!is_uppercase("User"));
    }
}
