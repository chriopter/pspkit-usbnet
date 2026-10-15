//! CLI validation must finish before any USB access or gateway startup.
use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pspkit-usbnetd"))
        .args(args)
        .env("TERM", "dumb")
        .output()
        .expect("run gateway binary")
}

#[test]
fn help_and_version_work_without_usb() {
    let help = run(&["--help"]);
    assert!(help.status.success());
    let text = String::from_utf8(help.stdout).unwrap();
    assert!(text.contains("Usage: pspkit-usbnetd"));
    assert!(!text.contains('\u{1b}'));
    let version = run(&["--version"]);
    assert!(version.status.success());
    assert_eq!(String::from_utf8(version.stdout).unwrap(),
        format!("pspkit-usbnetd {}\n", env!("CARGO_PKG_VERSION")));
}

#[test]
fn invalid_options_fail_before_starting_usb() {
    for args in [&["--unknown"][..], &["--dns"], &["--dns", "invalid"], &["--dns", "--help"]] {
        let output = run(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
    }
}

#[cfg(unix)]
#[test]
fn invalid_unicode_is_a_usage_error() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let output = Command::new(env!("CARGO_BIN_EXE_pspkit-usbnetd"))
        .arg(OsString::from_vec(vec![0xff]))
        .output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
}
