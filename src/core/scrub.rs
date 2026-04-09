//! ACOUSTIC-004: Scrub sensitive values from command strings before DB persistence.

use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    static ref SCRUB_PATTERNS: Vec<(Regex, &'static str)> = vec![
        // Bearer tokens in headers
        (Regex::new(r"(?i)(Authorization:\s*Bearer\s+)\S+").unwrap(), "${1}[REDACTED]"),
        // API key headers
        (Regex::new(r"(?i)(X-Api-Key:\s*)\S+").unwrap(), "${1}[REDACTED]"),
        // Basic auth in URLs: https://user:pass@host
        (Regex::new(r"(https?://[^:]+:)[^@]+(@)").unwrap(), "${1}[REDACTED]${2}"),
        // curl -u user:password
        (Regex::new(r"(-u\s+\S+:)\S+").unwrap(), "${1}[REDACTED]"),
        // AWS Access Key IDs
        (Regex::new(r"AKIA[A-Z0-9]{16}").unwrap(), "[AWS_KEY_REDACTED]"),
        // AWS secret keys by name
        (Regex::new(r"(?i)(aws_secret_access_key[=:\s]+)[A-Za-z0-9/+=]{20,}").unwrap(), "${1}[REDACTED]"),
        // Connection strings with credentials
        (Regex::new(r"(?i)((?:postgres|mongodb|mysql|redis|amqp)(?:\+\w+)?://[^:]+:)[^@]+(@)").unwrap(), "${1}[REDACTED]${2}"),
        // Env var assignments with sensitive names
        (Regex::new(r"(?i)((?:DATABASE_URL|DB_PASSWORD|API_KEY|SECRET_KEY|AUTH_TOKEN|ACCESS_TOKEN|PRIVATE_KEY|AWS_SECRET_ACCESS_KEY|AWS_SESSION_TOKEN|GITHUB_TOKEN|NPM_TOKEN|SLACK_TOKEN|OPENAI_API_KEY|ANTHROPIC_API_KEY)=)\S+").unwrap(), "${1}[REDACTED]"),
        // JWT tokens (eyJ pattern)
        (Regex::new(r"eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}").unwrap(), "[JWT_REDACTED]"),
    ];
}

/// Scrub potentially sensitive values from a command string.
pub fn scrub_command(cmd: &str) -> String {
    let mut result = cmd.to_string();
    for (pattern, replacement) in SCRUB_PATTERNS.iter() {
        result = pattern.replace_all(&result, *replacement).to_string();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Should NOT be scrubbed ---

    #[test]
    fn test_normal_git_unchanged() {
        let cmd = "git commit -m 'fix API endpoint'";
        assert_eq!(scrub_command(cmd), cmd);
    }

    #[test]
    fn test_cargo_test_unchanged() {
        let cmd = "cargo test --release --test-threads=4";
        assert_eq!(scrub_command(cmd), cmd);
    }

    #[test]
    fn test_grep_pattern_unchanged() {
        let cmd = "grep -rn 'password' src/";
        assert_eq!(scrub_command(cmd), cmd);
    }

    #[test]
    fn test_url_without_creds_unchanged() {
        let cmd = "curl https://api.example.com/v1/users";
        assert_eq!(scrub_command(cmd), cmd);
    }

    // --- Should BE scrubbed ---

    #[test]
    fn test_bearer_token() {
        let cmd = r#"curl -H "Authorization: Bearer sk-abc123def456""#;
        let result = scrub_command(cmd);
        assert!(!result.contains("sk-abc123def456"));
        assert!(result.contains("[REDACTED]"));
    }

    #[test]
    fn test_basic_auth_url() {
        let cmd = "curl https://admin:supersecret@api.example.com/data";
        let result = scrub_command(cmd);
        assert!(!result.contains("supersecret"));
        assert!(result.contains("@api.example.com"));
    }

    #[test]
    fn test_aws_key() {
        let cmd = "aws s3 ls AKIAIOSFODNN7EXAMPLE";
        let result = scrub_command(cmd);
        assert!(!result.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn test_connection_string() {
        // DATABASE_URL= env var pattern fires first and redacts the entire value
        let cmd = "DATABASE_URL=postgres://user:secret@localhost:5432/db cargo test";
        let result = scrub_command(cmd);
        assert!(!result.contains("secret"));
        assert!(result.contains("DATABASE_URL=[REDACTED]"));
    }

    #[test]
    fn test_jwt() {
        let cmd = "curl -H 'Auth: eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U'";
        let result = scrub_command(cmd);
        assert!(!result.contains("eyJhbGciOiJIUzI1NiJ9"));
    }

    #[test]
    fn test_env_var_github_token() {
        let cmd = "GITHUB_TOKEN=ghp_abc123xyz cargo publish";
        let result = scrub_command(cmd);
        assert!(!result.contains("ghp_abc123xyz"));
        assert!(result.contains("GITHUB_TOKEN=[REDACTED]"));
    }
}
