//! Shared lifecycle policy for human and noninteractive callers.
//! External adapters and tasks execute trusted code with the caller's privileges.
pub mod adapter;
pub mod build;
pub mod engine;
pub mod git;
pub mod manifest;
pub mod model;
mod registry;

pub use engine::Engine;
pub use manifest::{Adapter, FileMode, Manifest, Resource, SourceRoot, Task, TaskKind};
pub use model::*;
