//! Small, host-testable boundaries for badge inputs.
pub mod audio_buffer;
pub mod provisioning;
pub mod recording_start;
pub mod workers;
pub const STATUS_PORT: u16 = 9010;
pub const STALE_AFTER_MS: u64 = 15_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    Offline = 0,
    Idle = 1,
    Working = 2,
    NeedsYou = 3,
    Done = 4,
}

pub fn parse_status(packet: &[u8]) -> Option<Status> {
    match packet.strip_suffix(b"\n").unwrap_or(packet) {
        b"idle" => Some(Status::Idle),
        b"working" => Some(Status::Working),
        b"needs-you" => Some(Status::NeedsYou),
        b"done" => Some(Status::Done),
        _ => None,
    }
}

pub struct StatusTracker {
    latest: Option<(Status, u64)>,
}
impl Default for StatusTracker {
    fn default() -> Self {
        Self::new()
    }
}
impl StatusTracker {
    pub const fn new() -> Self {
        Self { latest: None }
    }
    /// Call only after peer/source/ACL checks. Invalid messages never refresh freshness.
    pub fn receive(&mut self, packet: &[u8], now_ms: u64) -> bool {
        if let Some(status) = parse_status(packet) {
            self.latest = Some((status, now_ms));
            true
        } else {
            false
        }
    }
    pub fn current(&self, now_ms: u64) -> Status {
        match self.latest {
            Some((status, seen)) if now_ms.saturating_sub(seen) < STALE_AFTER_MS => status,
            _ => Status::Offline,
        }
    }
}

pub fn valid_approval_url(url: &str) -> bool {
    !url.bytes()
        .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
        && (url.starts_with("https://login.tailscale.com/")
            || url.starts_with("https://controlplane.tailscale.com/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_boundary_and_staleness() {
        for (text, expected) in [
            ("idle", Status::Idle),
            ("working", Status::Working),
            ("needs-you", Status::NeedsYou),
            ("done", Status::Done),
        ] {
            assert_eq!(parse_status(text.as_bytes()), Some(expected));
            assert_eq!(parse_status(format!("{text}\n").as_bytes()), Some(expected));
        }
        for bytes in [
            b"".as_slice(),
            b"working\0",
            b"working\n\n",
            b"idle\r\n",
            b" working",
            b"offline",
            &[0xff],
        ] {
            assert_eq!(parse_status(bytes), None);
        }
        let mut tracker = StatusTracker::new();
        assert_eq!(tracker.current(0), Status::Offline);
        assert!(tracker.receive(b"working", 100));
        assert!(!tracker.receive(b"arbitrary remote text", 14_999));
        assert_eq!(tracker.current(15_099), Status::Working);
        assert_eq!(tracker.current(15_100), Status::Offline);
        assert!(tracker.receive(b"done", 15_101));
        assert_eq!(tracker.current(15_101), Status::Done);
    }
    #[test]
    fn approval_links_are_restricted() {
        assert!(valid_approval_url("https://login.tailscale.com/a/example"));
        for url in [
            "http://login.tailscale.com/a/x",
            "https://login.tailscale.com.evil/a/x",
            "https://login.tailscale.com@evil/a/x",
            "https://login.tailscale.com/a/\ntext",
        ] {
            assert!(!valid_approval_url(url));
        }
    }
}
