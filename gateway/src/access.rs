//! What to say when this user may not open the PSP on USB, per system.

/// Said in the event log and by `--ping`.
#[cfg(windows)]
pub const HINT: &str = "install the WinUSB driver for it once with Zadig (https://zadig.akeo.ie)";
#[cfg(not(windows))]
pub const HINT: &str = "start the gateway with sudo";

/// The same in the few words a step has room for.
#[cfg(windows)]
pub const STEP: &str = "found, but without the WinUSB driver: install it with Zadig (zadig.akeo.ie)";
#[cfg(not(windows))]
pub const STEP: &str = "found, but no access to USB: start with sudo";
