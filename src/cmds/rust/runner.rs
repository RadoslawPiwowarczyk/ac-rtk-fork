//! Runs arbitrary commands and captures only stderr or test failures.
//! ACOUSTIC-002: Uses direct execution (no shell) to prevent injection.

use crate::core::sanitize;
use crate::core::tracking;
use anyhow::{Context, Result};
use regex::Regex;
use std::process::{Command, Stdio};

/// Run a command and filter output to show only errors/warnings.
/// Takes args as a slice — no shell involved.
pub fn run_err(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let display_cmd = args.join(" ");

    if verbose > 0 {
        eprintln!("Running: {}", display_cmd);
    }

    // ACOUSTIC-002: Validate no shell metacharacters
    if let Err(msg) = sanitize::validate_args(args) {
        eprintln!("[rtk err] {}", msg);
        return Ok(1);
    }

    let (bin, rest) = args.split_first().unwrap(); // safe: validate_args checks non-empty

    let output = Command::new(bin)
        .args(rest)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("Failed to execute: {}", bin))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);
    let filtered = filter_errors(&raw);
    let mut rtk = String::new();

    if filtered.is_empty() {
        if output.status.success() {
            rtk.push_str("[ok] Command completed successfully (no errors)");
        } else {
            rtk.push_str(&format!(
                "[FAIL] Command failed (exit code: {:?})\n",
                output.status.code()
            ));
            let lines: Vec<&str> = raw.lines().collect();
            for line in lines.iter().rev().take(10).rev() {
                rtk.push_str(&format!("  {}\n", line));
            }
        }
    } else {
        rtk.push_str(&filtered);
    }

    let exit_code = crate::core::utils::exit_code_from_output(&output, "err");
    if let Some(hint) = crate::core::tee::tee_and_hint(&raw, "err", exit_code) {
        println!("{}\n{}", rtk, hint);
    } else {
        println!("{}", rtk);
    }
    timer.track(&display_cmd, "rtk run-err", &raw, &rtk);
    Ok(exit_code)
}

/// Flags that indicate a diagnostic/info invocation, not a test execution.
const DIAGNOSTIC_FLAGS: &[&str] = &[
    "--listTests",
    "--showConfig",
    "--help",
    "--version",
    "-h",
    "-V",
];

/// Run tests and show only failures.
/// Takes args as a slice — no shell involved.
pub fn run_test(args: &[String], verbose: u8) -> Result<i32> {
    // ACOUSTIC-013: Diagnostic flags bypass test filter — run unfiltered
    if args.iter().any(|a| DIAGNOSTIC_FLAGS.contains(&a.as_str())) {
        return run_passthrough(args, verbose, "test");
    }

    let timer = tracking::TimedExecution::start();
    let display_cmd = args.join(" ");

    if verbose > 0 {
        eprintln!("Running tests: {}", display_cmd);
    }

    // ACOUSTIC-002: Validate no shell metacharacters
    if let Err(msg) = sanitize::validate_args(args) {
        eprintln!("[rtk test] {}", msg);
        return Ok(1);
    }

    let (bin, rest) = args.split_first().unwrap(); // safe: validate_args checks non-empty

    let output = Command::new(bin)
        .args(rest)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("Failed to execute: {}", bin))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = crate::core::utils::exit_code_from_output(&output, "test");
    let summary = extract_test_summary(&raw, &display_cmd);
    if let Some(hint) = crate::core::tee::tee_and_hint(&raw, "test", exit_code) {
        println!("{}\n{}", summary, hint);
    } else {
        println!("{}", summary);
    }
    timer.track(&display_cmd, "rtk run-test", &raw, &summary);
    Ok(exit_code)
}

// --- Everything below here is UNCHANGED from upstream ---

fn filter_errors(output: &str) -> String {
    lazy_static::lazy_static! {
        static ref ERROR_PATTERNS: Vec<Regex> = vec![
            // Generic errors
            Regex::new(r"(?i)^.*error[\s:\[].*$").unwrap(),
            Regex::new(r"(?i)^.*\berr\b.*$").unwrap(),
            Regex::new(r"(?i)^.*warning[\s:\[].*$").unwrap(),
            Regex::new(r"(?i)^.*\bwarn\b.*$").unwrap(),
            Regex::new(r"(?i)^.*failed.*$").unwrap(),
            Regex::new(r"(?i)^.*failure.*$").unwrap(),
            Regex::new(r"(?i)^.*exception.*$").unwrap(),
            Regex::new(r"(?i)^.*panic.*$").unwrap(),
            // Rust specific
            Regex::new(r"^error\[E\d+\]:.*$").unwrap(),
            Regex::new(r"^\s*--> .*:\d+:\d+$").unwrap(),
            // Python
            Regex::new(r"^Traceback.*$").unwrap(),
            Regex::new(r#"^\s*File ".*", line \d+.*$"#).unwrap(),
            // JavaScript/TypeScript
            Regex::new(r"^\s*at .*:\d+:\d+.*$").unwrap(),
            // Go
            Regex::new(r"^.*\.go:\d+:.*$").unwrap(),
        ];
    }

    let mut result = Vec::new();
    let mut in_error_block = false;
    let mut blank_count = 0;

    for line in output.lines() {
        let is_error_line = ERROR_PATTERNS.iter().any(|p| p.is_match(line));

        if is_error_line {
            in_error_block = true;
            blank_count = 0;
            result.push(line.to_string());
        } else if in_error_block {
            if line.trim().is_empty() {
                blank_count += 1;
                if blank_count >= 2 {
                    in_error_block = false;
                } else {
                    result.push(line.to_string());
                }
            } else if line.starts_with(' ') || line.starts_with('\t') {
                result.push(line.to_string());
                blank_count = 0;
            } else {
                in_error_block = false;
            }
        }
    }

    result.join("\n")
}

fn extract_test_summary(output: &str, command: &str) -> String {
    let mut result = Vec::new();
    let lines: Vec<&str> = output.lines().collect();
    let is_cargo = command.contains("cargo test");
    let is_pytest = command.contains("pytest");
    let is_jest =
        command.contains("jest") || command.contains("npm test") || command.contains("yarn test")
            || command.contains("pnpm test");
    let is_go = command.contains("go test");
    let mut failures = Vec::new();
    let mut in_failure = false;
    let mut failure_lines = Vec::new();
    for line in lines.iter() {
        if is_cargo {
            if line.contains("test result:") {
                result.push(line.to_string());
            }
            if line.contains("FAILED") && !line.contains("test result") {
                failures.push(line.to_string());
            }
            if line.starts_with("failures:") {
                in_failure = true;
            }
            if in_failure && line.starts_with("    ") {
                failure_lines.push(line.to_string());
            }
        }
        if is_pytest {
            if line.contains(" passed") || line.contains(" failed") || line.contains(" error") {
                result.push(line.to_string());
            }
            if line.contains("FAILED") {
                failures.push(line.to_string());
            }
        }
        if is_jest {
            if line.contains("Tests:") || line.contains("Test Suites:") {
                result.push(line.to_string());
            }
            // FAIL file header
            if line.contains("FAIL") && !line.contains("Test Suites:") {
                failures.push(line.to_string());
            }
            // ACOUSTIC-012: Capture jest failure detail blocks
            // Jest outputs: "  ● Suite Name › test name" then indented diff/stack
            if line.trim_start().starts_with("●") {
                in_failure = true;
                failure_lines.push(line.to_string());
            } else if in_failure {
                if line.starts_with("    ") || line.starts_with("\t") || line.trim().is_empty() {
                    failure_lines.push(line.to_string());
                } else {
                    in_failure = false;
                }
            }
        }
        if is_go {
            if line.starts_with("ok") || line.starts_with("FAIL") || line.starts_with("---") {
                result.push(line.to_string());
            }
            if line.contains("FAIL") {
                failures.push(line.to_string());
            }
        }
    }
    let mut output = String::new();
    if !failures.is_empty() {
        output.push_str("[FAIL] FAILURES:\n");
        for f in failures.iter().take(10) {
            output.push_str(&format!("  {}\n", f));
        }
        if failures.len() > 10 {
            output.push_str(&format!("  ... +{} more failures\n", failures.len() - 10));
        }
        // ACOUSTIC-012: Append failure details (assertion diffs, stack traces)
        if !failure_lines.is_empty() {
            output.push('\n');
            for line in failure_lines.iter().take(50) {
                output.push_str(&format!("{}\n", line));
            }
            if failure_lines.len() > 50 {
                output.push_str(&format!(
                    "  ... +{} more detail lines truncated\n",
                    failure_lines.len() - 50
                ));
            }
        }
        output.push('\n');
    }
    if !result.is_empty() {
        output.push_str("SUMMARY:\n");
        for r in &result {
            output.push_str(&format!("  {}\n", r));
        }
    } else {
        output.push_str("OUTPUT (last 5 lines):\n");
        let start = lines.len().saturating_sub(5);
        for line in &lines[start..] {
            if !line.trim().is_empty() {
                output.push_str(&format!("  {}\n", line));
            }
        }
    }
    output
}

// Run a command as a transparent proxy — no filtering, just tracking.
fn run_passthrough(args: &[String], verbose: u8, label: &str) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let display_cmd = args.join(" ");

    if verbose > 0 {
        eprintln!("Running (passthrough): {}", display_cmd);
    }

    if let Err(msg) = sanitize::validate_args(args) {
        eprintln!("[rtk {}] {}", label, msg);
        return Ok(1);
    }

    let (bin, rest) = args.split_first().unwrap();

    let output = Command::new(bin)
        .args(rest)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("Failed to execute: {}", bin))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = crate::core::utils::exit_code_from_output(&output, label);
    if let Some(hint) = crate::core::tee::tee_and_hint(&raw, label, exit_code) {
        println!("{}\n{}", raw.trim(), hint);
    } else {
        println!("{}", raw.trim());
    }
    timer.track(&display_cmd, &format!("rtk run-{}-passthrough", label), &raw, &raw);
    Ok(exit_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_errors() {
        let output = "info: compiling\nerror: something failed\n  at line 10\ninfo: done";
        let filtered = filter_errors(output);
        assert!(filtered.contains("error"));
        assert!(!filtered.contains("info"));
    }
}