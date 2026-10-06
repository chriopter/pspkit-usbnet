//! What to do when this user may not open the PSP on USB, per system: the
//! words for it, and on Linux the fix itself (a udev rule), which the screen
//! offers and runs.

/// Said in the event log and by `--ping`.
#[cfg(target_os = "linux")]
pub const HINT: &str = "run with sudo, or allow it once: \
echo 'SUBSYSTEM==\"usb\", ATTR{idVendor}==\"054c\", ATTR{idProduct}==\"01c9\", MODE=\"0666\"' | sudo tee /etc/udev/rules.d/50-pspkit-usbnet.rules \
&& sudo udevadm control --reload && sudo udevadm trigger";
#[cfg(windows)]
pub const HINT: &str = "install the WinUSB driver for it once with Zadig (https://zadig.akeo.ie)";
#[cfg(not(any(target_os = "linux", windows)))]
pub const HINT: &str = "run with sudo";

/// The same in the few words a step has room for.
#[cfg(target_os = "linux")]
pub const STEP: &str = "found, but no access to USB.  F + Enter: allow it";
#[cfg(windows)]
pub const STEP: &str = "found, but without the WinUSB driver: install it with Zadig (zadig.akeo.ie)";
#[cfg(not(any(target_os = "linux", windows)))]
pub const STEP: &str = "found, but no access to USB (run with sudo)";

/// Whether `allow` can do anything here.
pub const CAN_ALLOW: bool = cfg!(target_os = "linux");

/// Writes the udev rule that lets every user open the PSP, and applies it to
/// the PSP that is plugged in. sudo asks for the password on the terminal.
/// True when it went through; the links find the PSP on their next look.
pub fn allow() -> bool {
    const RULE: &str = r#"SUBSYSTEM=="usb", ATTR{idVendor}=="054c", ATTR{idProduct}=="01c9", MODE="0666""#;
    let script = format!(
        "printf '%s\\n' '{RULE}' > /etc/udev/rules.d/50-pspkit-usbnet.rules \
         && udevadm control --reload && udevadm trigger --subsystem-match=usb --attr-match=idVendor=054c"
    );
    std::process::Command::new("sudo")
        .args(["sh", "-c", &script])
        .status()
        .is_ok_and(|s| s.success())
}
