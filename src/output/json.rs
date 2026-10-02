use crate::domain::Conversation;

pub fn render(conversation: &Conversation) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(conversation)
}
