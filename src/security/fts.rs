use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct FtsOptions {
    pub max_query_len: usize,
    pub max_terms: usize,
    pub min_prefix_len: usize,
}

impl Default for FtsOptions {
    fn default() -> Self {
        Self {
            max_query_len: 1024,
            max_terms: 24,
            min_prefix_len: 3,
        }
    }
}

/// Escapes and formats user search input for SQLite FTS5 safely.
///
/// Features:
/// - Splitting of camelCase and PascalCase identifiers (e.g., `DatabaseMigration` -> `Database`, `Migration`).
/// - Strict double-quote escaping to prevent FTS5 syntax injection.
/// - Bound max query length and max number of terms to prevent AST blowup.
/// - Restrict `*` wildcard to terms of length >= `min_prefix_len` to prevent prefix scan DoS.
pub fn escape_fts5_query(query: &str, opts: &FtsOptions) -> Option<String> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return None;
    }

    let bounded = if trimmed.len() > opts.max_query_len {
        let mut end = opts.max_query_len;
        while !trimmed.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        &trimmed[..end]
    } else {
        trimmed
    };

    let mut terms: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // First, split into raw tokens on non-alphanumeric (except underscore)
    for raw in bounded.split(|c: char| !c.is_alphanumeric() && c != '_') {
        let raw_token = raw.trim();
        if raw_token.is_empty() {
            continue;
        }

        // Deconstruct camelCase/PascalCase sub-tokens
        let sub_tokens = split_camel_case(raw_token);
        for token in sub_tokens {
            let lower = token.to_lowercase();
            if !seen.insert(lower) {
                continue;
            }

            let escaped = token.replace('"', "\"\"");
            let formatted = if token.chars().count() >= opts.min_prefix_len {
                format!("\"{escaped}\"*")
            } else {
                format!("\"{escaped}\"")
            };
            terms.push(formatted);
            if terms.len() >= opts.max_terms {
                break;
            }
        }
        if terms.len() >= opts.max_terms {
            break;
        }
    }

    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

/// Splits camelCase and PascalCase strings into constituent words.
/// E.g. "LockTimeoutException" -> ["LockTimeoutException", "Lock", "Timeout", "Exception"]
fn split_camel_case(token: &str) -> Vec<String> {
    let mut words = Vec::new();
    let chars: Vec<char> = token.chars().collect();
    if chars.is_empty() {
        return words;
    }

    // Always include the full token itself first
    words.push(token.to_string());

    // If token contains underscore, split on underscore too
    if token.contains('_') {
        for part in token.split('_') {
            let p = part.trim();
            if !p.is_empty() && p != token {
                words.push(p.to_string());
            }
        }
        return words;
    }

    let mut start = 0;
    for i in 1..chars.len() {
        // Transition from lowercase to uppercase or sequence of uppercase followed by lowercase
        if (chars[i].is_uppercase() && chars[i - 1].is_lowercase())
            || (i + 1 < chars.len()
                && chars[i - 1].is_uppercase()
                && chars[i].is_uppercase()
                && chars[i + 1].is_lowercase())
        {
            let word: String = chars[start..i].iter().collect();
            if word.len() > 1 && word != token {
                words.push(word);
            }
            start = i;
        }
    }
    let last_word: String = chars[start..].iter().collect();
    if last_word.len() > 1 && last_word != token {
        words.push(last_word);
    }

    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_and_splits_camel_case() {
        let opts = FtsOptions::default();
        let query = escape_fts5_query("PostgresMigrationError \"hello world\"", &opts).unwrap();
        assert!(query.contains("\"PostgresMigrationError\"*"));
        assert!(query.contains("\"Postgres\"*"));
        assert!(query.contains("\"Migration\"*"));
        assert!(query.contains("\"Error\"*"));
        assert!(query.contains("\"hello\"*"));
        assert!(query.contains("\"world\"*"));
    }

    #[test]
    fn prevents_short_prefix_wildcard_explosion() {
        let opts = FtsOptions::default();
        let query = escape_fts5_query("a to the port", &opts).unwrap();
        // 'a' has length 1 (< 3), so it must NOT have wildcard *
        assert!(query.contains("\"a\""));
        assert!(!query.contains("\"a\"*"));
        // 'to' has length 2 (< 3), so no wildcard *
        assert!(query.contains("\"to\""));
        assert!(!query.contains("\"to\"*"));
        // 'the' and 'port' have length >= 3, so they get *
        assert!(query.contains("\"the\"*"));
        assert!(query.contains("\"port\"*"));
    }
}
