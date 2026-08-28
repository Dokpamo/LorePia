//! Deterministic, provider-neutral prompt orchestration.

mod interactions;
mod knowledge;
mod memory;
mod modules;
mod package;
mod portable_text;
mod resolver;
mod template;
mod transforms;

pub use interactions::*;
pub use knowledge::*;
pub use memory::*;
pub use modules::*;
pub use package::*;
pub use portable_text::*;
pub use resolver::*;
pub use template::*;
pub use transforms::*;

/// Canonical package manifest persisted by the domain crate.
pub type LorepiaPackageManifest = lorepia_domain::PackageManifest;
pub use lorepia_domain::PackageContentHash;
