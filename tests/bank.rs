//! The question bank is edited by `chatterg add`, `list` and `remove`. What these
//! commands write must be read back by the run exactly as intended.

use chatterg::{
    bank::{self, BankError},
    domain::Questionnaire,
};

fn add(text: &str, question: &str, section: Option<&str>) -> (String, usize) {
    let (text, added) = bank::add(text, question, section).unwrap();
    (text, added.number)
}

fn load(text: &str) -> Questionnaire {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("q.txt");
    std::fs::write(&path, text).unwrap();
    Questionnaire::from_path(&path).unwrap()
}

fn sections(questionnaire: &Questionnaire) -> Vec<(Option<&str>, &str)> {
    questionnaire.questions.iter().map(|q| (q.section.as_deref(), q.question.as_str())).collect()
}

// ---- add --------------------------------------------------------------------------

#[test]
fn the_first_question_creates_the_text() {
    let (text, number) = add("", "What is a zeolite?", None);

    assert_eq!(text, "What is a zeolite?\n");
    assert_eq!(number, 1);
}

#[test]
fn a_question_goes_to_the_end_and_everything_else_is_untouched() {
    let original = "# my bank\n\nOne?\n\n# a note\nTwo?\n";

    let (text, number) = add(original, "Three?", None);

    assert_eq!(text, "# my bank\n\nOne?\n\n# a note\nTwo?\nThree?\n");
    assert_eq!(number, 3);
}

#[test]
fn a_missing_final_newline_is_handled() {
    let (text, _) = add("One?", "Two?", None);

    assert_eq!(text, "One?\nTwo?\n");
}

#[test]
fn a_question_with_a_section_joins_the_end_of_that_section() {
    let original = "## Basics\nOne?\nTwo?\n\n## More\nThree?\n";

    let (text, number) = add(original, "Basics three?", Some("Basics"));

    assert_eq!(text, "## Basics\nOne?\nTwo?\nBasics three?\n\n## More\nThree?\n");
    assert_eq!(number, 3, "the number counts questions in file order");

    let questionnaire = load(&text);
    assert_eq!(
        sections(&questionnaire),
        [
            (Some("Basics"), "One?"),
            (Some("Basics"), "Two?"),
            (Some("Basics"), "Basics three?"),
            (Some("More"), "Three?")
        ]
    );
}

#[test]
fn a_section_that_has_a_heading_but_no_questions_gets_the_question_right_below_it() {
    let (text, number) = add("## Empty\n\n## Other\nOne?\n", "New?", Some("Empty"));

    assert_eq!(text, "## Empty\nNew?\n\n## Other\nOne?\n");
    assert_eq!(number, 1);
}

#[test]
fn section_names_match_without_regard_to_case_or_spaces() {
    let (text, _) = add("## Basics\nOne?\n", "Two?", Some("  basics "));

    assert_eq!(text, "## Basics\nOne?\nTwo?\n");
}

#[test]
fn a_new_section_is_added_at_the_end_with_a_heading() {
    let (text, number) = add("One?\nTwo?\n", "Three?", Some("Nxtbrane"));

    assert_eq!(text, "One?\nTwo?\n\n## Nxtbrane\nThree?\n");
    assert_eq!(number, 3);
}

#[test]
fn a_question_without_a_section_after_a_section_does_not_slip_into_it() {
    let (text, number) = add("## Basics\nOne?\n", "Loose?", None);

    assert_eq!(text, "## Basics\nOne?\n\n##\nLoose?\n");
    assert_eq!(number, 2);

    let questionnaire = load(&text);
    assert_eq!(sections(&questionnaire), [(Some("Basics"), "One?"), (None, "Loose?")]);
}

#[test]
fn adding_to_an_unsectioned_tail_after_a_closed_section_needs_no_extra_heading() {
    let (text, _) = add("## Basics\nOne?\n\n##\nLoose?\n", "Another loose?", None);

    assert_eq!(text, "## Basics\nOne?\n\n##\nLoose?\nAnother loose?\n");
}

#[test]
fn a_duplicate_is_refused_whatever_its_case() {
    let error = bank::add("One?\nTwo?\n", "  two?  ", None).unwrap_err();

    assert!(matches!(&error, BankError::Duplicate { number: 2, text } if text == "Two?"));
    assert!(error.to_string().contains("already in the bank as number 2"));
}

#[test]
fn text_that_would_be_read_differently_is_refused() {
    for tricky in
        ["# a comment", "## heading", "- item", "* item", "1. item", "\"quoted\"", "'quoted'"]
    {
        assert!(
            matches!(bank::add("", tricky, None), Err(BankError::ReadDifferently)),
            "{tricky:?} should be refused"
        );
    }

    // numbers and dashes that are part of the sentence are fine
    for fine in ["1,000 hours?", "What is H-2 gas?", "Why 3.5 nm pores?"] {
        let (text, _) = add("", fine, None);
        assert_eq!(load(&text).questions[0].question, fine);
    }
}

#[test]
fn empty_and_multi_line_questions_are_refused() {
    assert!(matches!(bank::add("", "   ", None), Err(BankError::Empty)));
    assert!(matches!(bank::add("", "one\ntwo", None), Err(BankError::MultiLine)));
    assert!(matches!(bank::add("", "one\rtwo", None), Err(BankError::MultiLine)));
}

#[test]
fn bad_section_names_are_refused() {
    for bad in ["", "   ", "# x", "two\nlines"] {
        assert!(matches!(bank::add("", "Q?", Some(bad)), Err(BankError::BadSection)), "{bad:?}");
    }
}

// ---- remove -------------------------------------------------------------------------

const BANK: &str = "# notes\nOne?\n\n## Basics\nTwo?\nThree?\n\n##\nFour?\n";

#[test]
fn a_question_is_removed_by_number_and_everything_else_stays() {
    let (text, removed) = bank::remove(BANK, 3).unwrap();

    assert_eq!(removed.question, "Three?");
    assert_eq!(text, "# notes\nOne?\n\n## Basics\nTwo?\n\n##\nFour?\n");
}

#[test]
fn the_first_and_last_questions_can_be_removed() {
    assert_eq!(bank::remove(BANK, 1).unwrap().1.question, "One?");
    assert_eq!(bank::remove(BANK, 4).unwrap().1.question, "Four?");
    assert_eq!(bank::questions_in(&bank::remove(BANK, 1).unwrap().0), ["Two?", "Three?", "Four?"]);
}

#[test]
fn removing_a_number_that_does_not_exist_changes_nothing() {
    for number in [0, 5, 99] {
        assert!(
            matches!(bank::remove(BANK, number), Err(BankError::NoSuchQuestion { count: 4, .. })),
            "{number}"
        );
    }
}

#[test]
fn numbers_match_what_list_shows_and_what_the_run_calls_them() {
    let questionnaire = load(BANK);
    let listing = bank::render_list(&questionnaire, "questions.txt");

    assert_eq!(
        listing,
        "questions.txt: 4 questions\n\n 1  One?\n\n## Basics\n 2  Two?\n 3  Three?\n\n(no section)\n 4  Four?\n"
    );
    assert_eq!(questionnaire.questions[2].id.as_str(), "q003");
}

#[test]
fn the_listing_of_one_question_is_singular() {
    let listing = bank::render_list(&load("Only?\n"), "q.txt");

    assert!(listing.starts_with("q.txt: 1 question\n"));
}

// ---- files --------------------------------------------------------------------------

#[test]
fn adding_to_a_missing_file_creates_it_and_its_folder() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("bank").join("questions.txt");

    let added = bank::add_to_file(&file, "What is a zeolite?", None).unwrap();

    assert_eq!(added.number, 1);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "What is a zeolite?\n");
    assert!(!bank::backup_of(&file).exists(), "a new file has no previous version");
}

#[test]
fn every_edit_keeps_the_previous_version_and_leaves_no_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("questions.txt");
    std::fs::write(&file, "One?\nTwo?\n").unwrap();

    bank::add_to_file(&file, "Three?", None).unwrap();

    assert_eq!(std::fs::read_to_string(bank::backup_of(&file)).unwrap(), "One?\nTwo?\n");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "One?\nTwo?\nThree?\n");

    bank::remove_from_file(&file, 1).unwrap();

    assert_eq!(std::fs::read_to_string(bank::backup_of(&file)).unwrap(), "One?\nTwo?\nThree?\n");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "Two?\nThree?\n");

    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(!leftovers.iter().any(|name| name.ends_with(".tmp")), "{leftovers:?}");
}

#[test]
fn a_refused_edit_leaves_the_file_exactly_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("questions.txt");
    std::fs::write(&file, "One?\n").unwrap();

    assert!(bank::add_to_file(&file, "one?", None).is_err());
    assert!(bank::remove_from_file(&file, 7).is_err());

    assert_eq!(std::fs::read_to_string(&file).unwrap(), "One?\n");
    assert!(!bank::backup_of(&file).exists(), "no edit happened, so no backup either");
}

#[test]
fn yaml_banks_are_never_edited() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("questions.yaml");
    std::fs::write(&file, "questions: [\"One?\"]\n").unwrap();

    assert!(matches!(bank::add_to_file(&file, "Two?", None), Err(BankError::NotPlainText(_))));
    assert!(matches!(bank::remove_from_file(&file, 1), Err(BankError::NotPlainText(_))));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "questions: [\"One?\"]\n");
}

#[test]
fn an_unreadable_bank_is_reported_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    // a folder where the file should be
    let not_a_file = dir.path().join("questions.txt");
    std::fs::create_dir(&not_a_file).unwrap();

    assert!(matches!(bank::add_to_file(&not_a_file, "One?", None), Err(BankError::Read { .. })));
}

#[test]
fn what_is_added_is_exactly_what_the_run_will_ask() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("questions.txt");

    bank::add_to_file(&file, "What is a zeolite?", None).unwrap();
    bank::add_to_file(&file, "What is Nxtbrane?", Some("Nxtbrane")).unwrap();
    bank::add_to_file(&file, "How is it made?", Some("Nxtbrane")).unwrap();
    bank::add_to_file(&file, "What is a membrane?", None).unwrap();
    bank::remove_from_file(&file, 1).unwrap();

    let questionnaire = Questionnaire::from_path(&file).unwrap();

    assert_eq!(
        sections(&questionnaire),
        [
            (Some("Nxtbrane"), "What is Nxtbrane?"),
            (Some("Nxtbrane"), "How is it made?"),
            (None, "What is a membrane?")
        ]
    );
}
