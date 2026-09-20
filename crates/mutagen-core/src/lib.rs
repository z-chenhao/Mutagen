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
//! What *is* here consists of domain identities whose requirements are
//! independent of any future design decision:
//!
//! - [`ComponentId`] — a stable, versioned identity for anything that may
//!   one day be an evolvable component (prompts, tools, workflows, …).
//!   Versioning, lineage, and rollback all presuppose such an identity.
//! - [`EpisodeId`] — an opaque identifier for a recorded run. Replay-based
//!   experimentation presupposes that runs are addressable.
//!
//! Both types are dependency-free, deterministic, and `Send + Sync`.
//!
//! ## How this API is allowed to grow
//!
//! New public types or traits may be added only when an experiment or a
//! concrete consumer demonstrates the need (see `docs/decisions/` and
//! `AGENTS.md`). Evidence before abstraction.

mod component;
mod episode;

pub use component::ComponentId;
pub use episode::EpisodeId;

/// Semantic version of this crate, exposed for tooling such as `mutagen doctor`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
