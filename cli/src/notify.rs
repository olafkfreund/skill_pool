//! Desktop notifications for the Phase 4 capturer.
//!
//! Fires a "Skill draft ready" toast after each successful
//! `POST /v1/drafts`. Used by both the single-shot `capture-run` and the
//! long-lived `skill-pool-capturer` daemon.
//!
//! The notification is best-effort: any D-Bus failure (headless box,
//! Wayland sandbox without portal access, libnotify missing) is swallowed
//! so the daemon never crashes on a missing display server. Suppression
//! sources, in priority order:
//!
//!  1. Caller passes `notify_enabled = false` (e.g. `--no-notify`,
//!     `SKILL_POOL_CAPTURE_NO_NOTIFY=1`, or the daemon detecting it's
//!     under systemd without a session bus).
//!  2. On Linux, `should_emit()` requires `DBUS_SESSION_BUS_ADDRESS` to
//!     be set — daemons launched by systemd `Type=simple` without a
//!     user session see no bus and would emit a libdbus error log.
//!
//! Tests cover the gating logic deterministically; the actual D-Bus
//! call is exercised at runtime only.

/// Build the human-readable notification body. Pulled out so tests can
/// assert on it without spawning a Notification.
pub fn body_for(slug: &str, draft_id: &str, web_url: Option<&str>) -> String {
    match web_url {
        Some(url) => format!(
            "Draft `{slug}` is waiting in the inbox.\n{}/drafts/{draft_id}",
            url.trim_end_matches('/')
        ),
        None => format!("Draft `{slug}` is waiting in the inbox."),
    }
}

/// Decide whether the host environment supports a notification.
///
/// On Linux we require `DBUS_SESSION_BUS_ADDRESS`; without it,
/// `notify_rust::Notification::show()` would log a libdbus error to
/// stderr on every invocation. On other platforms the underlying
/// backend handles its own availability, so we always say yes and let
/// `try_show` swallow any error.
pub fn should_emit() -> bool {
    if !cfg!(target_os = "linux") {
        return true;
    }
    has_session_bus()
}

/// Inspect the env for an active D-Bus session bus address. Pulled out
/// for testability — tests stash and restore the variable.
fn has_session_bus() -> bool {
    std::env::var("DBUS_SESSION_BUS_ADDRESS")
        .ok()
        .filter(|s| !s.is_empty())
        .is_some()
}

/// Fire the toast. `enabled` is the caller's master switch (CLI flag,
/// env, etc.); `should_emit()` is the second gate. Errors are logged at
/// `tracing::debug!` level only — a notification failure must never
/// surface to the operator.
pub fn notify_draft_ready(enabled: bool, slug: &str, draft_id: &str, web_url: Option<&str>) {
    if !enabled {
        tracing::debug!(slug, "notify disabled by caller — skipping");
        return;
    }
    if !should_emit() {
        tracing::debug!(
            slug,
            "notify suppressed: no DBUS_SESSION_BUS_ADDRESS (headless host?)"
        );
        return;
    }
    let body = body_for(slug, draft_id, web_url);

    #[cfg(target_os = "linux")]
    {
        let mut n = notify_rust::Notification::new();
        n.summary("Skill draft ready")
            .body(&body)
            .icon("dialog-information")
            .appname("skill-pool");
        if let Some(url) = web_url {
            // Default action — clicking the toast opens the inbox.
            n.action("default", url);
        }
        if let Err(e) = n.show() {
            tracing::debug!(error = ?e, "desktop notification failed; ignoring");
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = web_url;
        // Other platforms: notify-rust still has a backend (macOS/Windows),
        // but we keep the surface explicit so a headless CI box on those
        // platforms doesn't crash on missing daemons either.
        let mut n = notify_rust::Notification::new();
        n.summary("Skill draft ready").body(&body);
        if let Err(e) = n.show() {
            eprintln!("skill-pool: desktop notification failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `should_emit` returns false on Linux when DBUS_SESSION_BUS_ADDRESS
    /// is unset. This is the core gate that keeps headless daemons from
    /// crashing on a missing libdbus session.
    #[test]
    #[cfg(target_os = "linux")]
    fn no_session_bus_means_no_emit_on_linux() {
        let prev = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok();
        // Safety: tests in this file are not run concurrently across the
        // same env var.
        std::env::remove_var("DBUS_SESSION_BUS_ADDRESS");
        assert!(!should_emit(), "expected no_emit when bus unset");
        if let Some(v) = prev {
            std::env::set_var("DBUS_SESSION_BUS_ADDRESS", v);
        }
    }

    /// And the inverse: a set bus address allows emission on Linux.
    #[test]
    #[cfg(target_os = "linux")]
    fn session_bus_set_allows_emit_on_linux() {
        let prev = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok();
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", "unix:path=/tmp/fake");
        assert!(should_emit(), "expected emit when bus set");
        match prev {
            Some(v) => std::env::set_var("DBUS_SESSION_BUS_ADDRESS", v),
            None => std::env::remove_var("DBUS_SESSION_BUS_ADDRESS"),
        }
    }

    #[test]
    fn body_includes_slug_and_draft() {
        let b = body_for("my-skill", "abc123", Some("https://example.com"));
        assert!(b.contains("my-skill"));
        assert!(b.contains("abc123"));
        assert!(b.contains("https://example.com/drafts/abc123"));
    }

    #[test]
    fn body_without_url_omits_action_link() {
        let b = body_for("my-skill", "abc123", None);
        assert!(b.contains("my-skill"));
        assert!(!b.contains("https://"));
    }

    #[test]
    fn trims_trailing_slash_on_web_url() {
        let b = body_for("s", "id", Some("https://example.com/"));
        assert!(
            b.contains("https://example.com/drafts/id"),
            "got: {b}"
        );
    }

    /// `notify_draft_ready(enabled=false, …)` must be a true no-op — no
    /// D-Bus call, no panic, regardless of environment.
    #[test]
    fn disabled_caller_is_no_op() {
        notify_draft_ready(false, "s", "id", Some("https://example.com"));
        notify_draft_ready(false, "s", "id", None);
    }

    /// On Linux, `notify_draft_ready` with no DBUS env must be a no-op
    /// even when `enabled=true`. We can't observe the absence of a
    /// notification directly, but we can assert the function returns
    /// without panicking when the gate kicks in.
    #[test]
    #[cfg(target_os = "linux")]
    fn headless_linux_does_not_panic() {
        let prev = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok();
        std::env::remove_var("DBUS_SESSION_BUS_ADDRESS");
        // Must complete cleanly even though `enabled = true`.
        notify_draft_ready(true, "s", "id", Some("https://example.com"));
        if let Some(v) = prev {
            std::env::set_var("DBUS_SESSION_BUS_ADDRESS", v);
        }
    }
}
