use chrono::{DateTime, Utc};
use serde::de::{self, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;

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

#[derive(Debug, Deserialize)]
struct RawUsageRecord {
    id: Option<String>,
    timestamp: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    model: Option<String>,
    usage: Option<RawUsage>,
}

#[derive(Debug, Default, Deserialize)]
struct RawUsage {
    #[serde(default, rename = "inputTokens", deserialize_with = "deserialize_token")]
    input_tokens: Option<u64>,
    #[serde(default, rename = "outputTokens", deserialize_with = "deserialize_token")]
    output_tokens: Option<u64>,
    #[serde(default, rename = "cacheReadTokens", deserialize_with = "deserialize_token")]
    cache_read_tokens: Option<u64>,
    #[serde(default, rename = "cacheWriteTokens", deserialize_with = "deserialize_token")]
    cache_write_tokens: Option<u64>,
}

pub(crate) fn parse_jsonl(content: &str) -> Vec<CommandCodeUsageEvent> {
    let mut events = Vec::new();
    let mut indexes = HashMap::new();

    for line in content.lines() {
        let Ok(record) = serde_json::from_str::<RawUsageRecord>(line) else {
            continue;
        };

        let Some(id) = record.id else {
            continue;
        };
        if id.is_empty() {
            continue;
        }

        let Some(timestamp) = record
            .timestamp
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc))
        else {
            continue;
        };

        let Some(usage) = record.usage else {
            continue;
        };

        let event = CommandCodeUsageEvent {
            id: id.clone(),
            timestamp,
            model: record.model,
            input_tokens: usage.input_tokens.unwrap_or(0),
            output_tokens: usage.output_tokens.unwrap_or(0),
            cache_read_tokens: usage.cache_read_tokens.unwrap_or(0),
            cache_write_tokens: usage.cache_write_tokens.unwrap_or(0),
        };

        if let Some(&index) = indexes.get(id) {
            if total_tokens(&event) > total_tokens(&events[index]) {
                events[index] = event;
            }
        } else {
            indexes.insert(id, events.len());
            events.push(event);
        }
    }

    events
}

fn parse_float(value: f64) -> Option<u64> {
    if value.is_finite() && value >= 0.0 && value.fract() == 0.0 && value < 18_446_744_073_709_551_616.0 {
        Some(value as u64)
    } else {
        None
    }
}

fn deserialize_token<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    struct TokenVisitor;

    impl<'de> Visitor<'de> for TokenVisitor {
        type Value = Option<u64>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a non-negative finite token count")
        }

        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Some(value))
        }

        fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            if value >= 0 {
                Ok(Some(value as u64))
            } else {
                Err(E::custom("token count cannot be negative"))
            }
        }

        fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            parse_float(value)
                .map(Some)
                .ok_or_else(|| E::custom("token count must be finite and non-negative"))
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            value
                .parse::<u64>()
                .ok()
                .or_else(|| value.parse::<f64>().ok().and_then(parse_float))
                .map(Some)
                .ok_or_else(|| E::custom("invalid token count"))
        }
    }

    deserializer.deserialize_any(TokenVisitor)
}

fn deserialize_optional_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    struct OptionalStringVisitor;

    impl<'de> Visitor<'de> for OptionalStringVisitor {
        type Value = Option<String>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("an optional string")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Some(value.to_owned()))
        }

        fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Some(value))
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_seq<A>(self, mut access: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            while access.next_element::<IgnoredAny>()?.is_some() {}
            Ok(None)
        }

        fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            while access.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
            Ok(None)
        }
    }

    deserializer.deserialize_any(OptionalStringVisitor)
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
    fn ignores_unknown_content_fields() {
        let line = r#"{"id":"event-1","timestamp":"2026-08-09T00:00:00Z","usage":{"inputTokens":1},"content":{"conversation":["private text"]}}"#;

        let events = parse_jsonl(line);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].input_tokens, 1);
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
