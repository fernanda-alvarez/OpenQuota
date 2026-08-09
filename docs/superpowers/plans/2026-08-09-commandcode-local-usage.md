# Command Code Local Usage Provider Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `commandcode` provider that reads local Command Code session usage for today, yesterday, and the last 30 days without reading credentials or calling billable APIs.

**Architecture:** Add a Rust provider using a focused JSONL scanner under `src-tauri/src/providers/commandcode/`. The scanner extracts only timestamp, model, and token metadata from `~/.commandcode/projects/**/*.jsonl`, estimates cost through the existing `PricingStore`, and feeds `DailyUsageAccumulator`. The provider exposes local usage and a Studio Usage link; live quota/reset data remains outside the provider until Command Code publishes a supported read-only endpoint.

**Tech Stack:** Rust 2021, Tauri 2, `serde_json`, `chrono`, `walkdir`, existing `DailyUsageAccumulator`, existing `PricingStore`, Svelte/TypeScript provider icon registry, Vitest and Cargo tests.

## Global Constraints

- Do not read, print, persist, or transmit `~/.commandcode/auth.json`, API keys, OAuth tokens, or conversation content.
- Do not call a Command Code model endpoint or generate a request solely to measure usage.
- Do not scrape undocumented Command Code web endpoints.
- Do not claim live quota, remaining credits, or reset times without authoritative source data.
- Malformed records must not blank a valid snapshot; skip them and expose a non-secret local-data warning.
- Keep the provider network-free and without an API-key capability.
- Use tests before production code for every new parser, scanner, mapper, and provider behavior.
- Preserve existing provider order and existing provider behavior; add `commandcode` intentionally.

---

## File Map

Create:

- `src-tauri/src/providers/commandcode/mod.rs` — provider definition, error mapping, discovery, snapshot assembly, and `UsageProvider` implementation.
- `src-tauri/src/providers/commandcode/local_usage.rs` — path discovery, JSONL walking, line parsing, deduplication, and aggregation input.
- `src-tauri/src/providers/commandcode/fixtures/session.jsonl` — sanitized usage and non-usage records based on the observed local field names.
- `docs/providers/commandcode.md` — setup, data source, limitations, privacy, and Studio link.
- `src/assets/provider-icons/commandcode.svg` — small monochrome provider icon.

Modify:

- `src-tauri/src/providers/mod.rs` — export `commandcode` and include its definition in provider tests.
- `src-tauri/src/lib.rs` — construct `CommandCodeProvider` with the existing `PricingStore` and append it to the runtime list.
- `src-tauri/src/providers/registry.rs` — include the provider in the canonical catalog test and expected order.
- `src/lib/providerIconPaths.ts` — import and register the Command Code icon.
- `src/test/appFixtures.ts` — add a provider fixture for frontend tests and customization rendering.
- `scripts/verify/verify-provider-registry-contract.js` — add `commandcode` to provider literals and expected runtime order.

---

### Task 1: Add the failing JSONL parser tests

**Files:**

- Create: `src-tauri/src/providers/commandcode/mod.rs` (module declaration only: `mod local_usage;`)
- Modify: `src-tauri/src/providers/mod.rs` (add `pub mod commandcode;` so the focused unit tests compile)
- Create: `src-tauri/src/providers/commandcode/local_usage.rs`
- Create: `src-tauri/src/providers/commandcode/fixtures/session.jsonl`

**Interfaces:**

- Produces the parser contract used by later tasks:

```rust
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

pub(crate) fn parse_jsonl(content: &str) -> Vec<CommandCodeUsageEvent>;
```

- [ ] **Step 1: Write the failing test**

Add a `#[cfg(test)]` module that imports `parse_jsonl` and asserts that a fixture with a user message, an assistant usage record, and an unknown event returns exactly one usage event. The fixture must use these observed keys without message text that identifies a real project:

```json
{"type":"assistant","version":1,"id":"event-1","timestamp":"2026-08-09T00:00:00Z","model":"deepseek/deepseek-v4-flash","usage":{"inputTokens":100,"outputTokens":40,"cacheReadTokens":20,"cacheWriteTokens":0},"content":"[redacted]"}
{"type":"user","version":1,"id":"event-2","timestamp":"2026-08-09T00:00:01Z","content":"[redacted]"}
{"type":"heartbeat","version":1,"id":"event-3","timestamp":"2026-08-09T00:00:02Z"}
```

Assert the event ID, timestamp, model, and all four token counters. Assert that a line containing `content` but no usage object produces no event.

- [ ] **Step 2: Run test to verify it fails**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml commandcode::local_usage
```

Expected: FAIL because the `commandcode` module and `parse_jsonl` implementation do not exist yet. This is the expected feature-missing failure.

- [ ] **Step 3: Commit**

```powershell
git add src-tauri/src/providers/commandcode/mod.rs src-tauri/src/providers/commandcode/local_usage.rs src-tauri/src/providers/commandcode/fixtures/session.jsonl src-tauri/src/providers/mod.rs
git commit -m "test: define Command Code usage event parser"
```

Include the module declaration files in this commit so the focused test command actually compiles and exercises the new failing tests.

### Task 2: Implement and test the JSONL parser

**Files:**

- Modify: `src-tauri/src/providers/commandcode/local_usage.rs`

**Interfaces:**

- Consumes raw JSONL text.
- Produces `Vec<CommandCodeUsageEvent>`.
- Reads only `id`, `timestamp`, `model`, and the numeric usage fields. It must never copy `content` into the event.

- [ ] **Step 1: Write the failing edge-case tests**

Add tests for malformed JSON, missing timestamp, non-finite or negative token values, numeric strings, and duplicate event IDs. The desired behavior is:

```rust
assert_eq!(parse_jsonl("not-json\n{}\n").len(), 0);
assert_eq!(parse_jsonl(line_without_timestamp).len(), 0);
assert_eq!(parse_jsonl(line_with_negative_tokens).len(), 0);
assert_eq!(parse_jsonl(line_with_string_tokens).len(), 1);
assert_eq!(parse_jsonl(duplicate_lines).len(), 1);
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml commandcode::local_usage
```

Expected: the new assertions fail because parsing and deduplication are not implemented.

- [ ] **Step 3: Implement the minimal parser**

Use `serde_json::Value` per line. Require a valid RFC3339 `timestamp` and non-empty `id`; accept `model` as optional. Read usage counters from `usage.inputTokens`, `usage.outputTokens`, `usage.cacheReadTokens`, and `usage.cacheWriteTokens`, accepting JSON numbers or numeric strings and rejecting negative/non-finite values. Keep only one event per ID, preferring the record with the larger total token count when duplicates occur.

- [ ] **Step 4: Run tests to verify they pass**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml commandcode::local_usage
```

Expected: all parser tests PASS.

- [ ] **Step 5: Commit**

```powershell
git add src-tauri/src/providers/commandcode/local_usage.rs
git commit -m "feat: parse Command Code local usage records"
```

### Task 3: Add local path scanning and usage aggregation

**Files:**

- Modify: `src-tauri/src/providers/commandcode/local_usage.rs`

**Interfaces:**

- Add:

```rust
pub(crate) struct CommandCodeUsageScanner {
    root: PathBuf,
}

impl CommandCodeUsageScanner {
    pub(crate) fn new() -> Self;
    #[cfg(test)] pub(crate) fn for_root(root: PathBuf) -> Self;
    pub(crate) fn scan(&self, now: DateTime<Utc>, pricing: &ModelPricing)
        -> Result<Option<CommandCodeUsageScan>, CommandCodeLocalError>;
    pub(crate) fn has_usage_directory(&self) -> bool;
}

pub(crate) struct CommandCodeUsageScan {
    pub(crate) usage: UsageHistory,
    pub(crate) warnings: Vec<String>,
}
```

- [ ] **Step 1: Write the failing scanner tests**

Use `tempdir()` and write sanitized JSONL files under `projects/project-a/session.jsonl`. Add tests that:

1. Scan two events on today and yesterday and aggregate the expected token totals.
2. Ignore events older than 30 local calendar days or later than `now`.
3. Use `PricingStore` estimates for known models and put unknown models in `unknown_models` without inventing cost.
4. Skip `.checkpoints.jsonl` and non-JSONL files.
5. Return `Ok(None)` when the projects directory is absent.
6. Return a warning when one file is unreadable but another file remains usable.

- [ ] **Step 2: Run tests to verify they fail**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml commandcode::local_usage
```

Expected: FAIL because scanner types and aggregation do not exist.

- [ ] **Step 3: Implement scanning and aggregation**

Resolve the home directory from `HOME` or `USERPROFILE`, append `.commandcode/projects`, walk only regular files ending in `.jsonl`, and exclude filenames ending in `.checkpoints.jsonl`. Parse each file with `parse_jsonl`, convert timestamps to the user's local date, and discard records outside the 30-day window.

Build a `TokenBreakdown` from input, cache-read, cache-write, and output counters. Calculate total tokens as the sum of those counters. For known models call `pricing.estimated_cost_dollars(model, tokens, true)` and add estimated usage to `DailyUsageAccumulator`; for unknown models call `add_unknown_model` only. Build the final history with the source note `From Command Code local session records; missing costs use catalog estimates`.

- [ ] **Step 4: Run tests to verify they pass**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml commandcode::local_usage
```

Expected: all scanner, aggregation, and parser tests PASS.

- [ ] **Step 5: Commit**

```powershell
git add src-tauri/src/providers/commandcode/local_usage.rs src-tauri/src/providers/commandcode/fixtures/session.jsonl
git commit -m "feat: aggregate Command Code local usage"
```

### Task 4: Implement the provider runtime

**Files:**

- Create: `src-tauri/src/providers/commandcode/mod.rs`
- Modify: `src-tauri/src/providers/mod.rs`

**Interfaces:**

- `CommandCodeProvider::new(pricing: Arc<PricingStore>) -> Self`.
- `definition() -> ProviderDefinition` with provider ID `commandcode`, display name `Command Code`, short name `CC`, no API-key capability, and a Studio Usage link.
- `UsageProvider::has_local_credentials()` returns whether the projects directory exists; it must not inspect `auth.json`.
- `UsageProvider::refresh()` returns a `ProviderSnapshot` with `provider_id: "commandcode"`, `usage` from the scanner, no quota windows, and non-secret warnings.

- [ ] **Step 1: Write the failing provider tests**

Add tests for:

```rust
assert_eq!(definition().id, "commandcode");
assert_eq!(definition().display_name, "Command Code");
assert!(definition().metrics.iter().any(|m| m.id == "commandcode.today"));
assert!(definition().metrics.iter().any(|m| m.id == "commandcode.trend"));
assert_eq!(provider.api_key_status(), None);
```

Add a snapshot test with a temporary projects directory and a fixed `now`, asserting that usage is present and `quotas.is_empty()`.

- [ ] **Step 2: Run tests to verify they fail**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml commandcode
```

Expected: FAIL because the provider module and runtime do not exist.

- [ ] **Step 3: Implement the provider**

Follow the existing OpenCode provider structure. Define usage metrics for today, yesterday, last 30 days, and a trend; set today and trend to always-visible without pinning more than the registry allows. Return `NotDetected` as `ProviderErrorKind::LocalData` when the projects directory is absent, and return a typed local-data error for filesystem failures. Add an informational notice or source note linking to `https://commandcode.ai/studio` usage data without claiming that the local snapshot is live.

- [ ] **Step 4: Run tests to verify they pass**

Run:

```powershell
cargo test --manifest-path src-tauri/Cargo.toml commandcode
```

Expected: all provider tests PASS.

- [ ] **Step 5: Commit**

```powershell
git add src-tauri/src/providers/commandcode src-tauri/src/providers/mod.rs
git commit -m "feat: add Command Code local usage provider"
```

### Task 5: Register the provider and update frontend metadata

**Files:**

- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/providers/registry.rs`
- Modify: `scripts/verify/verify-provider-registry-contract.js`
- Modify: `src/lib/providerIconPaths.ts`
- Modify: `src/test/appFixtures.ts`
- Create: `src/assets/provider-icons/commandcode.svg`

**Interfaces:**

- The runtime list adds `Arc::new(CommandCodeProvider::new(pricing.clone()))` after `ZaiProvider`.
- The canonical provider order ends with `"zai", "commandcode"`.
- The frontend resolves `providerId === "commandcode"` to the new icon without a fallback icon.

- [ ] **Step 1: Write the failing registry/fixture tests**

Add `commandcode` to the registry catalog test and contract expected list before runtime registration. Add a frontend fixture whose provider ID is `commandcode` and whose catalog includes `commandcode.today`, `commandcode.yesterday`, `commandcode.last30`, and `commandcode.trend`.

- [ ] **Step 2: Run tests to verify they fail**

Run:

```powershell
corepack pnpm verify:contracts
corepack pnpm test -- src/App.test.ts src/App.customization.test.ts
```

Expected: FAIL because the runtime order, icon registry, and fixture do not yet include Command Code.

- [ ] **Step 3: Implement registration and assets**

Add the module import and runtime construction, update the registry contract's provider literal and expected order, add a valid monochrome SVG using the existing icon dimensions, register it in `providerIconPaths.ts`, and extend the fixture catalog without adding provider-specific logic to shared frontend components.

- [ ] **Step 4: Run tests to verify they pass**

Run:

```powershell
corepack pnpm verify:contracts
corepack pnpm test -- src/App.test.ts src/App.customization.test.ts
```

Expected: PASS with the new provider included and all existing providers unchanged.

- [ ] **Step 5: Commit**

```powershell
git add src-tauri/src/lib.rs src-tauri/src/providers/registry.rs scripts/verify/verify-provider-registry-contract.js src/lib/providerIconPaths.ts src/test/appFixtures.ts src/assets/provider-icons/commandcode.svg
git commit -m "feat: register Command Code provider"
```

### Task 6: Add documentation and run full verification

**Files:**

- Create: `docs/providers/commandcode.md`

- [ ] **Step 1: Write the provider documentation**

Document that OpenQuota reads local session metadata from `~/.commandcode/projects`, estimates cost from the existing pricing catalog, ignores conversation content, and does not read `auth.json`. Explain that live Go Plan quota and reset information remains in Command Code Studio Usage and link to `https://commandcode.ai/docs/resources/usage-limits`.

- [ ] **Step 2: Run focused verification**

Run:

```powershell
cargo fmt --manifest-path src-tauri/Cargo.toml --all
cargo test --manifest-path src-tauri/Cargo.toml commandcode
corepack pnpm verify:contracts
corepack pnpm test
```

Expected: PASS with no secret values in test output.

- [ ] **Step 3: Run complete verification**

Run:

```powershell
corepack pnpm verify
```

Expected: version checks, contracts, frontend checks, and Rust checks all PASS.

- [ ] **Step 4: Inspect the final diff**

Run:

```powershell
git diff upstream/main...HEAD --stat
git diff --check upstream/main...HEAD
git status --short --branch
```

Confirm that only the intended provider, registry, frontend metadata, docs, fixtures, and test changes are present; confirm no API key, auth file, conversation content, or generated local data is tracked.

- [ ] **Step 5: Commit documentation and verification fixes**

```powershell
git add docs/providers/commandcode.md
git commit -m "docs: document Command Code local usage"
```

After the final commit, push the feature branch to the fork and report the exact verification commands and results.
