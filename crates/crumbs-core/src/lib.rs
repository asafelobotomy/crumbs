pub mod command;
pub mod desktop;
pub mod doctor;
pub mod env;
pub mod error;
pub mod inventory;
pub mod model;
pub mod privilege;
pub mod sources;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use env::{Distro, Env};
pub use error::{CoreError, SourceError};
pub use model::{App, Installation, SourceKind};
