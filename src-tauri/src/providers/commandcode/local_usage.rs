use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashMap;

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

pub(crate) fn parse_jsonl(content: &str) -> Vec<CommandCodeUsageEvent> {
    let mut events = Vec::new();
    let mut indexes = HashMap::new();

    for line in content.lines() {
        let Ok(Value::Object(object)) = serde_json::from_str::<Value>(line) else {
            continue;
        };

        let Some(id) = object.get("id").and_then(Value::as_str) else {
            continue;
        };
        if id.is_empty() {
            continue;
        }

        let Some(timestamp) = object
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc))
        else {
            continue;
        };

        let Some(usage) = object.get("usage").and_then(Value::as_object) else {
            continue;
        };
        let Some(input_tokens) = token_count(usage, "inputTokens") else {
            continue;
        };
        let Some(output_tokens) = token_count(usage, "outputTokens") else {
            continue;
        };
        let Some(cache_read_tokens) = token_count(usage, "cacheReadTokens") else {
            continue;
        };
        let Some(cache_write_tokens) = token_count(usage, "cacheWriteTokens") else {
            continue;
        };

        let event = CommandCodeUsageEvent {
            id: id.to_owned(),
            timestamp,
            model: object.get("model").and_then(Value::as_str).map(str::to_owned),
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
        };

        if let Some(&index) = indexes.get(id) {
            if total_tokens(&event) > total_tokens(&events[index]) {
                events[index] = event;
            }
        } else {
            indexes.insert(id.to_owned(), events.len());
            events.push(event);
        }
    }

    events
}

fn parse_token(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64().or_else(|| parse_float(number.as_f64()?)),
        Value::String(text) => text.parse::<u64>().ok().or_else(|| parse_float(text.parse().ok()?)),
        _ => None,
    }
}

fn token_count(usage: &serde_json::Map<String, Value>, key: &str) -> Option<u64> {
    usage.get(key).map(parse_token).unwrap_or(Some(0))
}

fn parse_float(value: f64) -> Option<u64> {
    if value.is_finite() && value >= 0.0 && value.fract() == 0.0 && value < 18_446_744_073_709_551_616.0 {
        Some(value as u64)
    } else {
        None
    }
}

fn total_tokens(event: &CommandCodeUsageEvent) -> u128 {
    u128::from(event.input_tokens)
        + u128::from(event.output_tokens)
        + u128::from(event.cache_read_tokens)
        + u128::from(event.cache_write_tokens)
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

    #[test]
    fn ignores_malformed_json() {
        assert_eq!(parse_jsonl("not-json\n{}\n").len(), 0);
    }

    #[test]
    fn ignores_events_without_timestamp() {
        let line = r#"{"id":"event-1","usage":{"inputTokens":1}}"#;

        assert_eq!(parse_jsonl(line).len(), 0);
    }

    #[test]
    fn ignores_negative_and_non_finite_token_values() {
        let negative = r#"{"id":"negative","timestamp":"2026-08-09T00:00:00Z","usage":{"inputTokens":-1}}"#;
        let non_finite = r#"{"id":"non-finite","timestamp":"2026-08-09T00:00:00Z","usage":{"inputTokens":"NaN"}}"#;

        assert_eq!(parse_jsonl(negative).len(), 0);
        assert_eq!(parse_jsonl(non_finite).len(), 0);
    }

    #[test]
    fn accepts_numeric_strings_for_token_values() {
        let line = r#"{"id":"event-1","timestamp":"2026-08-09T00:00:00Z","usage":{"inputTokens":"10","outputTokens":"2","cacheReadTokens":"3","cacheWriteTokens":"4"}}"#;

        let events = parse_jsonl(line);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].input_tokens, 10);
        assert_eq!(events[0].output_tokens, 2);
        assert_eq!(events[0].cache_read_tokens, 3);
        assert_eq!(events[0].cache_write_tokens, 4);
    }

    #[test]
    fn deduplicates_by_id_and_keeps_record_with_larger_total() {
        let lines = concat!(
            r#"{"id":"event-1","timestamp":"2026-08-09T00:00:00Z","usage":{"inputTokens":1,"outputTokens":1}}"#,
            "\n",
            r#"{"id":"event-1","timestamp":"2026-08-09T00:00:01Z","usage":{"inputTokens":10,"outputTokens":1}}"#,
        );

        let events = parse_jsonl(lines);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].input_tokens, 10);
    }
}
