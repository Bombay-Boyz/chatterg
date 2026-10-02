use chatterg::domain::Questionnaire;

#[test]
fn questionnaire_loads() {
    let source = std::fs::read_to_string("questions.yaml").expect("questions.yaml should exist");

    let questionnaire: Questionnaire =
        serde_yaml::from_str(&source).expect("questionnaire should be valid YAML");

    assert_eq!(questionnaire.questions.len(), 5);
}
