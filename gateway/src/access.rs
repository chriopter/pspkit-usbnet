//! What to say and do when this user may not open the PSP on USB, per system.

/// Said in the event log and by `--ping`.
#[cfg(windows)]
pub const HINT: &str = "Windows has no driver for it; the gateway installs one when started in a console window, or use Zadig (https://zadig.akeo.ie)";
#[cfg(not(windows))]
pub const HINT: &str = "start the gateway with sudo";

/// The same in the few words a step has room for.
#[cfg(windows)]
pub const STEP: &str = "found, but Windows has no driver for it";
#[cfg(not(windows))]
pub const STEP: &str = "found, but no access to USB: start with sudo";

/// On Linux and macOS the PSP on USB is root's unless a udev rule says
/// otherwise. Rather than send the user away to type sudo, the gateway
/// starts itself again under it, once: sudo asks for the password on this
/// terminal. Returns when that is not possible (no terminal, no sudo, root
/// already); then the words above stand.
///
/// On Windows nobody may use the PSP until the WinUSB driver is bound to
/// the usbnet interface. Rather than send the user away to Zadig, the
/// gateway installs it, once: Windows asks for permission (UAC). Returns
/// when that is done or was refused; the link looks for the PSP again as
/// it always does.
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
    #[cfg(windows)]
    {
        use std::io::IsTerminal;
        use std::sync::atomic::{AtomicBool, Ordering};
        static TRIED: AtomicBool = AtomicBool::new(false);
        // Somebody has to be there to answer Windows' question.
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() || TRIED.swap(true, Ordering::SeqCst) {
            return;
        }
        println!("\n\n  Windows has no driver for the PSP on USB yet.");
        println!("  Installing it now: Windows asks once for permission.");
        println!("  A certificate made on this computer is added, so that Windows accepts the driver.\n");
        // Done: nothing to say, the PSP's step says "found" next.
        if let Err(why) = install_driver() {
            println!("  The driver was not installed ({why}).");
            println!("  Start the gateway again to try once more, or install the WinUSB driver with Zadig (https://zadig.akeo.ie).\n");
        }
    }
}

/// Whether a PSP with usbnet as its only interface is on USB and Windows has
/// no driver for it. libusb does not list such a device at all (beside
/// PSPLink it does: the composite device has Windows' own driver), so
/// Windows is asked: a device 054c:01c9 that is present, has a problem and
/// is of class 0xfd. PSPLink alone looks the same but for its class.
#[cfg(windows)]
pub fn driverless() -> bool {
    use std::ffi::c_void;
    const PRESENT_OF_ENUMERATOR: u32 = 0x101; // CM_GETIDLIST_FILTER_ENUMERATOR | _PRESENT
    const DN_HAS_PROBLEM: u32 = 0x400;
    const CM_DRP_COMPATIBLEIDS: u32 = 3;
    #[link(name = "cfgmgr32")]
    unsafe extern "system" {
        fn CM_Get_Device_ID_List_SizeW(len: *mut u32, filter: *const u16, flags: u32) -> u32;
        fn CM_Get_Device_ID_ListW(filter: *const u16, list: *mut u16, len: u32, flags: u32) -> u32;
        fn CM_Locate_DevNodeW(node: *mut u32, id: *const u16, flags: u32) -> u32;
        fn CM_Get_DevNode_Status(status: *mut u32, problem: *mut u32, node: u32, flags: u32) -> u32;
        fn CM_Get_DevNode_Registry_PropertyW(node: u32, property: u32, kind: *mut u32, buffer: *mut c_void, len: *mut u32, flags: u32) -> u32;
    }
    let filter: Vec<u16> = "USB\\VID_054C&PID_01C9\0".encode_utf16().collect();
    let mut len = 0;
    if unsafe { CM_Get_Device_ID_List_SizeW(&mut len, filter.as_ptr(), PRESENT_OF_ENUMERATOR) } != 0 {
        return false;
    }
    let mut list = vec![0u16; len as usize];
    if unsafe { CM_Get_Device_ID_ListW(filter.as_ptr(), list.as_mut_ptr(), len, PRESENT_OF_ENUMERATOR) } != 0 {
        return false;
    }
    // The ids one after another, each ended by a zero.
    list.split(|&c| c == 0).filter(|id| !id.is_empty()).any(|id| {
        let id: Vec<u16> = id.iter().copied().chain([0]).collect();
        let (mut node, mut status, mut problem) = (0, 0, 0);
        let mut ids = [0u16; 512];
        let mut size = std::mem::size_of_val(&ids) as u32;
        unsafe {
            CM_Locate_DevNodeW(&mut node, id.as_ptr(), 0) == 0
                && CM_Get_DevNode_Status(&mut status, &mut problem, node, 0) == 0
                && status & DN_HAS_PROBLEM != 0
                && CM_Get_DevNode_Registry_PropertyW(node, CM_DRP_COMPATIBLEIDS, std::ptr::null_mut(), ids.as_mut_ptr().cast(), &mut size, 0) == 0
                && String::from_utf16_lossy(&ids).to_ascii_uppercase().contains("USB\\CLASS_FD")
        }
    })
}

#[cfg(not(windows))]
pub fn driverless() -> bool {
    false
}

/// Runs `DRIVER_PS1` as administrator and waits for it.
#[cfg(windows)]
fn install_driver() -> Result<(), String> {
    use std::process::Command;
    // By its full path: what runs as administrator is not looked up in PATH.
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let powershell = std::path::Path::new(&root).join("System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    let script = std::env::temp_dir().join("pspkit-usbnet-driver.ps1");
    std::fs::write(&script, DRIVER_PS1).map_err(|e| format!("{}: {e}", script.display()))?;
    // Start-Process -Verb RunAs is what brings Windows' question. The paths
    // go through the environment: no quoting can spoil them.
    const ASK: &str = "try { $p = Start-Process -FilePath $env:PSPKIT_POWERSHELL -Verb RunAs -Wait -PassThru -WindowStyle Hidden \
        -ArgumentList ('-NoProfile -ExecutionPolicy Bypass -File \"{0}\"' -f $env:PSPKIT_SCRIPT); exit $p.ExitCode } catch { exit 1223 }";
    let status = Command::new(&powershell)
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", ASK])
        .env("PSPKIT_POWERSHELL", &powershell)
        .env("PSPKIT_SCRIPT", &script)
        .stdin(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("PowerShell did not start: {e}"))?;
    // The script writes down what it did, beside itself.
    let log = script.with_extension("log");
    let _ = std::fs::remove_file(&script);
    match status.code() {
        Some(0) => {
            let _ = std::fs::remove_file(&log);
            Ok(())
        }
        // ERROR_CANCELLED: the question was answered with No.
        Some(1223) => Err("permission was not given".into()),
        _ => Err(format!("see {}", log.display())),
    }
}

/// The driver, as Windows wants it: an INF that says "WinUSB for the PSP's
/// usbnet interface" and refers to Windows' own winusb.inf, a catalog of it,
/// and a signature under the catalog. Nobody else has signed it, so the
/// certificate is made here and put among this computer's trusted ones; its
/// key is deleted once the catalog is signed, so nothing else can ever be
/// signed with it.
///
/// The INF names the interface in both forms the PSP has: beside PSPLink it
/// is interface 1 of a composite device (`&MI_01`), alone it is the whole
/// device. Alone it is named by its class, not by `VID_054C&PID_01C9`: that
/// is also the name of the composite device as a whole, and Windows would
/// put WinUSB on all of it and take PSPLink's interface away.
#[cfg(windows)]
const DRIVER_PS1: &str = r#"# pspkit-usbnetd: installs the WinUSB driver for the PSP's usbnet interface. Run as administrator.
$ErrorActionPreference = 'Stop'
$log = [IO.Path]::ChangeExtension($PSCommandPath, 'log')
$subject = 'CN=pspkit-usbnet driver (made on this computer)'
$dir = Join-Path $env:SystemRoot 'Temp\pspkit-usbnet-driver'
$pnputil = Join-Path $env:SystemRoot 'System32\pnputil.exe'
try {
    "$(Get-Date -Format o) start" | Set-Content $log
    Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $dir | Out-Null
    @"
; WinUSB for the usbnet interface of a PSP running usbnet.prx (pspkit-usbnet).
[Version]
Signature   = "`$Windows NT`$"
Class       = USBDevice
ClassGUID   = {88BAE032-5A81-49f0-BC3D-A4FF138216D6}
Provider    = %Provider%
CatalogFile = pspusbnet.cat
DriverVer   = $(Get-Date -Format 'MM\/dd\/yyyy'),1.0.0.0
PnpLockdown = 1

[Manufacturer]
%Provider% = Devices,NTamd64,NTarm64

; Interface 1 beside PSPLink; alone, the device by its class: Sony's of
; that class where Windows says so, any of that class where it does not.
[Devices.NTamd64]
%Name% = WinUSB_Install, USB\VID_054C&PID_01C9&MI_01
%Name% = WinUSB_Install, USB\COMPAT_VID_054C&Class_FD&SubClass_00&Prot_00
%Name% = WinUSB_Install, USB\Class_FD&SubClass_00&Prot_00
[Devices.NTarm64]
%Name% = WinUSB_Install, USB\VID_054C&PID_01C9&MI_01
%Name% = WinUSB_Install, USB\COMPAT_VID_054C&Class_FD&SubClass_00&Prot_00
%Name% = WinUSB_Install, USB\Class_FD&SubClass_00&Prot_00

[WinUSB_Install]
Include = winusb.inf
Needs   = WINUSB.NT

[WinUSB_Install.Services]
Include = winusb.inf
Needs   = WINUSB.NT.Services

[WinUSB_Install.HW]
AddReg = Dev_AddReg

[Dev_AddReg]
HKR,,DeviceInterfaceGUIDs,0x10000,"{6C1B0A3E-7F1D-4C36-9B0E-5053504E4554}"

[Strings]
Provider = "pspkit-usbnet"
Name     = "PSP network over USB (usbnet)"
"@ | Set-Content -Encoding ASCII "$dir\pspusbnet.inf"

    # An earlier one of ours goes first: its package and its certificates.
    $name = $null
    foreach ($line in (& $pnputil /enum-drivers)) {
        if ($line -match '(oem\d+\.inf)') { $name = $Matches[1] }
        elseif ($line -match 'pspusbnet\.inf' -and $name) { & $pnputil /delete-driver $name /uninstall | Add-Content $log; $name = $null }
    }
    foreach ($s in 'Root', 'TrustedPublisher', 'My') {
        Get-ChildItem "Cert:\LocalMachine\$s" | Where-Object Subject -eq $subject | Remove-Item
    }

    $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject $subject -CertStoreLocation Cert:\LocalMachine\My -NotAfter (Get-Date).AddYears(20) -HashAlgorithm SHA256
    $public = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2 (, $cert.RawData)
    foreach ($s in 'Root', 'TrustedPublisher') {
        $store = New-Object System.Security.Cryptography.X509Certificates.X509Store($s, 'LocalMachine')
        $store.Open('ReadWrite'); $store.Add($public); $store.Close()
    }
    New-FileCatalog -Path "$dir\pspusbnet.inf" -CatalogFilePath "$dir\pspusbnet.cat" -CatalogVersion 2.0 | Out-Null
    $signed = Set-AuthenticodeSignature -FilePath "$dir\pspusbnet.cat" -Certificate $cert -HashAlgorithm SHA256
    "signature: $($signed.Status)" | Add-Content $log
    # The key goes; the certificate stays trusted.
    Remove-Item "Cert:\LocalMachine\My\$($cert.Thumbprint)" -DeleteKey
    if ($signed.Status -ne 'Valid') { throw "signature: $($signed.StatusMessage)" }

    & $pnputil /add-driver "$dir\pspusbnet.inf" /install | Add-Content $log
    $code = $LASTEXITCODE
    "pnputil: $code" | Add-Content $log
    & $pnputil /scan-devices | Out-Null
    Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
    # 3010: installed, Windows would like a restart. 259: nothing more to do.
    if ($code -ne 0 -and $code -ne 3010 -and $code -ne 259) { exit $code }
    exit 0
} catch {
    "failed: $($_ | Out-String)" | Add-Content $log
    exit 1
}
"#;
