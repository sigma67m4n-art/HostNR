use crate::hosts::{Hosts, MAX_BYTES};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, fs::{self, File}, io::{Read, Write}, path::{Path, PathBuf}, time::{SystemTime, UNIX_EPOCH}};

pub fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub url: Option<String>,
    pub digest: String,
    pub records: usize,
    pub wildcards: usize,
    pub updated: u64,
    pub etag: Option<String>,
    pub modified: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub time: u64,
    pub action: String,
    pub snapshot: Option<Profile>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub profiles: Vec<Profile>,
    pub selected: Option<String>,
    pub history: Vec<Event>,
    pub dynamic: bool,
}

pub struct Store {
    root: PathBuf,
    pub state: State,
}

pub fn atomic_write(path: &Path, data: &[u8]) -> Result<(), String> {
    let temp = path.with_extension("pending");
    let mut file = File::create(&temp).map_err(|e| e.to_string())?;
    file.write_all(data).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    fs::rename(temp, path).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        File::open(parent).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

impl Store {
    pub fn open(root: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(root.join("cache")).map_err(|e| e.to_string())?;
        let path = root.join("library.json");
        let state = if path.exists() {
            let file = File::open(path).map_err(|e| e.to_string())?;
            serde_json::from_reader(file.take(2 * 1024 * 1024))
                .map_err(|e| format!("Cannot read the library. Existing data has not been overwritten: {e}"))?
        } else { State { dynamic: true, ..State::default() } };
        Ok(Self { root, state })
    }

    fn blob(&self, digest: &str) -> Result<PathBuf, String> {
        if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Invalid cache ID".into());
        }
        Ok(self.root.join("cache").join(format!("{digest}.hosts")))
    }

    pub fn text(&self, profile: &Profile) -> Result<String, String> {
        let mut bytes = Vec::new();
        File::open(self.blob(&profile.digest)?).map_err(|e| e.to_string())?
            .take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_BYTES || format!("{:x}", Sha256::digest(&bytes)) != profile.digest {
            return Err("Cache integrity check failed; import the source again".into());
        }
        String::from_utf8(bytes).map_err(|_| "File is not UTF-8".into())
    }

    pub fn selected(&self) -> Option<&Profile> {
        let id = self.state.selected.as_ref()?;
        self.state.profiles.iter().find(|p| &p.id == id)
    }

    fn commit(&mut self, mut state: State, action: &str, snapshot: Option<Profile>) -> Result<(), String> {
        state.history.insert(0, Event { time: now(), action: action.into(), snapshot });
        state.history.truncate(100);
        atomic_write(&self.root.join("library.json"), &serde_json::to_vec_pretty(&state).map_err(|e| e.to_string())?)?;
        self.state = state;
        self.gc();
        Ok(())
    }

    /* Keep library profiles and history snapshots; remove unreferenced blobs. */
    fn gc(&self) {
        let retained: HashSet<&str> = self.state.profiles.iter().map(|p| p.digest.as_str())
            .chain(self.state.history.iter().filter_map(|e| e.snapshot.as_ref().map(|p| p.digest.as_str()))).collect();
        if let Ok(entries) = fs::read_dir(self.root.join("cache")) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("hosts") {
                    if let Some(digest) = path.file_stem().and_then(|s| s.to_str()) {
                        if !retained.contains(digest) { let _ = fs::remove_file(path); }
                    }
                }
            }
        }
    }

    pub fn import(&mut self, name: String, url: Option<String>, text: &str, replace: Option<&str>, etag: Option<String>, modified: Option<String>) -> Result<Profile, String> {
        let parsed = Hosts::parse(text)?;
        if replace.is_none() && self.state.profiles.len() >= 24 { return Err("Maximum 24 profiles; remove a profile first".into()); }
        if replace.is_some_and(|id| !self.state.profiles.iter().any(|p| p.id == id)) { return Err("Profile not found".into()); }
        if url.as_ref().is_some_and(|s| s.len() > 4096) { return Err("URL is too long".into()); }
        let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
        let id = replace.map(str::to_owned).unwrap_or_else(|| {
            let mut sequence = 0;
            loop {
                let candidate = format!("{}-{}-{sequence}", now(), &digest[..12]);
                if !self.state.profiles.iter().any(|p| p.id == candidate) { break candidate; }
                sequence += 1;
            }
        });
        let name: String = name.trim().chars().filter(|c| !c.is_control()).take(80).collect();
        let profile = Profile { id, name: if name.is_empty() { "Saved hosts".into() } else { name }, url, digest, records: parsed.records, wildcards: parsed.wildcards, updated: now(),
            etag: etag.filter(|s| s.len() <= 1024), modified: modified.filter(|s| s.len() <= 128) };
        atomic_write(&self.blob(&profile.digest)?, text.as_bytes())?;
        let mut state = self.state.clone();
        if let Some(old) = state.profiles.iter_mut().find(|p| p.id == profile.id) {
            let previous = std::mem::replace(old, profile.clone());
            if previous.digest != profile.digest {
                state.history.insert(0, Event { time: now(), action: "Previous version".into(), snapshot: Some(previous) });
            }
        }
        else { state.profiles.push(profile.clone()); }
        if state.selected.is_none() { state.selected = Some(profile.id.clone()); }
        self.commit(state, if replace.is_some() { "Source refreshed" } else { "Hosts saved" }, Some(profile.clone()))?;
        Ok(profile)
    }

    pub fn select(&mut self, id: &str) -> Result<(), String> {
        let profile = self.state.profiles.iter().find(|p| p.id == id).cloned().ok_or("Profile not found")?;
        Hosts::parse(&self.text(&profile)?)?;
        let mut state = self.state.clone(); state.selected = Some(id.into());
        self.commit(state, "Profile loaded", Some(profile))
    }

    pub fn restore(&mut self, profile: &Profile) -> Result<(), String> {
        let text = self.text(profile)?;
        /* Restore into a separate profile without overwriting the current version. */
        let p = self.import(format!("{} (restored)", profile.name), profile.url.clone(), &text, None, None, None)?;
        self.select(&p.id)
    }

    pub fn delete(&mut self, id: &str) -> Result<(), String> {
        let mut state = self.state.clone();
        let snapshot = state.profiles.iter().find(|p| p.id == id).cloned();
        state.profiles.retain(|p| p.id != id);
        if state.selected.as_deref() == Some(id) { state.selected = state.profiles.first().map(|p| p.id.clone()); }
        self.commit(state, "Profile removed", snapshot)
    }

    pub fn record(&mut self, action: &str) -> Result<(), String> {
        self.commit(self.state.clone(), action, self.selected().cloned())
    }

    pub fn dynamic(&mut self, value: bool) -> Result<(), String> {
        let mut state = self.state.clone(); state.dynamic = value;
        self.commit(state, "Theme changed", None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_history_restore_and_corruption() {
        let path = std::env::temp_dir().join(format!("hostnr-test-{}-{}", std::process::id(), now()));
        let mut s = Store::open(path.clone()).unwrap();
        let p = s.import("Test".into(), None, "0.0.0.0 .example.com", None, None, None).unwrap();
        s.import("Test".into(), None, "127.0.0.1 .example.com", Some(&p.id), None, None).unwrap();
        assert_eq!(s.text(&p).unwrap(), "0.0.0.0 .example.com");
        s.delete(&p.id).unwrap();
        s.restore(&p).unwrap();
        assert_eq!(s.text(s.selected().unwrap()).unwrap(), "0.0.0.0 .example.com");
        assert!(s.import("Bad".into(), None, "broken", None, None, None).is_err());
        drop(s);
        let s = Store::open(path.clone()).unwrap();
        assert_eq!(s.state.profiles.len(), 1);
        fs::write(s.blob(&p.digest).unwrap(), "tampered").unwrap();
        assert!(s.text(&p).is_err());
        fs::write(path.join("library.json"), "broken").unwrap();
        assert!(Store::open(path.clone()).is_err());
        fs::remove_dir_all(path).unwrap();
    }
}
