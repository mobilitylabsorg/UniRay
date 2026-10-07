//! Identity of this device for subscription requests (HWID and friends).

use uniray_core::hwid::{derive_hwid, Device};
use uniray_core::model::new_id;

use crate::state::Store;

/// Describes the device. The HWID comes from the OS machine id so that it
/// survives reinstalls; where the OS has none, a random seed persisted in the
/// store stands in.
#[cfg(desktop)]
pub fn detect(store: &mut Store, app_version: &str) -> Device {
    let machine_id = machine_uid::get()
        .ok()
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| seed(store));
    Device {
        hwid: derive_hwid(&machine_id),
        os: os_name().to_owned(),
        os_version: os_version(),
        model: model(),
        app_version: app_version.to_owned(),
    }
}

/// On mobile the identifiers live behind platform APIs and are filled in by
/// the VPN plugin (see `core::mobile`); until then the stored seed is used.
#[cfg(mobile)]
pub fn detect(store: &mut Store, app_version: &str) -> Device {
    Device {
        hwid: derive_hwid(&seed(store)),
        os: os_name().to_owned(),
        os_version: String::new(),
        model: String::new(),
        app_version: app_version.to_owned(),
    }
}

fn seed(store: &mut Store) -> String {
    if let Some(seed) = &store.data.device_seed {
        return seed.clone();
    }
    let seed = new_id();
    store.data.device_seed = Some(seed.clone());
    if let Err(err) = store.save() {
        eprintln!("uniray: cannot persist the device seed: {err}");
    }
    seed
}

fn os_name() -> &'static str {
    match std::env::consts::OS {
        "linux" => "Linux",
        "windows" => "Windows",
        "macos" => "macOS",
        "android" => "Android",
        "ios" => "iOS",
        other => other,
    }
}

#[cfg(target_os = "linux")]
fn os_version() -> String {
    read_trimmed("/proc/sys/kernel/osrelease")
}

#[cfg(target_os = "linux")]
fn model() -> String {
    let model = read_trimmed("/sys/class/dmi/id/product_name");
    if model.is_empty() {
        read_trimmed("/proc/sys/kernel/hostname")
    } else {
        model
    }
}

#[cfg(target_os = "linux")]
fn read_trimmed(path: &str) -> String {
    std::fs::read_to_string(path).map(|text| text.trim().to_owned()).unwrap_or_default()
}

/// `10.0.22631`. `RtlGetVersion` reports the real version, unlike
/// `GetVersionEx`, which lies to apps without a compatibility manifest.
#[cfg(windows)]
fn os_version() -> String {
    use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
    use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;

    // SAFETY: the struct is plain data, sized as the call requires.
    let info = unsafe {
        let mut info: OSVERSIONINFOW = std::mem::zeroed();
        info.dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOW>() as u32;
        if RtlGetVersion(&mut info) != 0 {
            return String::new();
        }
        info
    };
    format!("{}.{}.{}", info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber)
}

#[cfg(all(desktop, not(any(target_os = "linux", windows))))]
fn os_version() -> String {
    String::new()
}

#[cfg(all(desktop, not(target_os = "linux")))]
fn model() -> String {
    std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_default()
}
