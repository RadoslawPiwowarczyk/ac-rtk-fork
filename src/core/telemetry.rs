//! Telemetry module — DISABLED (ACOUSTIC-001)
//!
//! Upstream RTK sends daily anonymous usage metrics via ureq.
//! This has been stripped at compile time for enterprise deployment.
//! See AUDIT-LOG.md and ACOUSTIC-CHANGES.md for details.

/// No-op. Telemetry stripped for Acoustic deployment.
pub fn maybe_ping() {
    // Intentionally empty — no network calls, no file writes, no tracking.
}