use chrono::{DateTime, Utc};
use serde::de::{self, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::fmt;
use std::path::PathBuf;
use walkdir::WalkDir;

use crate::{
    models::UsageHistory,
    pricing::{ModelPricing, TokenBreakdown},
    providers::daily_usage::DailyUsageAccumulator,
};

use chrono::{Days, Local, NaiveDate};

const SOURCE_NOTE: &str =
    "From Command Code local session records; missing costs use catalog estimates";

#[derive(Debug, thiserror::Error)]
pub(crate) enum CommandCodeLocalError {
    #[error("could not inspect Command Code local usage directory: {0}")]
    Directory(#[source] std::io::Error),
}

pub(crate) struct CommandCodeUsageScanner {
    root: PathBuf,
}

impl CommandCodeUsageScanner {
    pub(crate) fn new() -> Self {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_default();
        Self {
            root: home.join(".commandcode").join("projects"),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_root(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn has_usage_directory(&self) -> bool {
        self.root.is_dir()
    }

    pub(crate) fn scan(
        &self,
        now: DateTime<Utc>,
        pricing: &ModelPricing,
    ) -> Result<Option<CommandCodeUsageScan>, CommandCodeLocalError> {
        if !self.has_usage_directory() {
            return Ok(None);
        }

        let since = now
            .with_timezone(&Local)
            .date_naive()
            .checked_sub_days(Days::new(30))
            .unwrap_or(NaiveDate::MIN);
        let mut accumulator = DailyUsageAccumulator::default();
        let mut warnings = Vec::new();

        for entry in WalkDir::new(&self.root).follow_links(false).into_iter() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    warnings.push(format!("Could not inspect Command Code usage path: {error}"));
                    continue;
                }
            };
            let path = entry.path();
            if !entry.file_type().is_file()
                || path.extension().and_then(|value| value.to_str()) != Some("jsonl")
                || path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.ends_with(".checkpoints.jsonl"))
            {
                continue;
            }

            let content = match fs::read_to_string(path) {
                Ok(content) => content,
                Err(error) => {
                    warnings.push(format!(
                        "Could not read Command Code usage file {}: {error}",
                        path.display()
                    ));
                    continue;
                }
            };
            for event in parse_jsonl(&content) {
                if event.timestamp > now {
                    continue;
                }
                let date = event.timestamp.with_timezone(&Local).date_naive();
                if date < since {
                    continue;
                }
                let Some(model) = event.model.as_deref().map(str::trim).filter(|m| !m.is_empty())
                else {
                    continue;
                };
                let tokens = TokenBreakdown {
                    input: event.input_tokens,
                    cache_read: event.cache_read_tokens,
                    cache_write_5m: event.cache_write_tokens,
                    output: event.output_tokens,
                    ..TokenBreakdown::default()
                };
                let total = tokens.total_tokens();
                if let Some(cost) = pricing.estimated_cost_dollars(model, tokens, true) {
                    accumulator.add(date, total, cost, model);
                } else if total > 0 {
                    accumulator.add_unknown_model(date, model);
                }
            }
        }

        Ok(Some(CommandCodeUsageScan {
            usage: accumulator.build(now, SOURCE_NOTE),
            warnings,
        }))
    }
}

pub(crate) struct CommandCodeUsageScan {
    pub(crate) usage: UsageHistory,
    pub(crate) warnings: Vec<String>,
}

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
    use std::fs;

    use chrono::{TimeZone, Utc};
    use tempfile::tempdir;

    use crate::pricing::test_bundled_pricing;

    use super::{parse_jsonl, CommandCodeUsageScanner};

    fn write_session(root: &std::path::Path, name: &str, content: &str) {
        let path = root.join("project-a").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn scanner_root(directory: &tempfile::TempDir) -> std::path::PathBuf {
        directory.path().join(".commandcode").join("projects")
    }

    fn event(id: &str, timestamp: &str, model: &str, input: u64, output: u64) -> String {
        format!(
            r#"{{"id":"{id}","timestamp":"{timestamp}","model":"{model}","usage":{{"inputTokens":{input},"outputTokens":{output},"cacheReadTokens":3,"cacheWriteTokens":4}}}}"#
        )
    }

    #[test]
    fn scans_and_aggregates_recent_events_by_local_day() {
        let directory = tempdir().unwrap();
        let root = scanner_root(&directory);
        write_session(
            &root,
            "session.jsonl",
            &format!(
                "{}\n{}\n",
                event("today", "2026-08-09T03:00:00Z", "gpt-5.6-sol", 100, 40),
                event("yesterday", "2026-08-08T03:00:00Z", "gpt-5.6-sol", 10, 5)
            ),
        );

        let now = Utc.with_ymd_and_hms(2026, 8, 9, 12, 0, 0).unwrap();
        let scan = CommandCodeUsageScanner::for_root(root)
            .scan(now, &test_bundled_pricing())
            .unwrap()
            .unwrap();

        assert_eq!(scan.usage.today.as_ref().unwrap().tokens, 147);
        assert_eq!(scan.usage.yesterday.as_ref().unwrap().tokens, 22);
        assert_eq!(scan.usage.last_30_days.as_ref().unwrap().tokens, 169);
        assert!(scan.usage.today.as_ref().unwrap().estimated_cost_usd.unwrap() > 0.0);
        assert!(scan.warnings.is_empty());
    }

    #[test]
    fn filters_events_outside_the_thirty_day_local_window() {
        let directory = tempdir().unwrap();
        let root = scanner_root(&directory);
        write_session(
            &root,
            "session.jsonl",
            &format!(
                "{}\n{}\n{}\n",
                event("boundary", "2026-07-10T03:00:00Z", "gpt-5.6-sol", 1, 1),
                event("old", "2026-07-09T03:00:00Z", "gpt-5.6-sol", 100, 100),
                event("future", "2026-08-10T03:00:00Z", "gpt-5.6-sol", 100, 100)
            ),
        );

        let now = Utc.with_ymd_and_hms(2026, 8, 9, 12, 0, 0).unwrap();
        let scan = CommandCodeUsageScanner::for_root(root)
            .scan(now, &test_bundled_pricing())
            .unwrap()
            .unwrap();

        assert_eq!(scan.usage.last_30_days.as_ref().unwrap().tokens, 9);
    }

    #[test]
    fn tracks_unknown_models_without_inventing_cost() {
        let directory = tempdir().unwrap();
        let root = scanner_root(&directory);
        write_session(
            &root,
            "session.jsonl",
            &event("unknown", "2026-08-09T03:00:00Z", "future-model", 100, 40),
        );

        let now = Utc.with_ymd_and_hms(2026, 8, 9, 12, 0, 0).unwrap();
        let scan = CommandCodeUsageScanner::for_root(root)
            .scan(now, &test_bundled_pricing())
            .unwrap()
            .unwrap();

        assert_eq!(scan.usage.unknown_models, ["future-model"]);
        assert!(scan.usage.today.is_none());
    }

    #[test]
    fn skips_checkpoints_and_non_jsonl_files() {
        let directory = tempdir().unwrap();
        let root = scanner_root(&directory);
        write_session(
            &root,
            "session.jsonl",
            &event("kept", "2026-08-09T03:00:00Z", "gpt-5.6-sol", 1, 1),
        );
        write_session(
            &root,
            "session.checkpoints.jsonl",
            &event("checkpoint", "2026-08-09T03:00:00Z", "gpt-5.6-sol", 100, 100),
        );
        write_session(
            &root,
            "session.txt",
            &event("text", "2026-08-09T03:00:00Z", "gpt-5.6-sol", 100, 100),
        );

        let now = Utc.with_ymd_and_hms(2026, 8, 9, 12, 0, 0).unwrap();
        let scan = CommandCodeUsageScanner::for_root(root)
            .scan(now, &test_bundled_pricing())
            .unwrap()
            .unwrap();

        assert_eq!(scan.usage.today.as_ref().unwrap().tokens, 9);
    }

    #[test]
    fn returns_none_when_projects_directory_is_absent() {
        let directory = tempdir().unwrap();
        let scanner = CommandCodeUsageScanner::for_root(scanner_root(&directory));

        assert!(!scanner.has_usage_directory());
        assert!(scanner
            .scan(Utc::now(), &test_bundled_pricing())
            .unwrap()
            .is_none());
    }

    #[test]
    fn warns_for_file_that_cannot_be_decoded_when_another_file_is_usable() {
        let directory = tempdir().unwrap();
        let root = scanner_root(&directory);
        write_session(
            &root,
            "usable.jsonl",
            &event("usable", "2026-08-09T03:00:00Z", "gpt-5.6-sol", 1, 1),
        );
        let unreadable = root.join("project-a/unreadable.jsonl");
        fs::write(unreadable, [0xff, 0xfe]).unwrap();

        let scan = CommandCodeUsageScanner::for_root(root)
            .scan(
                Utc.with_ymd_and_hms(2026, 8, 9, 12, 0, 0).unwrap(),
                &test_bundled_pricing(),
            )
            .unwrap()
            .unwrap();

        assert_eq!(scan.usage.today.as_ref().unwrap().tokens, 9);
        assert_eq!(scan.warnings.len(), 1);
    }

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
