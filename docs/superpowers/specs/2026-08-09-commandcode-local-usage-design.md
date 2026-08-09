# Command Code Local Usage Provider

## Goal

Add a `commandcode` provider to OpenQuota for users of the Command Code Go plan. The first version reads usage records already stored locally by Command Code and exposes measured local usage in the OpenQuota dashboard. It does not call a billable model endpoint and does not depend on undocumented account APIs.

## Scope

### In scope

- Discover Command Code local project/session data under the platform home directory, principally `~/.commandcode/projects/**/*.jsonl`.
- Parse usage metadata when it is present in session records.
- Aggregate measured tokens and cost for today, yesterday, and the last 30 days.
- Expose a provider definition, local-data source note, provider links, icon, documentation, and registry coverage.
- Preserve OpenQuota's existing credential-store behavior by not adding an API-key capability to this provider.
- Link users to Command Code Studio Usage for live 5-hour, weekly, and credit limits.

### Out of scope

- Reading `~/.commandcode/auth.json`, API keys, OAuth tokens, or conversation content for display.
- Calling Command Code's model API or generating a request solely to measure usage.
- Scraping undocumented Command Code web endpoints.
- Claiming live quota, remaining credits, or reset times when local records do not provide authoritative values.
- Supporting the Command Code Provider plan in this first version.

## Architecture

Create `src-tauri/src/providers/commandcode/` with focused modules:

- `mod.rs`: provider definition, discovery, snapshot assembly, and `UsageProvider` implementation.
- `local_usage.rs`: safe path discovery, JSONL record loading, date filtering, and aggregation.
- `mapper.rs`: conversion from parser output to OpenQuota `UsageHistory` and value metrics.
- `fixtures/`: sanitized JSONL records for deterministic tests.

Register the provider in `src-tauri/src/providers/mod.rs`, the Tauri composition root in `src-tauri/src/lib.rs`, the provider registry contract, frontend icon registry, fixtures, and provider documentation.

The provider should report a local-data source note and a warning when no usage metadata is available. Malformed records are skipped and counted as local-data warnings; one malformed record must not blank a valid snapshot.

## Data flow

```text
~/.commandcode/projects/**/*.jsonl
        |
        v
safe JSONL parser
        |
        v
timestamp/model/token/cost records
        |
        v
today / yesterday / last-30-day aggregation
        |
        v
ProviderSnapshot -> OpenQuota dashboard
```

The parser must tolerate unknown event types and schema additions. It should only retain fields needed for aggregation: timestamp, model identifier, input/output/total tokens, and cost when explicitly present. It must not log raw records or message text.

## Dashboard behavior

The initial provider exposes measured local usage for today, yesterday, and the last 30 days, plus token/cost metrics when available. Metrics are marked estimated when the source record marks them estimated or when cost is derived from local pricing data.

The provider links include the Command Code Studio Usage page and relevant documentation. The dashboard should make clear that quota/reset values are live in Studio, while OpenQuota's local history may be incomplete. No zero-valued quota is synthesized for missing data.

## Error handling and privacy

- Missing Command Code data directory: provider is not detected.
- Directory exists but contains no usable usage records: provider is detected with a local-data notice.
- Malformed JSONL or unsupported record: skip the record, continue aggregation, and expose a non-secret warning.
- Filesystem failure: return a typed local-data error without exposing paths beyond the provider's user-facing explanation.
- Never read, print, persist, or transmit `auth.json` contents.
- No network requests are made by this provider.

## Testing strategy

Rust tests must be written before implementation code and must cover:

1. Valid records aggregate tokens and explicit costs into today, yesterday, and last-30-day periods.
2. Malformed and unknown records are ignored without losing valid records.
3. Missing usage fields do not become fabricated zeros.
4. The provider discovers the projects directory without reading the auth file.
5. Provider errors remain typed and do not include message content or credentials.
6. Provider definition and default metric layout pass registry validation.

Run the focused Rust tests first, then the existing frontend tests and provider-registry contract. Full verification is required before claiming completion.

## Acceptance criteria

- OpenQuota builds with a registered `commandcode` provider.
- A local Command Code session fixture produces a correct dashboard snapshot.
- No API key or authentication file is accessed by the provider.
- Missing or malformed local data produces an understandable non-fatal state.
- The provider documentation clearly distinguishes local usage history from live quota in Command Code Studio.
- Existing providers and registry order remain intact apart from the intentional addition.

## Follow-up

If Command Code later publishes a supported read-only usage/quota endpoint, add it as a separate capability with explicit authentication and tests. Do not replace the local parser with reverse-engineered endpoints.
