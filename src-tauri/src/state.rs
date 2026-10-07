//! Persistent state: servers, subscriptions and settings in one JSON file.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uniray_core::model::{Server, Settings, Subscription};

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Data {
    pub servers: Vec<Server>,
    pub subscriptions: Vec<Subscription>,
    pub settings: Settings,
    /// Random identifier standing in for the machine id where the OS gives none.
    pub device_seed: Option<String>,
}

pub struct Store {
    path: PathBuf,
    pub data: Data,
}

impl Store {
    /// Loads the state, starting empty when the file is missing. A file that
    /// cannot be parsed is kept aside rather than overwritten.
    pub fn load(path: PathBuf) -> Self {
        let data = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|err| {
                let backup = path.with_extension("json.corrupt");
                eprintln!("uniray: {} is unreadable ({err}), moved to {}", path.display(), backup.display());
                let _ = fs::rename(&path, backup);
                Data::default()
            }),
            Err(_) => Data::default(),
        };
        Self { path, data }
    }

    /// Writes the state atomically. The file holds server credentials, so it
    /// is created private to the user.
    pub fn save(&self) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(&self.data)?;
        write_private(&self.path, &bytes)
    }

    /// The selected server, falling back to the first one.
    pub fn selected_server(&self) -> Option<&Server> {
        self.data
            .settings
            .selected_server
            .as_ref()
            .and_then(|id| self.data.servers.iter().find(|server| &server.id == id))
            .or_else(|| self.data.servers.first())
    }
}

/// Atomically replaces `path` with `bytes`, readable by the owner only.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(tmp, path)
}
