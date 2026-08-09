mod local_usage;

use std::{path::PathBuf, sync::Arc};

use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::{
    models::{
        MetricDefinition, MetricSection, ProviderDefinition, ProviderErrorKind, ProviderLink,
        ProviderSnapshot, UsagePeriodSelection,
    },
    pricing::PricingStore,
};

use self::local_usage::{CommandCodeLocalError, CommandCodeUsageScanner};

use super::{ProviderError, UsageProvider};

const USAGE_SOURCE_NOTE: &str =
    "From Command Code local session records; live hosted usage is available at https://commandcode.ai/studio";

pub(crate) fn definition() -> ProviderDefinition {
    ProviderDefinition {
        id: "commandcode".into(),
        display_name: "Command Code".into(),
        short_name: "CC".into(),
        fallback_enabled: false,
        local_usage_source_note: Some(USAGE_SOURCE_NOTE.into()),
        links: vec![ProviderLink::new(
            "Studio Usage",
            "https://commandcode.ai/studio",
        )],
        metrics: vec![
            MetricDefinition::usage(
                "commandcode.today",
                "Today",
                UsagePeriodSelection::Today,
                MetricSection::AlwaysVisible,
                "T",
            ),
            MetricDefinition::usage(
                "commandcode.yesterday",
                "Yesterday",
                UsagePeriodSelection::Yesterday,
                MetricSection::OnDemand,
                "Y",
            ),
            MetricDefinition::usage(
                "commandcode.last30",
                "Last 30 Days",
                UsagePeriodSelection::Last30Days,
                MetricSection::OnDemand,
                "30",
            ),
            MetricDefinition::trend("commandcode.trend"),
        ],
    }
}

#[derive(Debug, Error)]
enum CommandCodeError {
    #[error("Command Code was not detected. Use Command Code locally first.")]
    NotDetected,
    #[error("Command Code local usage data could not be read: {0}")]
    LocalData(#[source] CommandCodeLocalError),
}

impl From<CommandCodeError> for ProviderError {
    fn from(error: CommandCodeError) -> Self {
        let kind = match error {
            CommandCodeError::NotDetected | CommandCodeError::LocalData(_) => {
                ProviderErrorKind::LocalData
            }
        };
        ProviderError::new(kind, error.to_string())
    }
}

pub struct CommandCodeProvider {
    scanner: CommandCodeUsageScanner,
    pricing: Arc<PricingStore>,
    now: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>,
}

impl CommandCodeProvider {
    pub fn new(pricing: Arc<PricingStore>) -> Self {
        Self {
            scanner: CommandCodeUsageScanner::new(),
            pricing,
            now: Arc::new(Utc::now),
        }
    }

    #[cfg(test)]
    fn with_dependencies(
        projects: PathBuf,
        pricing: Arc<PricingStore>,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            scanner: CommandCodeUsageScanner::for_root(projects),
            pricing,
            now: Arc::new(move || now),
        }
    }

    fn refresh_snapshot(&self) -> Result<ProviderSnapshot, CommandCodeError> {
        let now = (self.now)();
        let scan = self
            .scanner
            .scan(now, &self.pricing.current())
            .map_err(CommandCodeError::LocalData)?
            .ok_or(CommandCodeError::NotDetected)?;

        Ok(ProviderSnapshot {
            provider_id: "commandcode".into(),
            plan: None,
            quotas: Vec::new(),
            value_metrics: Vec::new(),
            status_metrics: Vec::new(),
            notices: Vec::new(),
            usage: scan.usage,
            warnings: scan.warnings,
            refreshed_at: now,
        })
    }
}

impl UsageProvider for CommandCodeProvider {
    fn definition(&self) -> ProviderDefinition {
        definition()
    }

    fn has_local_credentials(&self) -> bool {
        self.scanner.has_usage_directory()
    }

    fn refresh(&self) -> Result<ProviderSnapshot, ProviderError> {
        self.refresh_snapshot().map_err(ProviderError::from)
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Arc};

    use chrono::{TimeZone, Utc};
    use tempfile::tempdir;

    use crate::{
        pricing::PricingStore,
        providers::UsageProvider,
    };

    use super::{definition, CommandCodeProvider};

    #[test]
    fn definition_exposes_command_code_identity_and_usage_metrics() {
        let definition = definition();

        assert_eq!(definition.id, "commandcode");
        assert_eq!(definition.display_name, "Command Code");
        assert_eq!(definition.short_name, "CC");
        assert_eq!(
            definition.links,
            [crate::models::ProviderLink::new(
                "Studio Usage",
                "https://commandcode.ai/studio"
            )]
        );
        assert!(definition
            .metrics
            .iter()
            .any(|metric| metric.id == "commandcode.today"));
        assert!(definition
            .metrics
            .iter()
            .any(|metric| metric.id == "commandcode.yesterday"));
        assert!(definition
            .metrics
            .iter()
            .any(|metric| metric.id == "commandcode.last30"));
        assert!(definition
            .metrics
            .iter()
            .any(|metric| metric.id == "commandcode.trend"));
    }

    #[test]
    fn provider_does_not_expose_an_api_key_capability() {
        let directory = tempdir().unwrap();
        let pricing = Arc::new(
            PricingStore::new_without_refresh_for_test(directory.path().join("pricing")).unwrap(),
        );
        let provider = CommandCodeProvider::with_dependencies(
            directory.path().join("projects"),
            pricing,
            Utc::now(),
        );

        assert_eq!(provider.api_key_status(), None);
    }

    #[test]
    fn local_credentials_probe_only_checks_the_projects_directory() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("auth.json"), "{\"token\":\"secret\"}").unwrap();
        let projects = directory.path().join("projects");
        let pricing = Arc::new(
            PricingStore::new_without_refresh_for_test(directory.path().join("pricing")).unwrap(),
        );
        let provider = CommandCodeProvider::with_dependencies(
            projects.clone(),
            pricing,
            Utc::now(),
        );

        assert!(!provider.has_local_credentials());
        fs::create_dir_all(projects).unwrap();
        assert!(provider.has_local_credentials());
    }

    #[test]
    fn refresh_returns_local_usage_without_quota_windows() {
        let directory = tempdir().unwrap();
        let projects = directory.path().join("projects").join("project-a");
        fs::create_dir_all(&projects).unwrap();
        fs::write(
            projects.join("session.jsonl"),
            r#"{"id":"event-1","timestamp":"2026-08-09T03:00:00Z","model":"deepseek/deepseek-v4-flash","usage":{"inputTokens":100,"outputTokens":40}}"#,
        )
        .unwrap();
        let pricing = Arc::new(
            PricingStore::new_without_refresh_for_test(directory.path().join("pricing")).unwrap(),
        );
        let now = Utc.with_ymd_and_hms(2026, 8, 9, 12, 0, 0).unwrap();
        let provider = CommandCodeProvider::with_dependencies(
            directory.path().join("projects"),
            pricing,
            now,
        );

        let snapshot = provider.refresh().unwrap();

        assert_eq!(snapshot.provider_id, "commandcode");
        assert!(snapshot.usage.today.is_some());
        assert!(snapshot.usage.yesterday.is_none());
        assert!(snapshot.usage.last_30_days.is_some());
        assert!(snapshot.quotas.is_empty());
    }
}
