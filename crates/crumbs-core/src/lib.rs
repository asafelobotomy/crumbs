pub mod bloat;
pub mod command;
pub mod config;
pub mod desktop;
pub mod doctor;
pub mod env;
pub mod error;
pub mod executor;
pub mod history;
pub mod inventory;
pub mod leftovers;
pub mod model;
pub mod ownership;
pub mod plan;
pub mod privilege;
pub mod removal;
pub mod restore;
pub mod rules;
pub mod safety;
pub mod size;
pub mod sources;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod trash;

pub use env::{Distro, Env};
pub use error::{CoreError, SourceError};
pub use model::{App, Installation, SourceKind};
