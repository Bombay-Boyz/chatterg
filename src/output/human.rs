use crate::domain::Conversation;

pub fn render(conversation: &Conversation) -> String {
    conversation
        .answers
        .iter()
        .map(|answer| {
            let latest = answer.attempts.last();

            match latest {
                Some(attempt) => {
                    format!(
                        "{}\n  {}\n  {:?}\n",
                        answer.question_id.as_str(),
                        attempt.response,
                        attempt.validation
                    )
                }

                None => format!("{}\n  <no answer>\n", answer.question_id.as_str()),
            }
        })
        .collect()
}
