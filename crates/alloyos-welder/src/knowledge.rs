//! Local User Knowledge & Profile Learning Framework (`/knowledge/`).
//!
//! Maintains a local Markdown repository under `/knowledge/` on CobaltFS (or volatile RamFS fallback):
//! - `/knowledge/profile.md`      — Stable facts explicitly provided by the user
//! - `/knowledge/preferences.md`  — Scoped communication, visual, and workflow preferences
//! - `/knowledge/interests.md`    — Recurring interests and ongoing goals
//! - `/knowledge/observations.md` — Tentative patterns inferred from conversation
//! - `/knowledge/projects/*.md`   — Context scoped to individual projects
//!
//! Architectural invariants:
//! 1. Profile learning is owned by Welder; there is no `/skills/` directory (skills live under `/apps/<app_id>/SKILL.md`).
//! 2. Explicit user statements and corrections take precedence over earlier records and tentative observations.
//! 3. Uncertain observations remain in `observations.md` (`Basis: Inferred observation.`) and never overwrite explicit facts.
//! 4. One-off or component-specific requests (e.g. a monochrome boot-screen request) are scoped to their target rather than global.
//! 5. Secrets and credentials are never retained in `/knowledge/`.
//! 6. Apps may supply context through Welder, but cannot directly rewrite the user profile.
//! 7. CobaltFS provides durable persistence; RamFS is explicitly reported as volatile and never presented as surviving reboot.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use alloyos_forge::provider::json_escape;
use alloyos_mcp::{AppTile, McpValue, TileAction, TileItem};
use serde::{Deserialize, Serialize};

pub const KNOWLEDGE_ROOT: &str = "/knowledge";
pub const KNOWLEDGE_PROFILE_PATH: &str = "/knowledge/profile.md";
pub const KNOWLEDGE_PREFERENCES_PATH: &str = "/knowledge/preferences.md";
pub const KNOWLEDGE_INTERESTS_PATH: &str = "/knowledge/interests.md";
pub const KNOWLEDGE_OBSERVATIONS_PATH: &str = "/knowledge/observations.md";
pub const KNOWLEDGE_PROJECTS_DIR: &str = "/knowledge/projects";
pub const KNOWLEDGE_TILE_ID: &str = "knowledge.profile";

pub const MAX_ENTRY_TITLE_BYTES: usize = 120;
pub const MAX_ENTRY_VALUE_BYTES: usize = 512;
pub const MAX_ENTRY_SCOPE_BYTES: usize = 96;
pub const MAX_ENTRY_SOURCE_BYTES: usize = 96;
pub const MAX_KNOWLEDGE_FILE_BYTES: usize = 16 * 1024;
pub const DEFAULT_SUMMARY_MAX_BYTES: usize = 640;
pub const DEFAULT_RETRIEVAL_MAX_BYTES: usize = 1024;

pub const DEFAULT_PROFILE_MD: &str = "# User Profile\n";
pub const DEFAULT_PREFERENCES_MD: &str = "# User Preferences\n";
pub const DEFAULT_INTERESTS_MD: &str = "# User Interests & Goals\n";
pub const DEFAULT_OBSERVATIONS_MD: &str = "# Tentative Observations\n";

/// Distinguishes durable disk-backed CobaltFS storage from a volatile in-memory RamFS fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KnowledgeStorageKind {
    /// Persistent CobaltFS volume; records survive reboot.
    DurableCobaltFs,
    /// Volatile RamFS fallback; records are lost on reboot and must not be presented as durable.
    VolatileRamFs,
}

impl KnowledgeStorageKind {
    pub const fn is_persistent(self) -> bool {
        matches!(self, Self::DurableCobaltFs)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DurableCobaltFs => "CobaltFS",
            Self::VolatileRamFs => "RamFS",
        }
    }

    pub const fn durability_notice(self) -> &'static str {
        match self {
            Self::DurableCobaltFs => "durable (CobaltFS disk persistence)",
            Self::VolatileRamFs => "volatile (RamFS fallback; does not survive reboot)",
        }
    }
}

/// Canonical knowledge document categories under `/knowledge/`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum KnowledgeCategory {
    Profile,
    Preferences,
    Interests,
    Observations,
    Project(String),
}

impl KnowledgeCategory {
    /// Parses a category identifier (`profile`, `preferences`, `interests`, `observations`,
    /// `project:<slug>`, `projects/<slug>`) or a canonical `/knowledge/...` path.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("knowledge category or path must not be empty".to_string());
        }
        if trimmed.contains("..") || trimmed.contains('\\') {
            return Err(format!(
                "invalid knowledge path or category '{}': path traversal is forbidden",
                trimmed
            ));
        }

        let lower = trimmed.to_ascii_lowercase();
        match lower.as_str() {
            "profile" | "fact" | "facts" | "profile.md" | "/knowledge/profile.md" => {
                Ok(Self::Profile)
            }
            "preferences"
            | "preference"
            | "prefs"
            | "preferences.md"
            | "/knowledge/preferences.md" => Ok(Self::Preferences),
            "interests" | "interest" | "goals" | "goal" | "interests.md"
            | "/knowledge/interests.md" => Ok(Self::Interests),
            "observations"
            | "observation"
            | "tentative"
            | "observations.md"
            | "/knowledge/observations.md" => Ok(Self::Observations),
            _ => {
                let project_raw = if let Some(rest) = lower.strip_prefix("/knowledge/projects/") {
                    rest.strip_suffix(".md").unwrap_or(rest)
                } else if let Some(rest) = lower.strip_prefix("projects/") {
                    rest.strip_suffix(".md").unwrap_or(rest)
                } else if let Some(rest) = lower.strip_prefix("project:") {
                    rest.strip_suffix(".md").unwrap_or(rest)
                } else if let Some(rest) = lower.strip_prefix("project/") {
                    rest.strip_suffix(".md").unwrap_or(rest)
                } else {
                    return Err(format!(
                        "unknown knowledge category or path '{}'; expected profile, preferences, interests, observations, or projects/<slug>",
                        trimmed
                    ));
                };
                let slug = validate_project_slug(project_raw)?;
                Ok(Self::Project(slug))
            }
        }
    }

    pub fn path(&self) -> String {
        match self {
            Self::Profile => KNOWLEDGE_PROFILE_PATH.to_string(),
            Self::Preferences => KNOWLEDGE_PREFERENCES_PATH.to_string(),
            Self::Interests => KNOWLEDGE_INTERESTS_PATH.to_string(),
            Self::Observations => KNOWLEDGE_OBSERVATIONS_PATH.to_string(),
            Self::Project(slug) => format!("{}/{}.md", KNOWLEDGE_PROJECTS_DIR, slug),
        }
    }

    pub fn id(&self) -> String {
        match self {
            Self::Profile => "profile".to_string(),
            Self::Preferences => "preferences".to_string(),
            Self::Interests => "interests".to_string(),
            Self::Observations => "observations".to_string(),
            Self::Project(slug) => format!("projects/{}", slug),
        }
    }

    pub const fn primary_field_label(&self) -> &'static str {
        match self {
            Self::Profile => "Fact",
            Self::Preferences => "Preference",
            Self::Interests => "Interest",
            Self::Observations => "Observation",
            Self::Project(_) => "Preference",
        }
    }

    pub fn document_header(&self) -> String {
        match self {
            Self::Profile => "# User Profile".to_string(),
            Self::Preferences => "# User Preferences".to_string(),
            Self::Interests => "# User Interests & Goals".to_string(),
            Self::Observations => "# Tentative Observations".to_string(),
            Self::Project(slug) => format!("# Project Knowledge: {}", slug),
        }
    }
}

/// Whether an entry was explicitly stated by the user or inferred as a tentative pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KnowledgeBasis {
    Explicit,
    Inferred,
}

impl KnowledgeBasis {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let lower = raw.trim().to_ascii_lowercase();
        if lower.is_empty()
            || lower == "explicit"
            || lower.starts_with("explicit user statement")
            || lower == "user"
            || lower == "stated"
        {
            Ok(Self::Explicit)
        } else if lower == "inferred"
            || lower.starts_with("inferred observation")
            || lower.starts_with("tentative")
            || lower == "observation"
        {
            Ok(Self::Inferred)
        } else {
            Err(format!(
                "invalid knowledge basis '{}'; expected 'explicit' or 'inferred'",
                raw
            ))
        }
    }

    pub const fn as_markdown(self) -> &'static str {
        match self {
            Self::Explicit => "Explicit user statement.",
            Self::Inferred => "Inferred observation.",
        }
    }

    pub const fn as_code(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Inferred => "inferred",
        }
    }
}

/// A single structured Markdown record inside `/knowledge/*.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeEntry {
    pub category: KnowledgeCategory,
    pub title: String,
    pub value: String,
    pub basis: KnowledgeBasis,
    pub source: String,
    pub updated: String,
    pub scope: String,
}

impl KnowledgeEntry {
    pub fn new(
        category: KnowledgeCategory,
        title: impl Into<String>,
        value: impl Into<String>,
        basis: KnowledgeBasis,
        source: impl Into<String>,
        updated: impl Into<String>,
        scope: impl Into<String>,
    ) -> Self {
        Self {
            category,
            title: title.into(),
            value: value.into(),
            basis,
            source: source.into(),
            updated: updated.into(),
            scope: scope.into(),
        }
    }

    pub fn matches_query(&self, query: &str) -> bool {
        let q = query.trim().to_ascii_lowercase();
        if q.is_empty() || q == "*" {
            return true;
        }
        let title_lower = self.title.to_ascii_lowercase();
        let val_lower = self.value.to_ascii_lowercase();
        let scope_lower = self.scope.to_ascii_lowercase();
        if title_lower.contains(&q) || val_lower.contains(&q) || scope_lower.contains(&q) {
            return true;
        }
        q.split_whitespace()
            .filter(|t| t.len() >= 3)
            .any(|tok| title_lower.contains(tok) || val_lower.contains(tok) || scope_lower.contains(tok))
    }

    /// Renders this entry in the canonical `KNOWLEDGE.md` Markdown section format:
    /// ```markdown
    /// ### Visual style
    /// - Preference: Minimal black-and-white boot screen.
    /// - Basis: Explicit user statement.
    /// - Source: Conversation turn identifier.
    /// - Updated: Timestamp supplied by the runtime.
    /// - Scope: alloyOS boot screen.
    /// ```
    pub fn to_markdown_block(&self) -> String {
        format!(
            "### {}\n- {}: {}\n- Basis: {}\n- Source: {}\n- Updated: {}\n- Scope: {}\n",
            self.title,
            self.category.primary_field_label(),
            self.value,
            self.basis.as_markdown(),
            self.source,
            self.updated,
            self.scope,
        )
    }

    pub fn to_json(&self) -> String {
        format!(
            "{{\"category\":\"{}\",\"path\":\"{}\",\"title\":\"{}\",\"value\":\"{}\",\"basis\":\"{}\",\"source\":\"{}\",\"updated\":\"{}\",\"scope\":\"{}\"}}",
            json_escape(&self.category.id()),
            json_escape(&self.category.path()),
            json_escape(&self.title),
            json_escape(&self.value),
            self.basis.as_code(),
            json_escape(&self.source),
            json_escape(&self.updated),
            json_escape(&self.scope),
        )
    }
}

/// Search match returned by `knowledge_search` and relevant context retrieval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeSearchMatch {
    pub entry: KnowledgeEntry,
    pub score: u32,
}

/// Summary of automatic learning performed on a conversational turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeTurnReport {
    pub turn_id: String,
    pub learning_paused: bool,
    pub storage_kind: KnowledgeStorageKind,
    pub upserted: Vec<KnowledgeEntry>,
    pub forgotten: Vec<String>,
}

impl KnowledgeTurnReport {
    pub fn has_changes(&self) -> bool {
        !self.upserted.is_empty() || !self.forgotten.is_empty()
    }
}

/// Validates a project slug for `/knowledge/projects/<slug>.md`.
pub fn validate_project_slug(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_matches('/');
    if trimmed.is_empty() || trimmed.len() > 48 {
        return Err("project slug must be 1..=48 characters".to_string());
    }
    if trimmed.contains("..") || trimmed.contains('/') || trimmed.contains('\\') {
        return Err(format!("invalid project slug '{}'", raw));
    }
    let mut slug = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if ch == '-' || ch == '_' || ch == ' ' {
            if !slug.ends_with('-') && !slug.is_empty() {
                slug.push('-');
            }
        } else {
            return Err(format!(
                "invalid character '{}' in project slug '{}'",
                ch, raw
            ));
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        return Err("project slug must contain alphanumeric characters".to_string());
    }
    Ok(slug)
}

/// Returns `true` if `path` is inside `/knowledge` (or is `/knowledge` itself).
pub fn is_knowledge_path(path: &str) -> bool {
    let trimmed = path.trim();
    trimmed == KNOWLEDGE_ROOT || trimmed.starts_with("/knowledge/")
}

/// Checks whether a candidate title or value contains secrets, API keys, or credentials.
/// Profile extraction and `knowledge_upsert` must never retain secrets.
pub fn contains_secret_or_credential(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    for token_prefix in [
        "sk-",
        "aiza",
        "ghp_",
        "github_pat_",
        "hf_",
        "xoxb-",
        "xoxp-",
        "bearer ",
        "-----begin ",
    ] {
        if lower.contains(token_prefix) {
            return true;
        }
    }

    for secret_key in [
        "api_key",
        "apikey",
        "api-key",
        "secret_key",
        "private_key",
        "access_token",
        "refresh_token",
        "auth_token",
        "password",
        "passwd",
        "passphrase",
        "client_secret",
    ] {
        if lower.contains(secret_key) {
            return true;
        }
    }

    // Also catch "my password is ...", "my token is ...", "api key is ...", "secret is ..."
    for phrase in [
        "my password",
        "my api key",
        "my secret",
        "my token",
        "api key:",
        "api key =",
        "password:",
        "password =",
        "secret:",
        "secret =",
        "token:",
        "token =",
    ] {
        if lower.contains(phrase) {
            return true;
        }
    }

    false
}

/// Validates and normalizes a `KnowledgeEntry` before storage.
pub fn validate_entry(mut entry: KnowledgeEntry) -> Result<KnowledgeEntry, String> {
    entry.title = sanitize_single_line(&entry.title);
    entry.value = sanitize_single_line(&entry.value);
    entry.source = sanitize_single_line(&entry.source);
    entry.updated = sanitize_single_line(&entry.updated);
    entry.scope = sanitize_single_line(&entry.scope);

    if entry.title.is_empty() {
        return Err("knowledge entry title must not be empty".to_string());
    }
    if entry.title.len() > MAX_ENTRY_TITLE_BYTES {
        return Err(format!(
            "knowledge entry title exceeds {} bytes",
            MAX_ENTRY_TITLE_BYTES
        ));
    }
    if entry.value.is_empty() {
        return Err("knowledge entry value must not be empty".to_string());
    }
    if entry.value.len() > MAX_ENTRY_VALUE_BYTES {
        return Err(format!(
            "knowledge entry value exceeds {} bytes",
            MAX_ENTRY_VALUE_BYTES
        ));
    }
    if entry.scope.is_empty() {
        entry.scope = match &entry.category {
            KnowledgeCategory::Project(slug) => slug.clone(),
            _ => "global".to_string(),
        };
    }
    if entry.scope.len() > MAX_ENTRY_SCOPE_BYTES {
        return Err(format!(
            "knowledge entry scope exceeds {} bytes",
            MAX_ENTRY_SCOPE_BYTES
        ));
    }
    if entry.source.is_empty() {
        entry.source = "conversation".to_string();
    }
    if entry.source.len() > MAX_ENTRY_SOURCE_BYTES {
        return Err(format!(
            "knowledge entry source exceeds {} bytes",
            MAX_ENTRY_SOURCE_BYTES
        ));
    }
    if entry.updated.is_empty() {
        entry.updated = "t+0ms".to_string();
    }

    if contains_secret_or_credential(&entry.title)
        || contains_secret_or_credential(&entry.value)
        || contains_secret_or_credential(&entry.scope)
    {
        return Err(
            "refusing to store secret, API key, or credential in user knowledge repository"
                .to_string(),
        );
    }

    // Uncertain observations must never be stored as asserted facts in `profile.md`.
    if entry.basis == KnowledgeBasis::Inferred && entry.category == KnowledgeCategory::Profile {
        entry.category = KnowledgeCategory::Observations;
    }
    // Entries in `observations.md` are always tentative / inferred observations.
    if entry.category == KnowledgeCategory::Observations {
        entry.basis = KnowledgeBasis::Inferred;
    }

    Ok(entry)
}

fn sanitize_single_line(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_space = false;
    for ch in raw.trim().chars() {
        if ch == '\n' || ch == '\r' || ch == '\t' || ch == ' ' {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

/// Parses a `/knowledge/*.md` Markdown document into structured `KnowledgeEntry` items.
pub fn parse_knowledge_markdown(
    category: &KnowledgeCategory,
    markdown: &str,
) -> Vec<KnowledgeEntry> {
    let mut entries = Vec::new();
    let mut current_title: Option<String> = None;
    let mut current_value: Option<String> = None;
    let mut current_basis = if *category == KnowledgeCategory::Observations {
        KnowledgeBasis::Inferred
    } else {
        KnowledgeBasis::Explicit
    };
    let mut current_source = String::from("conversation");
    let mut current_updated = String::from("t+0ms");
    let mut current_scope = match category {
        KnowledgeCategory::Project(slug) => slug.clone(),
        _ => String::from("global"),
    };

    let flush = |entries: &mut Vec<KnowledgeEntry>,
                 title: &mut Option<String>,
                 value: &mut Option<String>,
                 basis: KnowledgeBasis,
                 source: &str,
                 updated: &str,
                 scope: &str| {
        if let (Some(t), Some(v)) = (title.take(), value.take()) {
            if !t.is_empty() && !v.is_empty() {
                entries.push(KnowledgeEntry {
                    category: category.clone(),
                    title: t,
                    value: v,
                    basis,
                    source: source.to_string(),
                    updated: updated.to_string(),
                    scope: scope.to_string(),
                });
            }
        }
    };

    for raw_line in markdown.lines() {
        let line = raw_line.trim();
        if let Some(heading) = line.strip_prefix("### ") {
            flush(
                &mut entries,
                &mut current_title,
                &mut current_value,
                current_basis,
                &current_source,
                &current_updated,
                &current_scope,
            );
            current_title = Some(heading.trim().to_string());
            current_value = None;
            current_basis = if *category == KnowledgeCategory::Observations {
                KnowledgeBasis::Inferred
            } else {
                KnowledgeBasis::Explicit
            };
            current_source = String::from("conversation");
            current_updated = String::from("t+0ms");
            current_scope = match category {
                KnowledgeCategory::Project(slug) => slug.clone(),
                _ => String::from("global"),
            };
        } else if let Some(bullet) = line.strip_prefix("- ") {
            if let Some((k, v)) = bullet.split_once(':') {
                let key = k.trim().to_ascii_lowercase();
                let val = v.trim();
                match key.as_str() {
                    "fact" | "preference" | "interest" | "observation" | "context" | "value" => {
                        current_value = Some(val.to_string());
                    }
                    "basis" => {
                        if let Ok(b) = KnowledgeBasis::parse(val) {
                            current_basis = b;
                        }
                    }
                    "source" => {
                        if !val.is_empty() {
                            current_source = val.to_string();
                        }
                    }
                    "updated" => {
                        if !val.is_empty() {
                            current_updated = val.to_string();
                        }
                    }
                    "scope" => {
                        if !val.is_empty() {
                            current_scope = val.to_string();
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    flush(
        &mut entries,
        &mut current_title,
        &mut current_value,
        current_basis,
        &current_source,
        &current_updated,
        &current_scope,
    );

    entries
}

/// Renders a list of `KnowledgeEntry` records into a canonical `/knowledge/*.md` Markdown file.
pub fn render_knowledge_markdown(
    category: &KnowledgeCategory,
    entries: &[KnowledgeEntry],
) -> String {
    let mut out = category.document_header();
    out.push('\n');
    for entry in entries {
        out.push('\n');
        out.push_str(&entry.to_markdown_block());
    }
    out
}

pub fn normalize_topic_key(s: &str) -> String {
    let mut out = String::new();
    for ch in s.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if (ch == ' ' || ch == '-' || ch == '_') && !out.ends_with(' ') && !out.is_empty() {
            out.push(' ');
        }
    }
    out.trim().to_string()
}

/// Merges `incoming` into `entries` for a given `category` according to `KNOWLEDGE.md` precedence rules:
/// - Explicit statements/corrections overwrite earlier entries with the same normalized title & compatible scope.
/// - Tentative/inferred observations never overwrite an existing `Explicit` entry.
/// - Returns `true` if `entries` was modified.
pub fn merge_entry_into_list(entries: &mut Vec<KnowledgeEntry>, incoming: KnowledgeEntry) -> bool {
    let incoming_key = normalize_topic_key(&incoming.title);
    let incoming_scope_key = normalize_topic_key(&incoming.scope);

    if let Some(existing) = entries.iter_mut().find(|e| {
        let same_title = normalize_topic_key(&e.title) == incoming_key;
        let existing_scope = normalize_topic_key(&e.scope);
        same_title
            && (existing_scope == incoming_scope_key
                || incoming.basis == KnowledgeBasis::Explicit && existing_scope == "global")
    }) {
        // An inferred observation must never overwrite an explicit user statement.
        if existing.basis == KnowledgeBasis::Explicit && incoming.basis == KnowledgeBasis::Inferred
        {
            return false;
        }
        *existing = incoming;
        return true;
    }

    entries.push(incoming);
    true
}

/// Extracts structured knowledge updates and/or forget directives from a user conversation turn.
#[derive(Debug, Clone, Default)]
pub struct ExtractedTurnSignals {
    pub forget_queries: Vec<String>,
    pub entries: Vec<KnowledgeEntry>,
}

/// Analyzes a user utterance from a conversation turn (`turn_id`, `timestamp`) and extracts:
/// - User forget/delete commands ("forget my preference for ...", "delete my record about ...")
/// - Explicit user corrections ("actually, my name is ...", "correction: I prefer ...")
/// - Explicit user profile facts ("my name is ...", "call me ...", "I work as ...", "my timezone is ...")
/// - Scoped or global preferences ("I prefer ...", "use a minimal black-and-white boot screen for alloyOS")
/// - Interests & goals ("I'm interested in ...", "my goal is ...", "I am building ...")
/// - Tentative observations ("maybe I prefer ...", "I might want ...")
pub fn extract_signals_from_user_turn(
    user_text: &str,
    turn_id: &str,
    timestamp: &str,
) -> ExtractedTurnSignals {
    let mut signals = ExtractedTurnSignals::default();
    let trimmed = user_text.trim();
    if trimmed.is_empty() || trimmed.starts_with('/') {
        return signals;
    }

    // Split into clauses by sentence boundaries ('.', ';', '\n') while preserving content.
    for raw_clause in trimmed.split(['.', ';', '\n', '!']) {
        let clause = raw_clause.trim();
        if clause.is_empty() {
            continue;
        }
        if contains_secret_or_credential(clause) {
            continue;
        }

        let lower = clause.to_ascii_lowercase();

        // 1. Check for natural-language "forget" / "delete from knowledge" requests
        if let Some(forget_target) = extract_forget_target(clause, &lower) {
            if !forget_target.is_empty() {
                signals.forget_queries.push(forget_target);
                continue;
            }
        }

        // Determine if this clause is a user correction
        let (clean_clause, is_correction) = strip_correction_prefix(clause);
        let clean_lower = clean_clause.to_ascii_lowercase();
        let source = if is_correction {
            format!("{}:user-correction", turn_id)
        } else {
            turn_id.to_string()
        };

        // Determine if this clause is tentative / uncertain
        let is_tentative = is_tentative_clause(&clean_lower);
        let basis = if is_tentative && !is_correction {
            KnowledgeBasis::Inferred
        } else {
            KnowledgeBasis::Explicit
        };

        // Detect scope (global vs component/project-scoped)
        let (detected_scope, detected_project) = detect_clause_scope(clean_clause, &clean_lower);

        // 2. Profile facts: Name, Role, Location, Timezone
        if let Some(name) = extract_name_fact(clean_clause, &clean_lower) {
            let entry = KnowledgeEntry {
                category: if basis == KnowledgeBasis::Inferred {
                    KnowledgeCategory::Observations
                } else {
                    KnowledgeCategory::Profile
                },
                title: "Preferred name".to_string(),
                value: name,
                basis,
                source: source.clone(),
                updated: timestamp.to_string(),
                scope: "global".to_string(),
            };
            if let Ok(valid) = validate_entry(entry) {
                signals.entries.push(valid);
                continue;
            }
        }

        if let Some((fact_title, fact_val)) = extract_profile_fact(clean_clause, &clean_lower) {
            let entry = KnowledgeEntry {
                category: if basis == KnowledgeBasis::Inferred {
                    KnowledgeCategory::Observations
                } else {
                    KnowledgeCategory::Profile
                },
                title: fact_title,
                value: fact_val,
                basis,
                source: source.clone(),
                updated: timestamp.to_string(),
                scope: "global".to_string(),
            };
            if let Ok(valid) = validate_entry(entry) {
                signals.entries.push(valid);
                continue;
            }
        }

        // 3. Interests and ongoing goals
        if let Some((int_title, int_val)) = extract_interest_or_goal(clean_clause, &clean_lower) {
            let category = if basis == KnowledgeBasis::Inferred {
                KnowledgeCategory::Observations
            } else {
                KnowledgeCategory::Interests
            };
            let entry = KnowledgeEntry {
                category,
                title: int_title,
                value: int_val,
                basis,
                source: source.clone(),
                updated: timestamp.to_string(),
                scope: detected_scope.clone(),
            };
            if let Ok(valid) = validate_entry(entry) {
                signals.entries.push(valid);
                continue;
            }
        }

        // 4. Preferences (communication, visual, workflow, or project-scoped)
        if let Some((pref_title, pref_val, specific_scope)) =
            extract_preference(clean_clause, &clean_lower, &detected_scope)
        {
            let category = if basis == KnowledgeBasis::Inferred {
                KnowledgeCategory::Observations
            } else if let Some(proj_slug) = &detected_project {
                KnowledgeCategory::Project(proj_slug.clone())
            } else {
                KnowledgeCategory::Preferences
            };
            let entry = KnowledgeEntry {
                category,
                title: pref_title.clone(),
                value: pref_val.clone(),
                basis,
                source: source.clone(),
                updated: timestamp.to_string(),
                scope: specific_scope.clone(),
            };
            if let Ok(valid) = validate_entry(entry) {
                signals.entries.push(valid);
            }
            // If it was a project-specific preference, also record in Preferences with the narrow scope
            // so both project lookup and preference lookup find it with its proper non-global scope.
            if detected_project.is_some() && basis == KnowledgeBasis::Explicit {
                let scoped_pref = KnowledgeEntry {
                    category: KnowledgeCategory::Preferences,
                    title: pref_title,
                    value: pref_val,
                    basis,
                    source,
                    updated: timestamp.to_string(),
                    scope: specific_scope,
                };
                if let Ok(valid) = validate_entry(scoped_pref) {
                    signals.entries.push(valid);
                }
            }
        }
    }

    signals
}

fn extract_forget_target(clause: &str, lower: &str) -> Option<String> {
    for prefix in [
        "forget my preference for ",
        "forget my preference about ",
        "forget my preference on ",
        "forget my ",
        "forget that i ",
        "forget about ",
        "delete my preference for ",
        "delete my preference about ",
        "delete my record about ",
        "remove my preference for ",
        "remove my preference about ",
        "don't remember my ",
        "do not remember my ",
    ] {
        if let Some(idx) = lower.find(prefix) {
            let start = idx + prefix.len();
            let target = clause[start..].trim().trim_matches(['.', '!', '?', '"', '\'']);
            if !target.is_empty() {
                return Some(target.to_string());
            }
        }
    }
    None
}

fn strip_correction_prefix(clause: &str) -> (&str, bool) {
    let lower = clause.to_ascii_lowercase();
    for prefix in [
        "correction:",
        "correction -",
        "actually,",
        "actually ",
        "instead,",
        "update:",
        "to clarify,",
    ] {
        if lower.starts_with(prefix) {
            return (clause[prefix.len()..].trim(), true);
        }
    }
    (clause, false)
}

fn is_tentative_clause(lower: &str) -> bool {
    for marker in [
        "maybe ",
        "perhaps ",
        "i might ",
        "might prefer ",
        "not sure, but ",
        "tentatively ",
        "for now maybe ",
        "sometimes i ",
    ] {
        if lower.contains(marker) {
            return true;
        }
    }
    false
}

fn detect_clause_scope(clause: &str, lower: &str) -> (String, Option<String>) {
    // Check explicit "for project <X>" or "in project <X>"
    for marker in ["for project ", "in project ", "on project ", "project "] {
        if let Some(idx) = lower.find(marker) {
            let after = &clause[idx + marker.len()..];
            let token: String = after
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
                .next()
                .unwrap_or("")
                .to_string();
            if !token.is_empty() {
                if let Ok(slug) = validate_project_slug(&token) {
                    return (format!("project:{}", slug), Some(slug));
                }
            }
        }
    }

    // Check specific component/project scopes (e.g., "alloyOS boot screen", "boot screen", "alloyOS")
    if lower.contains("boot screen") || lower.contains("boot-screen") || lower.contains("splash") {
        if lower.contains("alloyos") {
            return ("alloyOS boot screen".to_string(), Some("alloyos".to_string()));
        }
        return ("alloyOS boot screen".to_string(), None);
    }
    if lower.contains("for alloyos") || lower.contains("in alloyos") || lower.contains("on alloyos") {
        return ("alloyOS".to_string(), Some("alloyos".to_string()));
    }
    if lower.contains("dashboard") {
        return ("Ferrofluid dashboard".to_string(), None);
    }
    if lower.contains("keyboard") {
        return ("soft keyboard".to_string(), None);
    }

    ("global".to_string(), None)
}

fn extract_name_fact(clause: &str, lower: &str) -> Option<String> {
    for prefix in [
        "my name is ",
        "call me ",
        "you can call me ",
        "please call me ",
    ] {
        if let Some(idx) = lower.find(prefix) {
            let after = clause[idx + prefix.len()..].trim();
            let after_lower = after.to_ascii_lowercase();
            let stop = [" and ", " but ", ",", "."]
                .into_iter()
                .filter_map(|delim| after_lower.find(delim))
                .min()
                .unwrap_or(after.len());
            let candidate = after[..stop].trim().trim_matches(['"', '\'']);
            if !candidate.is_empty() && candidate.len() <= 64 {
                return Some(candidate.to_string());
            }
        }
    }
    None
}

fn extract_profile_fact(clause: &str, lower: &str) -> Option<(String, String)> {
    for (prefix, title) in [
        ("my role is ", "Role"),
        ("i work as ", "Role"),
        ("i am a ", "Role"),
        ("i'm a ", "Role"),
        ("i am an ", "Role"),
        ("i'm an ", "Role"),
        ("my timezone is ", "Timezone"),
        ("my time zone is ", "Timezone"),
        ("i live in ", "Location"),
        ("i am based in ", "Location"),
        ("i'm based in ", "Location"),
        ("my primary language is ", "Primary language"),
    ] {
        if let Some(idx) = lower.find(prefix) {
            let after = clause[idx + prefix.len()..].trim();
            if !after.is_empty() && after.len() <= 160 {
                return Some((title.to_string(), after.to_string()));
            }
        }
    }
    None
}

fn extract_interest_or_goal(clause: &str, lower: &str) -> Option<(String, String)> {
    for (prefix, title) in [
        ("i am interested in ", "Interest"),
        ("i'm interested in ", "Interest"),
        ("my interest is ", "Interest"),
        ("my goal is ", "Ongoing goal"),
        ("my main goal is ", "Ongoing goal"),
        ("i want to build ", "Ongoing goal"),
        ("i am working on ", "Active focus"),
        ("i'm working on ", "Active focus"),
    ] {
        if let Some(idx) = lower.find(prefix) {
            let after = clause[idx + prefix.len()..].trim();
            if !after.is_empty() && after.len() <= 200 {
                return Some((title.to_string(), after.to_string()));
            }
        }
    }
    None
}

fn extract_preference(
    clause: &str,
    lower: &str,
    default_scope: &str,
) -> Option<(String, String, String)> {
    // Visual / boot-screen / theme preferences
    if lower.contains("boot screen") || lower.contains("boot-screen") {
        if lower.contains("black-and-white")
            || lower.contains("black and white")
            || lower.contains("monochrome")
            || lower.contains("minimal")
        {
            return Some((
                "Visual style".to_string(),
                 format!("{}.", clause.trim_end_matches('.')),
                "alloyOS boot screen".to_string(),
            ));
        }
    }

    for prefix in [
        "i always prefer ",
        "i prefer ",
        "i'd prefer ",
        "i would prefer ",
        "my preference is ",
        "please always ",
        "always use ",
        "keep answers ",
        "keep responses ",
        "i like my answers ",
        "i do not like ",
        "i don't like ",
        "i dislike ",
    ] {
        if let Some(idx) = lower.find(prefix) {
            let after = clause[idx + prefix.len()..].trim();
            if after.is_empty() {
                continue;
            }
            let title = classify_preference_title(&after.to_ascii_lowercase());
            let scope = if default_scope != "global" {
                default_scope.to_string()
            } else if lower.contains("for this ") || lower.contains("right now") || lower.contains("just this once") {
                "current task".to_string()
            } else {
                "global".to_string()
            };
            // A one-off task-only preference ("for this request only") is not a durable preference
            if scope == "current task" {
                return None;
            }
            let val = if prefix.contains("not like") || prefix.contains("don't like") || prefix.contains("dislike") {
                format!("Avoid {}", after.trim_end_matches('.'))
            } else if prefix.starts_with("keep answers") || prefix.starts_with("keep responses") {
                format!("Keep responses {}", after.trim_end_matches('.'))
            } else {
                after.trim_end_matches('.').to_string()
            };
            return Some((title, val, scope));
        }
    }

    None
}

fn classify_preference_title(lower_val: &str) -> String {
    if lower_val.contains("concise")
        || lower_val.contains("brief")
        || lower_val.contains("short")
        || lower_val.contains("detailed")
        || lower_val.contains("verbose")
        || lower_val.contains("tone")
        || lower_val.contains("response")
        || lower_val.contains("answer")
        || lower_val.contains("bullet")
    {
        "Communication style".to_string()
    } else if lower_val.contains("dark")
        || lower_val.contains("light")
        || lower_val.contains("monochrome")
        || lower_val.contains("black-and-white")
        || lower_val.contains("black and white")
        || lower_val.contains("minimal")
        || lower_val.contains("color")
        || lower_val.contains("theme")
        || lower_val.contains("ui")
        || lower_val.contains("visual")
        || lower_val.contains("boot")
    {
        "Visual style".to_string()
    } else {
        "Workflow preference".to_string()
    }
}

/// Builds a declarative `AppTile` (`knowledge.profile`) so Ferrofluid can display what was learned,
/// show storage durability (`CobaltFS` vs `RamFS`), and let the user pause/resume or inspect learning.
pub fn build_knowledge_tile(
    entries: &[KnowledgeEntry],
    learning_paused: bool,
    storage_kind: KnowledgeStorageKind,
) -> AppTile {
    let status_label = if learning_paused {
        "learning paused"
    } else {
        "learning active"
    };
    let subtitle = format!(
        "{} record(s) · {} · {}",
        entries.len(),
        status_label,
        storage_kind.durability_notice()
    );

    let mut items: Vec<TileItem> = entries
        .iter()
        .take(6)
        .enumerate()
        .map(|(idx, e)| TileItem {
            id: format!("k_{}", idx + 1),
            title: format!("{}: {}", e.title, e.value),
            subtitle: format!("{} · scope: {}", e.category.path(), e.scope),
            badge: e.basis.as_code().to_string(),
            status: e.updated.clone(),
        })
        .collect();

    if items.is_empty() {
        items.push(TileItem {
            id: "empty".to_string(),
            title: "No user knowledge recorded yet".to_string(),
            subtitle: format!("Storage: {}", storage_kind.durability_notice()),
            badge: "empty".to_string(),
            status: status_label.to_string(),
        });
    }

    let toggle_action = if learning_paused {
        TileAction {
            id: "resume_learning".to_string(),
            label: "Resume Learning".to_string(),
            tool_name: "knowledge.resume".to_string(),
            default_args: BTreeMap::new(),
        }
    } else {
        TileAction {
            id: "pause_learning".to_string(),
            label: "Pause Learning".to_string(),
            tool_name: "knowledge.pause".to_string(),
            default_args: BTreeMap::new(),
        }
    };

    let mut inspect_args = BTreeMap::new();
    inspect_args.insert("query".to_string(), McpValue::Text("*".to_string()));
    let inspect_action = TileAction {
        id: "inspect_knowledge".to_string(),
        label: "Inspect Knowledge".to_string(),
        tool_name: "knowledge_search".to_string(),
        default_args: inspect_args,
    };

    AppTile {
        tile_id: KNOWLEDGE_TILE_ID.to_string(),
        title: "User Knowledge".to_string(),
        subtitle,
        refresh_resource_uri: Some("knowledge://profile".to_string()),
        items,
        actions: vec![inspect_action, toggle_action],
    }
}
