use crate::error::SecurityError;
use std::collections::HashSet;
use std::io::{self, BufRead};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone)]
pub struct InputLimits {
    pub max_query_len: usize,
    pub max_title_len: usize,
    pub max_context_len: usize,
    pub max_content_len: usize,
    pub max_verification_len: usize,
    pub max_tag_len: usize,
    pub max_tags_count: usize,
    pub max_project_scope_len: usize,
    pub max_line_len: usize,
}

impl Default for InputLimits {
    fn default() -> Self {
        Self {
            max_query_len: 2048,
            max_title_len: 256,
            max_context_len: 64 * 1024,      // 64 KB
            max_content_len: 128 * 1024,     // 128 KB
            max_verification_len: 64 * 1024, // 64 KB
            max_tag_len: 64,
            max_tags_count: 32,
            max_project_scope_len: 128,
            max_line_len: 512 * 1024, // 512 KB per JSON-RPC line
        }
    }
}

pub fn validate_string_bound<'a>(
    field_name: &str,
    val: &'a str,
    max_len: usize,
) -> Result<&'a str, SecurityError> {
    let trimmed = val.trim();
    if trimmed.is_empty() {
        return Err(SecurityError::EmptyField {
            field: field_name.to_owned(),
        });
    }
    if trimmed.len() > max_len {
        return Err(SecurityError::InputTooLong {
            field: field_name.to_owned(),
            max: max_len,
            actual: trimmed.len(),
        });
    }
    Ok(trimmed)
}

pub fn validate_tags(tags: &[String], limits: &InputLimits) -> Result<Vec<String>, SecurityError> {
    if tags.len() > limits.max_tags_count {
        return Err(SecurityError::TooManyTags {
            actual: tags.len(),
            max: limits.max_tags_count,
        });
    }
    let mut seen = HashSet::new();
    let mut cleaned = Vec::with_capacity(tags.len());
    for tag in tags {
        let trimmed = tag.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.len() > limits.max_tag_len {
            return Err(SecurityError::InvalidTag {
                tag: trimmed.to_owned(),
                reason: format!("Tag exceeds maximum length of {} bytes", limits.max_tag_len),
            });
        }
        let lower = trimmed.to_lowercase();
        if seen.insert(lower.clone()) {
            cleaned.push(lower);
        }
    }
    Ok(cleaned)
}

pub fn validate_database_path(raw_path: &Path) -> Result<PathBuf, SecurityError> {
    let path_str = raw_path.to_string_lossy();
    if path_str.trim().is_empty() {
        return Err(SecurityError::InvalidPath(
            "Database path cannot be empty or whitespace".to_owned(),
        ));
    }

    if path_str == ":memory:" {
        return Ok(PathBuf::from(":memory:"));
    }

    if path_str.starts_with("file:") {
        return Err(SecurityError::InvalidPath(
            "SQLite URI paths ('file:...') are not permitted for security reasons".to_owned(),
        ));
    }

    let path = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| SecurityError::InvalidPath(format!("Cannot get current dir: {e}")))?
            .join(raw_path)
    };

    for comp in path.components() {
        if let Component::ParentDir = comp {
            return Err(SecurityError::InvalidPath(
                "Path traversal ('..') is not permitted in database path".to_owned(),
            ));
        }
    }

    let prohibited_prefixes = [
        "/etc",
        "/dev",
        "/proc",
        "/sys",
        "/bin",
        "/sbin",
        "/usr/bin",
        "/usr/sbin",
        "/System",
        "/Library",
        "/private/etc",
        "/var/run",
    ];
    let path_str_canonical = path.to_string_lossy();
    for prefix in prohibited_prefixes {
        if path_str_canonical.starts_with(prefix) {
            return Err(SecurityError::InvalidPath(format!(
                "Database path points to prohibited system location: {prefix}"
            )));
        }
    }

    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if path_str_canonical.starts_with(home_str.as_ref()) {
            let relative = path.strip_prefix(&home).unwrap_or(&path);
            if let Some(first_comp) = relative.components().next() {
                let first = first_comp.as_os_str().to_string_lossy();
                let prohibited_user_dirs = [
                    ".ssh",
                    ".gnupg",
                    ".aws",
                    ".docker",
                    ".bashrc",
                    ".zshrc",
                    ".gitconfig",
                ];
                for prohibited in prohibited_user_dirs {
                    if first == prohibited || first.starts_with(prohibited) {
                        return Err(SecurityError::InvalidPath(format!(
                            "Database path cannot be placed inside sensitive directory '{first}'"
                        )));
                    }
                }
            }
        }
    }

    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext_lower = ext.to_lowercase();
        if !matches!(ext_lower.as_str(), "db" | "sqlite" | "sqlite3") {
            return Err(SecurityError::InvalidPath(format!(
                "Invalid database extension '.{ext}'. Must be '.db', '.sqlite', or '.sqlite3'"
            )));
        }
    } else {
        return Err(SecurityError::InvalidPath(
            "Database path must have a valid extension (.db, .sqlite, .sqlite3)".to_owned(),
        ));
    }

    Ok(path)
}

/// Reads a line from `reader` with a strict byte limit to prevent memory exhaustion DoS.
pub fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    buf: &mut String,
    max_bytes: usize,
) -> io::Result<Option<usize>> {
    buf.clear();
    let mut total_read = 0;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(if total_read == 0 {
                None
            } else {
                Some(total_read)
            });
        }

        let (found_newline, used) = match available.iter().position(|&b| b == b'\n') {
            Some(pos) => (true, pos + 1),
            None => (false, available.len()),
        };

        if total_read + used > max_bytes {
            reader.consume(used);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Input line exceeds maximum allowed limit of {max_bytes} bytes"),
            ));
        }

        let chunk = &available[..used];
        let chunk_str = std::str::from_utf8(chunk)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        buf.push_str(chunk_str);
        reader.consume(used);
        total_read += used;

        if found_newline {
            return Ok(Some(total_read));
        }
    }
}
