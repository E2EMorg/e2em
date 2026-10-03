//! Local service adapter. OS peer credentials plus an explicit application grant.
#[cfg(unix)]
mod unix;
pub use common::{Grant, Grants, proof, read_frame, write_frame};
#[cfg(unix)]
pub use unix::*;

mod common;

#[cfg(unix)]
mod pressure;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;
