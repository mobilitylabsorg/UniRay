//! The corporate networks next to the VPN (see `uniray_core::corp`): the
//! OpenVPN profiles the user imported and the Check Point gateways they gave,
//! their settings, and the state of their tunnels in a live connection, which
//! the engines report in the log. Any number of them can be on at once, each
//! with an engine of its own.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use uniray_core::corp::{self, CorpKind, CorpNetwork, ProfileInfo, Pushed, Via};

use crate::app::{emit_state, App};
use crate::core;
use crate::error::{CmdError, CmdResult};
use crate::logs;

/// What the UI shows of the corporate network: no password, only whether
/// there is one.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CorpView {
    pub id: String,
    pub kind: CorpKind,
    pub enabled: bool,
    pub name: String,
    pub info: Option<ProfileInfo>,
    /// Check Point: the gateway as the user gave it, the certificate's file.
    pub gateway: String,
    pub cert_name: String,
    pub has_cert: bool,
    pub has_cert_password: bool,
    pub username: String,
    pub has_password: bool,
    pub use_pushed: bool,
    pub routes: Vec<String>,
    pub domains: Vec<String>,
    pub dns: Vec<String>,
    pub via: Via,
    pub learned: Pushed,
}

impl CorpView {
    pub fn of(corp: &CorpNetwork) -> Self {
        Self {
            id: corp.id.clone(),
            kind: corp.kind,
            enabled: corp.enabled,
            name: corp.name.clone(),
            info: corp.info().ok(),
            gateway: corp.gateway.clone(),
            cert_name: corp.cert_name.clone(),
            has_cert: !corp.cert.is_empty(),
            has_cert_password: !corp.cert_password.is_empty(),
            username: corp.username.clone(),
            has_password: !corp.password.is_empty(),
            use_pushed: corp.use_pushed,
            routes: corp.routes.clone(),
            domains: corp.domains.clone(),
            dns: corp.dns.clone(),
            via: corp.via.clone(),
            learned: corp.learned.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CorpState {
    #[default]
    Connecting,
    Up,
    Down,
}

/// The corporate tunnel in a live connection.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CorpStatus {
    pub state: CorpState,
    /// The tunnel's own address, once up.
    pub address: Option<String>,
    /// Why it is down.
    pub error: Option<String>,
}

/// Whether this build can run the corporate network: it needs uniray-ovpn
/// or uniray-cp next to the app (desktop for now).
pub fn available(app: &AppHandle) -> bool {
    !kinds(app).is_empty()
}

/// What this build can run the corporate network with.
pub fn kinds(app: &AppHandle) -> Vec<CorpKind> {
    [CorpKind::OpenVpn, CorpKind::CheckPoint].into_iter().filter(|kind| binary(app, *kind).is_some()).collect()
}

/// The program the engine runs for `kind`, when this build has it.
pub fn binary(app: &AppHandle, kind: CorpKind) -> Option<std::path::PathBuf> {
    #[cfg(desktop)]
    {
        let paths = &app.state::<App>().paths;
        let path = match kind {
            CorpKind::OpenVpn => &paths.ovpn,
            CorpKind::CheckPoint => &paths.cp,
        };
        path.is_file().then(|| path.clone())
    }
    #[cfg(mobile)]
    {
        let _ = (app, kind);
        None
    }
}

/// The network `id` in the list, or an error for the UI.
fn find<'a>(corps: &'a mut [CorpNetwork], id: &str) -> CmdResult<&'a mut CorpNetwork> {
    corps.iter_mut().find(|corp| corp.id == id).ok_or_else(|| CmdError::new("notFound", "no such corporate network"))
}

/// Imports an OpenVPN profile with the files it names (see
/// `corp::assemble`): a new network, or in place of the profile of network
/// `id`, whose credentials and own routes stay when it is the same server
/// again.
#[tauri::command]
pub async fn corp_import(app: AppHandle, files: Vec<(String, String)>, id: Option<String>) -> CmdResult<()> {
    let (name, profile) = corp::assemble(&files)?;
    let state = app.state::<App>();
    state
        .update(|store| -> CmdResult<()> {
            let fresh = || CorpNetwork { id: uniray_core::model::new_id(), use_pushed: true, ..Default::default() };
            let previous = match &id {
                Some(id) => find(&mut store.data.corps, id)?.clone(),
                None => fresh(),
            };
            let same_server = previous.kind == CorpKind::OpenVpn
                && corp::profile_info(&previous.profile).ok().map(|info| (info.host, info.port))
                    == corp::profile_info(&profile).ok().map(|info| (info.host, info.port));
            let kept = if same_server { previous.clone() } else { CorpNetwork { id: previous.id.clone(), ..fresh() } };
            let next = CorpNetwork { name, profile, enabled: true, learned: Pushed::default(), ..kept };
            match &id {
                Some(id) => *find(&mut store.data.corps, id)? = next,
                None => store.data.corps.push(next),
            }
            Ok(())
        })
        .map_err(CmdError::io)??;
    emit_state(&app);
    core::reconnect_if_connected(&app).await
}

/// A Check Point gateway as the user gives it. `cert` is the certificate's
/// file name and its content in base64; `None` keeps the one there is, as
/// `None` keeps a password.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckPointSetup {
    /// The network to change; `None` adds one.
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub gateway: String,
    pub cert: Option<(String, String)>,
    pub cert_password: Option<String>,
    pub username: String,
    pub password: Option<String>,
}

/// Whether `cert` (base64) is a PKCS#12 file at all; its password is checked
/// when the tunnel starts.
fn check_cert(cert: &str) -> CmdResult<()> {
    use base64::Engine;
    let der = base64::engine::general_purpose::STANDARD
        .decode(cert.trim())
        .map_err(|_| CmdError::new("invalidSettings", "the certificate file could not be read"))?;
    // PKCS#12 is a DER SEQUENCE; a PEM file or anything else is not one.
    if der.first() != Some(&0x30) {
        return Err(CmdError::new("invalidSettings", "the certificate is not a PKCS#12 file (.p12, .pfx)"));
    }
    Ok(())
}

/// Adds a Check Point gateway as a corporate network, or changes network
/// `id`; the user's own routes stay when it is the same gateway again.
#[tauri::command]
pub async fn corp_checkpoint(app: AppHandle, setup: CheckPointSetup) -> CmdResult<()> {
    let info = corp::gateway_info(&setup.gateway)?;
    if let Some((_, cert)) = &setup.cert {
        check_cert(cert)?;
    }
    let state = app.state::<App>();
    let changed = state
        .update(|store| -> CmdResult<bool> {
            let previous = match &setup.id {
                Some(id) => find(&mut store.data.corps, id)?.clone(),
                None => CorpNetwork { id: uniray_core::model::new_id(), ..Default::default() },
            };
            let same_gateway = previous.kind == CorpKind::CheckPoint
                && corp::gateway_info(&previous.gateway).ok().map(|known| (known.host, known.port))
                    == Some((info.host.clone(), info.port));
            let mut next = if same_gateway {
                previous.clone()
            } else {
                CorpNetwork { id: previous.id.clone(), kind: CorpKind::CheckPoint, use_pushed: true, ..Default::default() }
            };
            next.enabled = true;
            next.name = match setup.name.trim() {
                "" => info.host.clone(),
                name => name.to_owned(),
            };
            next.gateway = setup.gateway.trim().to_owned();
            if let Some((name, cert)) = setup.cert {
                next.cert_name = name;
                next.cert = cert.trim().to_owned();
            }
            if let Some(password) = setup.cert_password {
                next.cert_password = password;
            }
            next.username = setup.username.trim().to_owned();
            if let Some(password) = setup.password {
                next.password = password;
            }
            let changed = next != previous;
            match &setup.id {
                Some(id) => *find(&mut store.data.corps, id)? = next,
                None => store.data.corps.push(next),
            }
            Ok(changed)
        })
        .map_err(CmdError::io)??;
    emit_state(&app);
    if changed {
        core::reconnect_if_connected(&app).await?;
    }
    Ok(())
}

/// The settings the UI changes; `password` is kept when `None`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CorpUpdate {
    pub id: String,
    pub enabled: bool,
    pub username: String,
    pub password: Option<String>,
    pub use_pushed: bool,
    pub routes: Vec<String>,
    pub domains: Vec<String>,
    pub dns: Vec<String>,
    pub via: Via,
    /// Check Point: the gateway, the certificate (file name, base64) and its
    /// password; each kept when `None`.
    #[serde(default)]
    pub gateway: Option<String>,
    #[serde(default)]
    pub cert: Option<(String, String)>,
    #[serde(default)]
    pub cert_password: Option<String>,
}

#[tauri::command]
pub async fn corp_save(app: AppHandle, update: CorpUpdate) -> CmdResult<()> {
    let clean = |items: Vec<String>| -> Vec<String> {
        items.into_iter().map(|item| item.trim().to_owned()).filter(|item| !item.is_empty()).collect()
    };
    for route in &update.routes {
        let route = route.trim();
        if !route.is_empty()
            && uniray_core::routes::parse_cidr(route).is_none()
            && route.parse::<std::net::IpAddr>().is_err()
        {
            return Err(CmdError::new("invalidSettings", format!("{route} is not a network (10.0.0.0/8) or an address")));
        }
    }
    if let Some(gateway) = &update.gateway {
        corp::gateway_info(gateway)?;
    }
    if let Some((_, cert)) = &update.cert {
        check_cert(cert)?;
    }
    for dns in &update.dns {
        if !dns.trim().is_empty() && dns.trim().parse::<std::net::IpAddr>().is_err() {
            return Err(CmdError::new("invalidSettings", format!("{} is not an IP address", dns.trim())));
        }
    }
    let changed = app
        .state::<App>()
        .update(|store| {
            let corp = find(&mut store.data.corps, &update.id)?;
            let before = corp.clone();
            corp.enabled = update.enabled;
            corp.username = update.username.trim().to_owned();
            if let Some(password) = update.password {
                corp.password = password;
            }
            corp.use_pushed = update.use_pushed;
            corp.routes = clean(update.routes);
            corp.domains = clean(update.domains);
            corp.dns = clean(update.dns);
            corp.via = update.via;
            if corp.kind == CorpKind::CheckPoint {
                if let Some(gateway) = update.gateway {
                    corp.gateway = gateway.trim().to_owned();
                }
                if let Some((name, cert)) = update.cert {
                    corp.cert_name = name;
                    corp.cert = cert.trim().to_owned();
                }
                if let Some(password) = update.cert_password {
                    corp.cert_password = password;
                }
            }
            Ok::<bool, CmdError>(*corp != before)
        })
        .map_err(CmdError::io)??;
    emit_state(&app);
    if changed {
        core::reconnect_if_connected(&app).await?;
    }
    Ok(())
}

#[tauri::command]
pub async fn corp_delete(app: AppHandle, id: String) -> CmdResult<()> {
    let had = app
        .state::<App>()
        .update(|store| {
            let at = store.data.corps.iter().position(|corp| corp.id == id)?;
            Some(store.data.corps.remove(at))
        })
        .map_err(CmdError::io)?;
    emit_state(&app);
    if had.is_some_and(|corp| corp.enabled) {
        core::reconnect_if_connected(&app).await?;
    }
    Ok(())
}

/// Follows the engines' reports on the tunnels (engine/ovpn.go):
/// `corp[<id>]: up {settings}`, `corp[<id>]: down <reason>`,
/// `corp[<id>]: reconnecting`. When a server pushed other networks than the
/// run was built with, they are kept and the connection is made again with
/// them.
pub fn on_log(app: &AppHandle, text: &str) {
    let Some((id, report)) = corp::report(text) else { return };
    let state = app.state::<App>();
    let status = if report.starts_with("up ") {
        let address = report
            .split_once("\"addresses\":[\"")
            .and_then(|(_, rest)| rest.split('"').next())
            .map(|address| address.split('/').next().unwrap_or(address).to_owned());
        CorpStatus { state: CorpState::Up, address, error: None }
    } else if let Some(reason) = report.strip_prefix("down ") {
        // "AUTH_FAILED; again in 2s" -> "AUTH_FAILED".
        let reason = reason.split("; again in").next().unwrap_or(reason).trim().to_owned();
        let state = if reason.contains("AUTH_FAILED") || !report.contains("; again in") { CorpState::Down } else { CorpState::Connecting };
        CorpStatus { state, address: None, error: Some(reason) }
    } else if report.starts_with("reconnecting") {
        CorpStatus { state: CorpState::Connecting, ..Default::default() }
    } else {
        return;
    };
    {
        let mut connection = state.connection.lock().unwrap();
        // Only networks of this run (a late line of an earlier one is not).
        let Some(known) = connection.corps.get_mut(id) else { return };
        *known = status;
    }
    emit_state(app);

    let Some(pushed) = Pushed::from_log(text) else { return };
    let relearned = state.update(|store| match store.data.corps.iter_mut().find(|corp| corp.id == id) {
        Some(corp) if corp.learned != pushed => {
            corp.learned = pushed;
            corp.use_pushed
        }
        _ => false,
    });
    if let Ok(true) = relearned {
        logs::push(app, "[Info] corp: the server pushed other networks; connecting again with them");
        let generation = state.generation.load(std::sync::atomic::Ordering::SeqCst);
        let app = app.clone();
        tauri::async_runtime::spawn(async move { core::reconnect_run(&app, generation).await });
    }
}
