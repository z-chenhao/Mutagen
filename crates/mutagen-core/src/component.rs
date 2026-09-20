use std::fmt;
use std::str::FromStr;

/// Stable, versioned identity of a runtime component.
///
/// Anything that may eventually be evolvable — a prompt, a tool, a routing
/// policy, a workflow — is expected to carry one of these. The design is
/// intentionally boring: a logical name plus a monotonic version.
///
/// - `name` is stable across evolution: `router@1` and `router@7` are
///   versions of the same logical component.
/// - `version` increases monotonically; version `n + 1` is a descendant of
///   `n`. Lineage representation beyond "next version" is an open research
///   question (see `docs/evolution.md`) and deliberately not encoded here.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ComponentId {
    name: String,
    version: u32,
}

impl ComponentId {
    /// Create a component identity.
    ///
    /// # Panics
    ///
    /// Panics if `name` is empty. An anonymous component cannot be
    /// referenced in feedback, replay, or rollback.
    #[must_use]
    pub fn new(name: impl Into<String>, version: u32) -> Self {
        let name = name.into();
        assert!(!name.is_empty(), "component name must not be empty");
        Self { name, version }
    }

    /// The logical name, stable across versions.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The monotonic version number.
    #[must_use]
    pub fn version(&self) -> u32 {
        self.version
    }

    /// The identity of the next version of the same logical component.
    #[must_use]
    pub fn next_version(&self) -> Self {
        Self {
            name: self.name.clone(),
            version: self.version + 1,
        }
    }
}

impl fmt::Display for ComponentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.name, self.version)
    }
}

/// A component identity could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentIdParseError {
    /// The input that failed to parse, for diagnostics.
    pub input: String,
}

impl fmt::Display for ComponentIdParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid component id: {:?}", self.input)
    }
}

impl std::error::Error for ComponentIdParseError {}

impl FromStr for ComponentId {
    type Err = ComponentIdParseError;

    /// Parse the canonical `name@version` form.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.rsplit_once('@') {
            // `rsplit_once` so names may themselves contain `@` in the
            // future; the *last* `@` delimits the version.
            Some((name, version)) if !name.is_empty() => version
                .parse::<u32>()
                .ok()
                .map(|v| Self {
                    name: name.to_string(),
                    version: v,
                })
                .ok_or(ComponentIdParseError {
                    input: s.to_string(),
                }),
            _ => Err(ComponentIdParseError {
                input: s.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_and_parse_round_trip() {
        let id = ComponentId::new("router", 7);
        assert_eq!(id.to_string(), "router@7");
        assert_eq!("router@7".parse::<ComponentId>().expect("parses"), id);
    }

    #[test]
    fn parses_trailing_at_in_name() {
        let id = "a@b@3".parse::<ComponentId>().expect("parses");
        assert_eq!(id.name(), "a@b");
        assert_eq!(id.version(), 3);
    }

    #[test]
    fn rejects_malformed_input() {
        for bad in ["", "@3", "name@", "name@notanumber"] {
            assert!(
                bad.parse::<ComponentId>().is_err(),
                "expected parse failure for {bad:?}"
            );
        }
    }

    #[test]
    fn next_version_bumps_monotonically() {
        let id = ComponentId::new("prompt", 3);
        let next = id.next_version();
        assert_eq!(next.name(), "prompt");
        assert_eq!(next.version(), 4);
    }

    #[test]
    #[should_panic(expected = "component name must not be empty")]
    fn empty_name_is_rejected() {
        let _ = ComponentId::new("", 1);
    }
}
