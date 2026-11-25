//! Persistent state: servers, subscriptions and settings in one JSON file.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uniray_core::corp::CorpNetwork;
use uniray_core::model::{Server, Settings, Subscription};

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Data {
    pub servers: Vec<Server>,
    pub subscriptions: Vec<Subscription>,
    pub settings: Settings,
    /// The corporate networks next to the VPN, see `crate::corp`.
    pub corps: Vec<CorpNetwork>,
    /// The one corporate network of 0.4.0 and before; moved to `corps` on load.
    #[serde(skip_serializing)]
    pub corp: Option<CorpNetwork>,
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
        let mut store = Self { path, data };
        store.adopt_corps();
        store
    }

    /// Moves the corporate network of an older version into the list and
    /// gives every network an id.
    pub fn adopt_corps(&mut self) {
        if let Some(corp) = self.data.corp.take() {
            self.data.corps.insert(0, corp);
        }
        for index in 0..self.data.corps.len() {
            if self.data.corps[index].id.is_empty() || self.data.corps[..index].iter().any(|other| other.id == self.data.corps[index].id) {
                self.data.corps[index].id = uniray_core::model::new_id();
            }
        }
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

#[cfg(test)]
mod corp_tests {
    use super::*;

    #[test]
    fn an_older_single_corporate_network_joins_the_list() {
        let dir = std::env::temp_dir().join(format!("uniray-state-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        fs::write(&path, r#"{"corp":{"kind":"checkPoint","enabled":true,"name":"Work","gateway":"vpn.example"}}"#).unwrap();
        let store = Store::load(path.clone());
        assert_eq!(store.data.corps.len(), 1);
        assert_eq!(store.data.corps[0].name, "Work");
        assert!(!store.data.corps[0].id.is_empty());
        store.save().unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(saved.get("corp").is_none());
        assert_eq!(saved["corps"][0]["name"], "Work");
        let _ = fs::remove_dir_all(dir);
    }
}
