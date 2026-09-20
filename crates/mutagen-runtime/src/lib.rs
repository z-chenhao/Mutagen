//! # mutagen-runtime
//!
//! The future orchestration/runtime layer of Mutagen.
//!
//! ## Phase 0 status: intentionally empty
//!
//! This crate exists right now for exactly one purpose: to fix the
//! workspace dependency direction
//!
//! ```text
//! mutagen-cli → mutagen-runtime → mutagen-core
//! ```
//!
//! and to prove the three-crate seam compiles, tests, and ships through CI.
//!
//! Deliberately *not* here: an agent loop, a model-provider integration,
//! a memory system, a tool system, a plugin loader, a hot-reload mechanism,
//! and any "runtime abstraction" in general. The shape of the runtime is
//! the single most consequential unknown in this project; it must be
//! derived from experiments (see `docs/evolution.md` and `docs/experiments/`)
//! rather than assumed. Until a real use case exists, this crate stays a
//! stub on purpose.

/// The foundational domain types, re-exported so upper layers have a single
/// access path (`mutagen_runtime::core`) instead of a second direct dependency.
pub use mutagen_core as core;

/// Semantic version of this crate, exposed for tooling such as `mutagen doctor`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::VERSION;
    use mutagen_core::ComponentId;

    /// Smoke test pinning the dependency direction: the runtime layer must
    /// be able to consume core types.
    #[test]
    fn runtime_consumes_core() {
        let id = ComponentId::new("router", 1);
        assert_eq!(id.to_string(), "router@1");
        assert!(!VERSION.is_empty());
    }
}
