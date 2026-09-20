use std::fmt;

/// Opaque identifier for a recorded run (an "episode").
///
/// Replay-based experimentation requires that runs be addressable by a
/// stable id. The id is deliberately opaque — an opaque `String` newtype —
/// because how episodes are actually generated and persisted (file naming,
/// uuids, content hashes) is a future implementation detail, not a domain
/// decision. No parsing, no format constraints: any non-empty string is a
/// valid episode id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EpisodeId(String);

impl EpisodeId {
    /// Create an episode id from an arbitrary non-empty string.
    ///
    /// # Panics
    ///
    /// Panics on empty input: an unaddressable run is useless to replay.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        let id = id.into();
        assert!(!id.is_empty(), "episode id must not be empty");
        Self(id)
    }

    /// The raw identifier string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EpisodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_display() {
        let id = EpisodeId::new("2026-01-15-abcdef01");
        assert_eq!(id.to_string(), "2026-01-15-abcdef01");
    }

    #[test]
    fn equality_is_by_raw_string() {
        assert_eq!(EpisodeId::new("x"), EpisodeId::new("x"));
        assert_ne!(EpisodeId::new("x"), EpisodeId::new("y"));
    }
}
