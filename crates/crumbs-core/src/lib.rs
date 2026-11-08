pub mod command;
pub mod env;
pub mod error;
pub mod model;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use env::{Distro, Env};
pub use error::{CoreError, SourceError};
pub use model::{App, Installation, SourceKind};
