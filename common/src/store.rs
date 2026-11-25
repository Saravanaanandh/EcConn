//! Tiny on-disk store of paired hosts (M7 UI).
//!
//! No serde: one `|`-separated line per host, keys in hex, a `#` header and
//! `#` comments are ignored. Small enough to unit-test thoroughly.
//!
//! ```text
//! # lowlat paired hosts v1
//! host-id|host name|00000000deadbeef|<64 hex chars>
//! ```

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::pairing::{from_hex, to_hex, KEY_LEN};

/// First line written by [`PairedStore::to_text`].
pub const STORE_HEADER: &str = "# lowlat paired hosts v1";

/// One host the user allowed before.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairedHost {
    pub host_id: String,
    pub host_name: String,
    /// Token that input datagrams must carry.
    pub token: u64,
    /// 32-byte session key, kept for the future encrypted stream.
    pub key: [u8; KEY_LEN],
}

#[derive(Default, Clone, Debug)]
pub struct PairedStore {
    pub hosts: Vec<PairedHost>,
}

impl PairedStore {
    /// Load the store; a missing or unreadable file is an empty store.
    pub fn load(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(text) => Self::from_text(&text),
            Err(_) => Self::default(),
        }
    }

    /// Write the store atomically enough for a config file (temp + rename).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension("tmp");
        fs::write(&temp, self.to_text())?;
        match fs::rename(&temp, path) {
            Ok(()) => Ok(()),
            Err(_) => {
                // Windows will not rename over an existing file.
                fs::copy(&temp, path)?;
                let _ = fs::remove_file(&temp);
                Ok(())
            }
        }
    }

    pub fn find(&self, host_id: &str) -> Option<&PairedHost> {
        self.hosts.iter().find(|host| host.host_id == host_id)
    }

    /// `(token, key)` for a known host, ready for [`crate::pairing::PairingMachine`].
    pub fn credentials(&self, host_id: &str) -> Option<(u64, [u8; KEY_LEN])> {
        self.find(host_id).map(|host| (host.token, host.key))
    }

    /// Insert or replace by `host_id`.
    pub fn upsert(&mut self, host: PairedHost) {
        if let Some(existing) = self
            .hosts
            .iter_mut()
            .find(|existing| existing.host_id == host.host_id)
        {
            *existing = host;
        } else {
            self.hosts.push(host);
        }
    }

    pub fn to_text(&self) -> String {
        let mut text = String::from(STORE_HEADER);
        text.push('\n');
        for host in &self.hosts {
            text.push_str(&format!(
                "{}|{}|{:016x}|{}\n",
                sanitize(&host.host_id),
                sanitize(&host.host_name),
                host.token,
                to_hex(&host.key)
            ));
        }
        text
    }

    pub fn from_text(text: &str) -> Self {
        let mut store = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split('|');
            let (Some(host_id), Some(host_name), Some(token_hex), Some(key_hex)) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            if host_id.is_empty() || parts.next().is_some() {
                continue;
            }
            let Ok(token) = u64::from_str_radix(token_hex, 16) else {
                continue;
            };
            let Some(key_bytes) = from_hex(key_hex) else {
                continue;
            };
            if key_bytes.len() != KEY_LEN {
                continue;
            }
            let mut key = [0u8; KEY_LEN];
            key.copy_from_slice(&key_bytes);
            store.hosts.push(PairedHost {
                host_id: host_id.to_string(),
                host_name: host_name.to_string(),
                token,
                key,
            });
        }
        store
    }
}

/// Where the receiver keeps its paired-host list.
pub fn default_path() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        return PathBuf::from(appdata)
            .join("lowlat-screen")
            .join("paired_hosts.txt");
    }
    if let Ok(config) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(config)
            .join("lowlat-screen")
            .join("paired_hosts.txt");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("lowlat-screen")
        .join("paired_hosts.txt")
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| if c == '|' || c == '\n' || c == '\r' { '_' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(id: &str) -> PairedHost {
        let mut key = [0u8; KEY_LEN];
        for (index, byte) in key.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(7);
        }
        PairedHost {
            host_id: id.into(),
            host_name: format!("{id} laptop"),
            token: 0x0102_0304_0506_0708,
            key,
        }
    }

    #[test]
    fn text_roundtrip() {
        let mut store = PairedStore::default();
        store.upsert(host("host-a"));
        store.upsert(host("host-b"));
        let text = store.to_text();
        assert!(text.starts_with(STORE_HEADER));
        let parsed = PairedStore::from_text(&text);
        assert_eq!(parsed.hosts, store.hosts);
    }

    #[test]
    fn upsert_replaces_by_id() {
        let mut store = PairedStore::default();
        store.upsert(host("host-a"));
        let mut updated = host("host-a");
        updated.token = 99;
        store.upsert(updated.clone());
        assert_eq!(store.hosts.len(), 1);
        assert_eq!(store.find("host-a"), Some(&updated));
        assert_eq!(store.credentials("host-a").unwrap().0, 99);
    }

    #[test]
    fn comments_and_bad_lines_are_ignored() {
        let text = format!(
            "{STORE_HEADER}\n# a comment\n\nbad line\nhost-a|A|0000000000000001|{}\n",
            to_hex(&[0u8; KEY_LEN])
        );
        let store = PairedStore::from_text(&text);
        assert_eq!(store.hosts.len(), 1);
        assert_eq!(store.hosts[0].host_id, "host-a");
    }

    #[test]
    fn missing_store_is_empty() {
        let path = std::env::temp_dir().join("lowlat-store-that-does-not-exist.txt");
        let _ = fs::remove_file(&path);
        assert!(PairedStore::load(&path).hosts.is_empty());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let path = std::env::temp_dir().join(format!(
            "lowlat-store-test-{}.txt",
            std::process::id()
        ));
        let mut store = PairedStore::default();
        store.upsert(host("host-a"));
        store.save(&path).expect("save works");
        let loaded = PairedStore::load(&path);
        assert_eq!(loaded.hosts, store.hosts);
        let _ = fs::remove_file(&path);
    }
}
