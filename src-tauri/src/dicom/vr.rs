use chrono::NaiveDate;
use regex::Regex;

use super::model::ValidationResult;

pub fn validate(vr: &str, value: &str) -> ValidationResult {
    if vr == "SQ" {
        return ValidationResult::invalid("SQ values cannot be edited directly");
    }

    if value.is_empty() {
        return ValidationResult::valid();
    }

    if is_multi_value_vr(vr) {
        return validate_multi(vr, value);
    }

    match vr {
        "DA" => validate_da(value),
        "TM" => validate_tm(value),
        "UI" => validate_ui(value),
        "IS" => validate_is(value),
        "DS" => validate_ds(value),
        "CS" => validate_cs(value),
        "LO" => validate_text(value, 64, "LO"),
        "SH" => validate_text(value, 16, "SH"),
        "PN" => validate_pn(value),
        _ => ValidationResult::valid(),
    }
}

fn validate_multi(vr: &str, value: &str) -> ValidationResult {
    for (index, component) in value.split('\\').enumerate() {
        let result = validate_single(vr, component);
        if !result.valid {
            let detail = result.message.unwrap_or_else(|| "invalid value".to_string());
            return ValidationResult::invalid(format!("value #{}: {detail}", index + 1));
        }
    }

    ValidationResult::valid()
}

fn validate_single(vr: &str, value: &str) -> ValidationResult {
    match vr {
        "DA" => validate_da(value),
        "TM" => validate_tm(value),
        "UI" => validate_ui(value),
        "IS" => validate_is(value),
        "DS" => validate_ds(value),
        "CS" => validate_cs(value),
        "LO" => validate_text(value, 64, "LO"),
        "SH" => validate_text(value, 16, "SH"),
        "PN" => validate_pn(value),
        _ => ValidationResult::valid(),
    }
}

fn is_multi_value_vr(vr: &str) -> bool {
    matches!(vr, "DA" | "TM" | "UI" | "IS" | "DS" | "CS" | "LO" | "SH" | "PN")
}

fn validate_da(value: &str) -> ValidationResult {
    if value.len() != 8 || !value.chars().all(|c| c.is_ascii_digit()) {
        return ValidationResult::invalid("DA must be YYYYMMDD");
    }

    match NaiveDate::parse_from_str(value, "%Y%m%d") {
        Ok(_) => ValidationResult::valid(),
        Err(_) => ValidationResult::invalid("DA must be an existing date"),
    }
}

fn validate_tm(value: &str) -> ValidationResult {
    let re = Regex::new(r"^\d{6}(\.\d{1,6})?$").expect("valid TM regex");
    if !re.is_match(value) {
        return ValidationResult::invalid("TM must be HHMMSS or HHMMSS.FFFFFF");
    }

    let hour = value[0..2].parse::<u32>().unwrap_or(24);
    let minute = value[2..4].parse::<u32>().unwrap_or(60);
    let second = value[4..6].parse::<u32>().unwrap_or(60);

    if hour <= 23 && minute <= 59 && second <= 59 {
        ValidationResult::valid()
    } else {
        ValidationResult::invalid("TM time components are out of range")
    }
}

fn validate_ui(value: &str) -> ValidationResult {
    if value.len() > 64 {
        return ValidationResult::invalid("UI must be 64 characters or fewer");
    }
    if value.starts_with('.') || value.ends_with('.') {
        return ValidationResult::invalid("UI cannot start or end with a dot");
    }
    if value.chars().all(|c| c.is_ascii_digit() || c == '.') {
        ValidationResult::valid()
    } else {
        ValidationResult::invalid("UI may contain only digits and dots")
    }
}

fn validate_is(value: &str) -> ValidationResult {
    if value.len() > 12 {
        return ValidationResult::invalid("IS must be 12 characters or fewer");
    }
    let re = Regex::new(r"^[+-]?\d+$").expect("valid IS regex");
    if re.is_match(value) {
        ValidationResult::valid()
    } else {
        ValidationResult::invalid("IS must be a signed integer string")
    }
}

fn validate_ds(value: &str) -> ValidationResult {
    if value.len() > 16 {
        return ValidationResult::invalid("DS must be 16 characters or fewer");
    }
    let re = Regex::new(r"^[+-]?(\d+(\.\d*)?|\.\d+)([eE][+-]?\d+)?$").expect("valid DS regex");
    if re.is_match(value) {
        ValidationResult::valid()
    } else {
        ValidationResult::invalid("DS must be a decimal string")
    }
}

fn validate_cs(value: &str) -> ValidationResult {
    if value.len() > 16 {
        return ValidationResult::invalid("CS must be 16 characters or fewer");
    }
    if value
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == ' ' || c == '_')
    {
        ValidationResult::valid()
    } else {
        ValidationResult::invalid("CS may contain only A-Z, digits, spaces, and underscores")
    }
}

fn validate_text(value: &str, max_len: usize, vr: &str) -> ValidationResult {
    if value.len() > max_len {
        return ValidationResult::invalid(format!("{vr} must be {max_len} characters or fewer"));
    }
    if value.chars().any(|c| c.is_control()) {
        return ValidationResult::invalid(format!("{vr} cannot contain control characters"));
    }
    ValidationResult::valid()
}

fn validate_pn(value: &str) -> ValidationResult {
    if value.chars().any(|c| c.is_control()) {
        return ValidationResult::invalid("PN cannot contain control characters");
    }
    if value.split('=').all(|group| group.split('^').count() <= 5) {
        ValidationResult::valid()
    } else {
        ValidationResult::invalid("PN name groups may contain at most five caret-separated components")
    }
}

#[cfg(test)]
mod tests {
    use super::validate;

    #[test]
    fn validates_multi_value_ds() {
        assert!(validate("DS", "0\\0\\0").valid);
        assert!(validate("DS", "-1.25\\2.5e3\\.75").valid);
    }

    #[test]
    fn reports_invalid_multi_value_component() {
        let result = validate("DS", "0\\abc\\1");

        assert!(!result.valid);
        assert_eq!(
            result.message.as_deref(),
            Some("value #2: DS must be a decimal string")
        );
    }

    #[test]
    fn allows_empty_values() {
        assert!(validate("DS", "").valid);
        assert!(validate("DA", "").valid);
    }
}
