//! Shared recording admission reasons; never imply audio was captured on rejection.
use std::fmt;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartBlocked {
    SetupOpen,
    Offline,
    ClockPending,
}
impl StartBlocked {
    pub fn check(connected: bool, time_ready: bool, setup_open: bool) -> Option<Self> {
        if setup_open {
            Some(Self::SetupOpen)
        } else if !connected {
            Some(Self::Offline)
        } else if !time_ready {
            Some(Self::ClockPending)
        } else {
            None
        }
    }
    pub fn message(self) -> &'static str {
        match self {
            Self::SetupOpen => "Close Wi-Fi setup before recording.",
            Self::Offline => "No Wi-Fi. Both buttons: settings.",
            Self::ClockPending => "Wi-Fi joined; waiting for clock sync.",
        }
    }
}
impl fmt::Display for StartBlocked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}
impl std::error::Error for StartBlocked {}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn start_is_blocked_with_the_specific_cause_until_network_is_ready() {
        assert_eq!(
            StartBlocked::check(false, false, false),
            Some(StartBlocked::Offline)
        );
        assert_eq!(
            StartBlocked::check(true, false, false),
            Some(StartBlocked::ClockPending)
        );
        assert_eq!(
            StartBlocked::check(true, true, true),
            Some(StartBlocked::SetupOpen)
        );
        assert_eq!(
            StartBlocked::check(false, false, true),
            Some(StartBlocked::SetupOpen)
        );
        assert_eq!(StartBlocked::check(true, true, false), None);
        assert!(StartBlocked::Offline.message().contains("No Wi-Fi"));
    }
}
