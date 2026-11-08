pub mod env;
pub mod error;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use env::{Distro, Env};
pub use error::CoreError;
