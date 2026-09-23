use regex::Regex;
use std::sync::OnceLock;

pub struct RedactionRule {
    pub name: &'static str,
    pub regex: Regex,
    pub replacement: &'static str,
}

static RULES: OnceLock<Vec<RedactionRule>> = OnceLock::new();

fn get_redaction_rules() -> &'static Vec<RedactionRule> {
    RULES.get_or_init(|| {
        vec![
            // Cryptographic PEM private keys
            RedactionRule {
                name: "PRIVATE_KEY",
                regex: Regex::new(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z0-9 ]*PRIVATE KEY-----").unwrap(),
                replacement: "[REDACTED_PRIVATE_KEY]",
            },
            // JSON Web Tokens (JWT)
            RedactionRule {
                name: "JWT",
                regex: Regex::new(r"\beyJ[A-Za-z0-9_\-]{10,}\.eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\b").unwrap(),
                replacement: "[REDACTED_JWT]",
            },
            // AWS Access Key ID
            RedactionRule {
                name: "AWS_ACCESS_KEY",
                regex: Regex::new(r"\b(AKIA|ABIA|ACCA|ASIA)[0-9A-Z]{16}\b").unwrap(),
                replacement: "[REDACTED_AWS_ACCESS_KEY]",
            },
            // AWS Secret Access Key
            RedactionRule {
                name: "AWS_SECRET_KEY",
                regex: Regex::new(r#"(?i)\b(aws_secret_access_key|secret_access_key)\s*[:=]\s*["']?([A-Za-z0-9/+=]{40})["']?"#).unwrap(),
                replacement: "${1}=[REDACTED_AWS_SECRET]",
            },
            // GitHub Personal Access Tokens
            RedactionRule {
                name: "GITHUB_TOKEN",
                regex: Regex::new(r"\b(gh[pousr]_[A-Za-z0-9]{36}|github_pat_[A-Za-z0-9_]{82})\b").unwrap(),
                replacement: "[REDACTED_GITHUB_TOKEN]",
            },
            // GitLab Personal Access Tokens
            RedactionRule {
                name: "GITLAB_TOKEN",
                regex: Regex::new(r"\bglpat-[A-Za-z0-9\-_=]{20,}\b").unwrap(),
                replacement: "[REDACTED_GITLAB_TOKEN]",
            },
            // OpenAI API Keys
            RedactionRule {
                name: "OPENAI_KEY",
                regex: Regex::new(r"\b(sk-[A-Za-z0-9]{20,}|sk-proj-[A-Za-z0-9_\-]{40,})\b").unwrap(),
                replacement: "[REDACTED_OPENAI_KEY]",
            },
            // Anthropic API Keys
            RedactionRule {
                name: "ANTHROPIC_KEY",
                regex: Regex::new(r"\bsk-ant-[A-Za-z0-9_\-]{20,}\b").unwrap(),
                replacement: "[REDACTED_ANTHROPIC_KEY]",
            },
            // Google Cloud API Keys
            RedactionRule {
                name: "GOOGLE_API_KEY",
                regex: Regex::new(r"\bAIza[0-9A-Za-z_\-]{35}\b").unwrap(),
                replacement: "[REDACTED_GOOGLE_API_KEY]",
            },
            // Slack Tokens
            RedactionRule {
                name: "SLACK_TOKEN",
                regex: Regex::new(r"\bxox[baprs]-[0-9a-zA-Z]{10,48}\b").unwrap(),
                replacement: "[REDACTED_SLACK_TOKEN]",
            },
            // HTTP Authorization Bearer Tokens
            RedactionRule {
                name: "BEARER_TOKEN",
                regex: Regex::new(r"(?i)\bBearer\s+[A-Za-z0-9_\-\.\+=]{16,}\b").unwrap(),
                replacement: "Bearer [REDACTED_BEARER_TOKEN]",
            },
            // Database Connection URIs containing embedded passwords
            RedactionRule {
                name: "DB_URI",
                regex: Regex::new(r"(?i)\b(postgres|postgresql|mysql|redis|mongodb|amqp|mssql)://([^:\s]+):([^@\s]+)@").unwrap(),
                replacement: "${1}://${2}:[REDACTED]@",
            },
            // Generic password/secret assignments
            RedactionRule {
                name: "GENERIC_SECRET",
                regex: Regex::new(r#"(?i)\b(password|passwd|secret|api_key|apikey|auth_token|client_secret)\s*[:=]\s*["']?([^\s"';,]{8,})["']?"#).unwrap(),
                replacement: "${1}=[REDACTED_SECRET]",
            },
        ]
    })
}

/// Scans input text and masks all detected credentials with safe redaction markers.
/// Returns the sanitized string and the total count of replaced secrets.
pub fn redact_secrets(input: &str) -> (String, usize) {
    let rules = get_redaction_rules();
    let mut current = input.to_string();
    let mut total_redactions = 0;

    for rule in rules {
        let matches = rule.regex.find_iter(&current).count();
        if matches > 0 {
            total_redactions += matches;
            let replaced = rule
                .regex
                .replace_all(&current, rule.replacement)
                .into_owned();
            current = replaced;
        }
    }

    (current, total_redactions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_openai_and_db_passwords() {
        let raw = "Error connecting to postgres://admin:super_secret_pw@db.prod.internal:5432/main using sk-1234567890abcdef1234567890";
        let (sanitized, count) = redact_secrets(raw);
        assert!(sanitized.contains("postgres://admin:[REDACTED]@db.prod.internal:5432/main"));
        assert!(sanitized.contains("[REDACTED_OPENAI_KEY]"));
        assert_eq!(count, 2);
    }
}
