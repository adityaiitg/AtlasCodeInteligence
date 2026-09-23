use crate::simd::cosine_similarity;
use crate::types::KnowledgeEntry;
use regex::Regex;
use std::collections::HashSet;
use std::sync::OnceLock;

static UUID_RE: OnceLock<Regex> = OnceLock::new();
static HEX_RE: OnceLock<Regex> = OnceLock::new();
static DATE_RE: OnceLock<Regex> = OnceLock::new();
static PORT_RE: OnceLock<Regex> = OnceLock::new();
static TEMP_PATH_RE: OnceLock<Regex> = OnceLock::new();
static ERROR_SIG_RE: OnceLock<Regex> = OnceLock::new();

fn get_uuid_re() -> &'static Regex {
    UUID_RE.get_or_init(|| {
        Regex::new(r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b").unwrap()
    })
}

fn get_hex_re() -> &'static Regex {
    HEX_RE.get_or_init(|| Regex::new(r"(?i)\b(0x[0-9a-f]{4,}|[0-9a-f]{16,64})\b").unwrap())
}

fn get_date_re() -> &'static Regex {
    DATE_RE.get_or_init(|| {
        Regex::new(r"\b\d{4}-\d{2}-\d{2}(?:T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?)?\b")
            .unwrap()
    })
}

fn get_port_re() -> &'static Regex {
    PORT_RE.get_or_init(|| Regex::new(r":\b\d{4,5}\b").unwrap())
}

fn get_temp_path_re() -> &'static Regex {
    TEMP_PATH_RE.get_or_init(|| Regex::new(r"/(?:tmp|var/folders)/[a-zA-Z0-9_\-\./]+").unwrap())
}

fn get_error_sig_re() -> &'static Regex {
    ERROR_SIG_RE.get_or_init(|| {
        Regex::new(r"(?i)\b(E\d{4}|TS\d{4,5}|CS\d{4}|ORA-\d{5}|SQLSTATE\s+[0-9A-Z]{5}|(?:TypeError|ValueError|KeyError|AttributeError|RuntimeError|NullPointerException|ConnectTimeoutException))\b").unwrap()
    })
}

/// Normalizes error strings and dynamic tokens before generating deduplication keys.
pub fn scrub_dynamic_tokens(text: &str) -> String {
    let mut scrubbed = get_uuid_re().replace_all(text, "").into_owned();
    scrubbed = get_hex_re().replace_all(&scrubbed, "").into_owned();
    scrubbed = get_date_re().replace_all(&scrubbed, "").into_owned();
    scrubbed = get_temp_path_re().replace_all(&scrubbed, "").into_owned();
    scrubbed = get_port_re().replace_all(&scrubbed, "").into_owned();
    scrubbed
}

/// Extracts an explicit error signature if present (e.g. E0382, TS2322).
pub fn extract_error_signature(text: &str) -> Option<String> {
    get_error_sig_re()
        .find(text)
        .map(|m| m.as_str().to_lowercase().replace(' ', ""))
}

/// Generates a canonical deduplication key from project scope and title.
/// Ensures that volatile dynamic values (UUIDs, timestamps, temp paths) don't create false splits.
pub fn generate_dedup_key(project_scope: Option<&str>, title: &str) -> String {
    let scope = project_scope
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("global");

    let scrubbed = scrub_dynamic_tokens(title);
    let effective_title = if scrubbed.trim().is_empty() {
        title
    } else {
        &scrubbed
    };

    let normalized_title = effective_title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>();

    let mut cleaned_title = String::with_capacity(normalized_title.len());
    let mut prev_dash = false;
    for c in normalized_title.chars() {
        if c == '-' {
            if !prev_dash && !cleaned_title.is_empty() {
                cleaned_title.push('-');
                prev_dash = true;
            }
        } else {
            cleaned_title.push(c);
            prev_dash = false;
        }
    }
    let cleaned_title = cleaned_title.trim_end_matches('-').trim_start_matches('-');
    format!("{scope}:{cleaned_title}")
}

/// Evaluates whether a new entry is a near-duplicate of an existing entry based on cosine similarity.
pub fn find_near_duplicate<'a>(
    query_vec: &[f32],
    entries: &'a [(KnowledgeEntry, Vec<f32>)],
    project_scope: Option<&str>,
    threshold: f64,
) -> Option<&'a KnowledgeEntry> {
    if query_vec.is_empty() {
        return None;
    }

    for (entry, vec) in entries {
        if vec.is_empty() {
            continue;
        }
        if entry.project_scope.as_deref() != project_scope {
            continue;
        }

        let sim = cosine_similarity(query_vec, vec);
        if sim >= threshold {
            return Some(entry);
        }
    }

    None
}

/// Merges an incoming entry into an existing entry non-destructively.
/// - Performs a union of tags (preserving all unique tags).
/// - Appends new verification evidence if distinct from existing evidence.
/// - Updates content and context to the newest resolution.
/// - Retains the original entry's ID and creation timestamp.
#[allow(clippy::too_many_arguments)]
pub fn smart_merge_entry(
    existing: &KnowledgeEntry,
    new_title: &str,
    new_kind: &str,
    new_context: &str,
    new_content: &str,
    new_verification: &str,
    new_tags: &[String],
    new_project_scope: Option<&str>,
    new_dedup_key: Option<&str>,
) -> KnowledgeEntry {
    // 1. Tag set union
    let mut seen_tags = HashSet::new();
    let mut merged_tags = Vec::new();

    for tag in &existing.tags {
        let lower = tag.to_lowercase();
        if seen_tags.insert(lower) {
            merged_tags.push(tag.clone());
        }
    }
    for tag in new_tags {
        let lower = tag.to_lowercase();
        if seen_tags.insert(lower) {
            merged_tags.push(tag.clone());
        }
    }

    // 2. Verification evidence aggregation
    let merged_verification = if existing.verification.trim() == new_verification.trim()
        || new_verification.trim().is_empty()
    {
        existing.verification.clone()
    } else if existing.verification.trim().is_empty() {
        new_verification.to_string()
    } else if existing.verification.contains(new_verification.trim()) {
        existing.verification.clone()
    } else {
        format!(
            "{}\n---\nAdditional verification: {}",
            existing.verification.trim(),
            new_verification.trim()
        )
    };

    let now = chrono::Utc::now().to_rfc3339();

    KnowledgeEntry {
        id: existing.id.clone(),
        title: if new_title.trim().is_empty() {
            existing.title.clone()
        } else {
            new_title.to_string()
        },
        kind: new_kind.to_string(),
        context: new_context.to_string(),
        content: new_content.to_string(),
        verification: merged_verification,
        tags: merged_tags,
        project_scope: new_project_scope
            .map(str::to_owned)
            .or_else(|| existing.project_scope.clone()),
        created_at: existing.created_at.clone(),
        updated_at: now,
        dedup_key: new_dedup_key
            .map(str::to_owned)
            .or_else(|| existing.dedup_key.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs_dynamic_uuids_and_temp_paths_for_dedup() {
        let title1 = "Fix crash in /tmp/run-a8b23c91-4d1e-436f-9988-123456789abc/app on port :8080";
        let key1 = generate_dedup_key(Some("backend"), title1);
        let title2 = "Fix crash in /tmp/run-f47ac10b-58cc-4372-a567-0e02b2c3d479/app on port :9090";
        let key2 = generate_dedup_key(Some("backend"), title2);
        assert_eq!(key1, key2);
    }

    #[test]
    fn smart_merge_unions_tags_and_appends_verification() {
        let existing = KnowledgeEntry {
            id: "uuid-1".to_string(),
            title: "Original Title".to_string(),
            kind: "resolved_failure".to_string(),
            context: "ctx1".to_string(),
            content: "sol1".to_string(),
            verification: "test passed in staging".to_string(),
            tags: vec!["rust".to_string(), "db".to_string()],
            project_scope: Some("proj".to_string()),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            dedup_key: Some("proj:original-title".to_string()),
        };

        let merged = smart_merge_entry(
            &existing,
            "Updated Title",
            "workflow",
            "new ctx",
            "new sol",
            "test passed in prod",
            &["db".to_string(), "performance".to_string()],
            Some("proj"),
            None,
        );

        assert_eq!(merged.id, "uuid-1");
        assert_eq!(merged.created_at, "2026-01-01T00:00:00Z");
        assert_eq!(merged.tags, vec!["rust", "db", "performance"]);
        assert!(merged.verification.contains("test passed in staging"));
        assert!(merged.verification.contains("test passed in prod"));
    }
}
