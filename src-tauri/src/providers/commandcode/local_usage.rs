use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandCodeUsageEvent {
    pub(crate) id: String,
    pub(crate) timestamp: DateTime<Utc>,
    pub(crate) model: Option<String>,
    pub(crate) input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) cache_read_tokens: u64,
    pub(crate) cache_write_tokens: u64,
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::parse_jsonl;

    #[test]
    fn parses_assistant_usage_and_ignores_non_usage_events() {
        let content = include_str!("fixtures/session.jsonl");

        let events = parse_jsonl(content);

        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.id, "event-1");
        assert_eq!(
            event.timestamp,
            Utc.with_ymd_and_hms(2026, 8, 9, 0, 0, 0).unwrap()
        );
        assert_eq!(event.model.as_deref(), Some("deepseek/deepseek-v4-flash"));
        assert_eq!(event.input_tokens, 100);
        assert_eq!(event.output_tokens, 40);
        assert_eq!(event.cache_read_tokens, 20);
        assert_eq!(event.cache_write_tokens, 0);
    }
}
