//! Small audited OS security boundary; the assessment library forbids unsafe code.
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(windows)]
mod windows_pressure;
#[cfg(windows)]
pub use windows_pressure::PressureMonitor;
#[cfg(target_os = "macos")]
mod macos_pressure;
#[cfg(target_os = "macos")]
pub use macos_pressure::PressureMonitor;
