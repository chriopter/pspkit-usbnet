//! What to say and do when this user may not open the PSP on USB, per system.

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

/// On Linux and macOS the PSP on USB is root's unless a udev rule says
/// otherwise. Rather than send the user away to type sudo, the gateway
/// starts itself again under it, once: sudo asks for the password on this
/// terminal. Returns when that is not possible (no terminal, no sudo, root
/// already); then the words above stand.
pub fn elevate() {
    #[cfg(unix)]
    {
        use std::io::IsTerminal;
        use std::os::unix::process::CommandExt;
        use std::sync::atomic::{AtomicBool, Ordering};
        static TRIED: AtomicBool = AtomicBool::new(false);
        if unsafe { libc::geteuid() } == 0 || !std::io::stdin().is_terminal() || TRIED.swap(true, Ordering::SeqCst) {
            return;
        }
        let Ok(me) = std::env::current_exe() else { return };
        println!("\n\n  The PSP is there, but this user may not use USB.");
        println!("  Starting again with sudo: it asks for your password.\n");
        // Only comes back when sudo could not be started.
        let failed = std::process::Command::new("sudo").arg(me).args(std::env::args_os().skip(1)).exec();
        println!("  sudo could not be started ({failed}). Start the gateway with sudo yourself.\n");
    }
}
