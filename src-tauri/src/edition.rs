//! Which edition this build is. The full one (GitHub releases) has every
//! feature; the Google Play one (built with `UNIRAY_STORE=play`, which
//! build.rs turns into `cfg(play_store)`) leaves out what Play's policies do
//! not allow: stealth mode (a device-wide tunnel without VpnService) and ad
//! blocking (blocking other apps' ads). The code for them is not compiled in,
//! here or in the Android project (src/play vs src/full).

use serde::Serialize;
use uniray_core::model::Settings;

/// What this edition offers; the UI hides the rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Features {
    pub stealth: bool,
    pub ad_block: bool,
}

#[cfg(not(play_store))]
pub const FEATURES: Features = Features { stealth: true, ad_block: true };
#[cfg(play_store)]
pub const FEATURES: Features = Features { stealth: false, ad_block: false };

/// Turns off the settings this edition cannot honour: saved ones from the
/// full edition, a backup or the settings page alike.
pub fn restrict(settings: &mut Settings) {
    restrict_to(settings, FEATURES);
}

fn restrict_to(settings: &mut Settings, features: Features) {
    if !features.stealth {
        settings.stealth = false;
        settings.stealth_root_tun = false;
    }
    if !features.ad_block {
        settings.block_ads = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_play_edition_turns_off_what_it_lacks() {
        let wanted = Settings { stealth: true, stealth_root_tun: true, block_ads: true, ..Default::default() };

        let mut full = wanted.clone();
        restrict_to(&mut full, Features { stealth: true, ad_block: true });
        assert_eq!(full, wanted);

        let mut play = wanted.clone();
        restrict_to(&mut play, Features { stealth: false, ad_block: false });
        assert!(!play.stealth && !play.stealth_root_tun && !play.block_ads);
        assert_eq!(Settings { stealth: true, stealth_root_tun: true, block_ads: true, ..play }, wanted);
    }
}
