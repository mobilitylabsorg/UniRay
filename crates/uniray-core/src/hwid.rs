//! Device identification sent with subscription requests.
//!
//! Remnawave's "HWID device limit" identifies devices by the `x-hwid` request
//! header and answers 404 when the feature is on and the header is missing.
//! See <https://docs.rw/docs/features/hwid-device-limit>.

use sha2::{Digest, Sha256};

/// Domain separation for [`derive_hwid`]; changing it changes every HWID.
const HWID_SALT: &str = "uniray-hwid-v1:";

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub hwid: String,
    /// Platform name, e.g. `Linux`, `Android`.
    pub os: String,
    pub os_version: String,
    pub model: String,
    pub app_version: String,
}

/// Derives the HWID from a stable machine identifier (`/etc/machine-id`,
/// `ANDROID_ID`, ...). The identifier is hashed so that it never leaves the
/// device, and the result stays the same across reinstalls.
pub fn derive_hwid(machine_id: &str) -> String {
    let digest = Sha256::new()
        .chain_update(HWID_SALT)
        .chain_update(machine_id.trim())
        .finalize();
    digest[..16].iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The format Remnawave accepts: `^[a-zA-Z0-9=-]{10,64}$`. Other values are
/// silently ignored by the panel.
pub fn is_valid_hwid(hwid: &str) -> bool {
    (10..=64).contains(&hwid.len())
        && hwid
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'=' || byte == b'-')
}

pub fn user_agent(app_version: &str) -> String {
    format!("UniRay/{app_version}")
}

/// Headers of a subscription request, lowercase names.
pub fn request_headers(device: &Device, send_hwid: bool) -> Vec<(&'static str, String)> {
    let mut headers = vec![("user-agent", user_agent(&device.app_version))];
    if send_hwid && is_valid_hwid(&device.hwid) {
        headers.push(("x-hwid", device.hwid.clone()));
        for (name, value) in [
            ("x-device-os", &device.os),
            ("x-ver-os", &device.os_version),
            ("x-device-model", &device.model),
        ] {
            let value = header_safe(value);
            if !value.is_empty() {
                headers.push((name, value));
            }
        }
    }
    headers
}

/// HTTP header values must be visible ASCII.
fn header_safe(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> Device {
        Device {
            hwid: derive_hwid("b08dfa6083e7567a1921a715000001fb"),
            os: "Linux".into(),
            os_version: "6.12".into(),
            model: "ThinkPad X1 Карбон".into(),
            app_version: "0.1.0".into(),
        }
    }

    #[test]
    fn hwid_is_stable_valid_and_not_the_machine_id() {
        let machine_id = "b08dfa6083e7567a1921a715000001fb";
        let hwid = derive_hwid(machine_id);
        assert_eq!(hwid, derive_hwid(&format!("{machine_id}\n")));
        assert_ne!(hwid, derive_hwid("another-machine"));
        assert_ne!(hwid, machine_id);
        assert_eq!(hwid.len(), 32);
        assert!(is_valid_hwid(&hwid));
    }

    #[test]
    fn hwid_validation_matches_panel_regex() {
        assert!(is_valid_hwid("abcDEF123=-"));
        assert!(!is_valid_hwid("short"));
        assert!(!is_valid_hwid(&"a".repeat(65)));
        assert!(!is_valid_hwid("has_underscore_1"));
        assert!(!is_valid_hwid("кириллица-кириллица"));
    }

    #[test]
    fn headers_carry_hwid_and_device() {
        let device = device();
        let headers = request_headers(&device, true);
        assert_eq!(
            headers,
            [
                ("user-agent", "UniRay/0.1.0".to_owned()),
                ("x-hwid", device.hwid.clone()),
                ("x-device-os", "Linux".to_owned()),
                ("x-ver-os", "6.12".to_owned()),
                ("x-device-model", "ThinkPad X1".to_owned()),
            ]
        );
    }

    #[test]
    fn hwid_can_be_turned_off() {
        assert_eq!(request_headers(&device(), false), [("user-agent", "UniRay/0.1.0".to_owned())]);
    }

    #[test]
    fn invalid_hwid_is_not_sent() {
        let device = Device { hwid: "bad id".into(), ..device() };
        assert_eq!(request_headers(&device, true).len(), 1);
    }
}
