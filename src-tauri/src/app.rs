//! Shared application state and the snapshot the frontend renders.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use uniray_core::hwid::Device;
use uniray_core::model::{Server, Settings, Subscription};

use crate::core::{ConnectionInfo, Running};
use crate::stats::Traffic;
use tauri::async_runtime::JoinHandle;
use crate::logs::LogLine;
use crate::state::Store;

pub const STATE_EVENT: &str = "state";

pub struct Paths {
    /// Holds the generated core config while connected.
    pub run_dir: PathBuf,
    /// Directory with `geoip.dat` and `geosite.dat`.
    #[cfg(desktop)]
    pub assets: PathBuf,
    #[cfg(desktop)]
    pub xray: PathBuf,
    #[cfg(desktop)]
    pub helper: PathBuf,
    /// Runs the servers Xray cannot (WireGuard, AmneziaWG), see `core::engine`.
    #[cfg(desktop)]
    pub engine: PathBuf,
    /// NaiveProxy's client, which the engine runs for a Naive server.
    #[cfg(desktop)]
    pub naive: PathBuf,
}

pub struct App {
    pub store: Mutex<Store>,
    pub device: Mutex<Device>,
    pub connection: Mutex<ConnectionInfo>,
    /// The running core. Held for the whole of a connect or disconnect, which
    /// serializes them.
    pub running: tokio::sync::Mutex<Option<Running>>,
    /// Identifies a core run, so that a late exit notification of an old run
    /// cannot tear down a newer one.
    pub generation: AtomicU64,
    pub logs: Mutex<VecDeque<LogLine>>,
    pub testing: AtomicBool,
    /// Running on a TV (Android TV / Google TV): the frontend switches to its
    /// remote-control layout. Set once the device info is in.
    pub tv: AtomicBool,
    /// The TV's import page while its dialog is open (handoff.rs).
    pub handoff: Mutex<Option<crate::handoff::Handoff>>,
    /// The latest traffic sample of the live connection.
    pub traffic: Mutex<Traffic>,
    /// Connection checker and system UI presenter, see `monitor`.
    pub monitor: Mutex<Option<(JoinHandle<()>, JoinHandle<()>)>>,
    pub visible: AtomicBool,
    pub paths: Paths,
    /// Direct HTTP client, ignoring any proxy environment.
    pub http: reqwest::Client,
}

impl App {
    pub fn new(store: Store, device: Device, paths: Paths) -> Self {
        Self {
            store: Mutex::new(store),
            device: Mutex::new(device),
            connection: Mutex::new(ConnectionInfo::default()),
            running: tokio::sync::Mutex::new(None),
            generation: AtomicU64::new(0),
            logs: Mutex::new(VecDeque::new()),
            testing: AtomicBool::new(false),
            tv: AtomicBool::new(false),
            handoff: Mutex::new(None),
            traffic: Mutex::new(Traffic::default()),
            monitor: Mutex::new(None),
            visible: AtomicBool::new(true),
            paths,
            http: reqwest::Client::builder()
                .no_proxy()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .build()
                .expect("static HTTP client configuration"),
        }
    }

    /// Mutates the store and persists it.
    pub fn update<T>(&self, mutate: impl FnOnce(&mut Store) -> T) -> std::io::Result<T> {
        let mut store = self.store.lock().unwrap();
        let result = mutate(&mut store);
        store.save()?;
        Ok(result)
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    pub hwid: String,
    pub os: String,
    pub os_version: String,
    pub model: String,
}

/// Everything the frontend needs to render, sent whole on every change.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub servers: Vec<Server>,
    pub subscriptions: Vec<Subscription>,
    pub settings: Settings,
    pub connection: ConnectionInfo,
    pub device: DeviceView,
    pub version: String,
    /// `linux`, `windows`, `macos`, `android` or `ios`.
    pub platform: &'static str,
    /// A delay test is in progress.
    pub testing: bool,
    /// A TV: remote-control layout.
    pub tv: bool,
    /// What this edition offers (the Google Play one has no stealth mode).
    pub features: crate::edition::Features,
}

pub fn snapshot(app: &AppHandle) -> Snapshot {
    let state = app.state::<App>();
    let connection = state.connection.lock().unwrap().clone();
    let store = state.store.lock().unwrap();
    let device = state.device.lock().unwrap();
    Snapshot {
        servers: store.data.servers.clone(),
        subscriptions: store.data.subscriptions.clone(),
        settings: store.data.settings.clone(),
        connection,
        device: DeviceView {
            hwid: device.hwid.clone(),
            os: device.os.clone(),
            os_version: device.os_version.clone(),
            model: device.model.clone(),
        },
        version: device.app_version.clone(),
        platform: std::env::consts::OS,
        testing: state.testing.load(Ordering::Relaxed),
        tv: state.tv.load(Ordering::Relaxed),
        features: crate::edition::FEATURES,
    }
}

/// Tells the frontend that the state changed.
pub fn emit_state(app: &AppHandle) {
    let _ = app.emit(STATE_EVENT, snapshot(app));
    #[cfg(desktop)]
    crate::tray::refresh(app);
}
