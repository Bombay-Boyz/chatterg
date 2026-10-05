use super::{
    answer::ValidationStatus,
    question::{AnswerType, Question},
};

pub fn validate(question: &Question, response: &str) -> ValidationStatus {
    let response = response.trim();

    if response.is_empty() {
        return if question.required {
            ValidationStatus::Rejected
        } else {
            ValidationStatus::Accepted
        };
    }

    let lowered = response.to_lowercase();
    if question.reject_if_contains.iter().any(|phrase| lowered.contains(&phrase.to_lowercase())) {
        return ValidationStatus::Rejected;
    }

    match &question.answer_type {
        AnswerType::String | AnswerType::Text => ValidationStatus::Accepted,

        AnswerType::Integer => {
            if response.parse::<i64>().is_ok() {
                ValidationStatus::Accepted
            } else {
                ValidationStatus::Rejected
            }
        }

        AnswerType::List => {
            if response.split(',').map(str::trim).any(|item| !item.is_empty()) {
                ValidationStatus::Accepted
            } else {
                ValidationStatus::Rejected
            }
        }

        AnswerType::Enum => {
            if question.values.iter().any(|value| value.eq_ignore_ascii_case(response)) {
                ValidationStatus::Accepted
            } else {
                ValidationStatus::Rejected
            }
        }
    }
}
