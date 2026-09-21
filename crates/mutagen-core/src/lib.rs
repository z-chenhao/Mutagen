//! # mutagen-core
//!
//! Foundational domain types shared across the Mutagen workspace.
//!
//! ## The public API is intentionally minimal
//!
//! Mutagen is in **Phase 0: repository bootstrap**. We do not yet know the
//! correct architecture for self-evolution, and this crate deliberately
//! refuses to guess it. No agent loop, no provider integration, no
//! evolution algorithms, no plugin system, no global state lives here —
//! inventing those now would spend architectural freedom we need later.
//!
//! What *is* here is the smallest type justified by an immediate
//! requirement:
//!
//! - [`EpisodeId`] — an opaque identifier for a recorded run. Replay-based
//!   experimentation presupposes that runs are addressable. The internal
//!   format is deliberately opaque; how episode ids are generated and
//!   persisted is a future implementation detail, not a domain decision.
//!
//! Component identity, revision identity, versioning, and lineage are
//! **intentionally undefined** in this crate until experiments establish
//! their requirements (see `docs/evolution.md`). The type formerly
//! occupying that space was removed for encoding a linear, monotonically
//! numbered lineage model that no experiment has validated.
//!
//! ## How this API is allowed to grow
//!
//! New public types or traits may be added only when an experiment or a
//! concrete consumer demonstrates the need (see `docs/decisions/` and
//! `AGENTS.md`). Evidence before abstraction.

mod episode;

pub use episode::EpisodeId;

/// Semantic version of this crate, exposed for tooling such as `mutagen doctor`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
