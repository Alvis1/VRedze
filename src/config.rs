//! Saved servers (`~/.config/just-video/servers.json`) and their passwords
//! (`credentials.json`, mode 0600 like a mount.cifs credentials file).
//! SteamOS's game-mode session has no reachable Secret Service, so there is no
//! encrypted keystore to use; the file is readable only by its owner.

use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Server {
    pub name: String,
    /// `smb://[domain;]user@host[:port]`, no password.
    pub url: String,
}

pub fn dir() -> anyhow::Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .context("No home directory")?;
    Ok(base.join("just-video"))
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(name: &str) -> anyhow::Result<T> {
    let path = dir()?.join(name);
    match std::fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("Parse {}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e).with_context(|| format!("Read {}", path.display())),
    }
}

/// Writes atomically; `private` files are created with mode 0600.
fn write_json<T: Serialize>(name: &str, value: &T, private: bool) -> anyhow::Result<()> {
    let dir = dir()?;
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let tmp = dir.join(format!(".{name}.tmp"));
    let _ = std::fs::remove_file(&tmp);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(if private { 0o600 } else { 0o644 })
        .open(&tmp)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    std::fs::rename(&tmp, dir.join(name))?;
    Ok(())
}

pub fn servers() -> anyhow::Result<Vec<Server>> {
    read_json("servers.json")
}

pub fn password(url: &str) -> anyhow::Result<Option<String>> {
    let credentials: BTreeMap<String, String> = read_json("credentials.json")?;
    Ok(credentials.get(url).cloned())
}

/// Adds or replaces a server (matched by URL) and stores its password.
pub fn save_server(server: Server, password: &str) -> anyhow::Result<()> {
    let mut list = servers()?;
    list.retain(|s| s.url != server.url);
    let url = server.url.clone();
    list.push(server);
    write_json("servers.json", &list, false)?;
    let mut credentials: BTreeMap<String, String> = read_json("credentials.json")?;
    credentials.insert(url, password.to_string());
    write_json("credentials.json", &credentials, true)
}

/// Removes a server by name or URL, with its password. Returns whether it existed.
pub fn remove_server(name_or_url: &str) -> anyhow::Result<bool> {
    let mut list = servers()?;
    let Some(index) = list
        .iter()
        .position(|s| s.name == name_or_url || s.url == name_or_url)
    else {
        return Ok(false);
    };
    let removed = list.remove(index);
    write_json("servers.json", &list, false)?;
    let mut credentials: BTreeMap<String, String> = read_json("credentials.json")?;
    credentials.remove(&removed.url);
    write_json("credentials.json", &credentials, true)?;
    Ok(true)
}

/// A user's choice of how to show one file (when metadata and name are wrong).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LayoutOverride {
    pub projection: crate::vr::Projection,
    pub stereo: crate::vr::Stereo,
    pub swap_eyes: bool,
}

pub fn layout_override(key: &str) -> anyhow::Result<Option<LayoutOverride>> {
    let all: BTreeMap<String, LayoutOverride> = read_json("layouts.json")?;
    Ok(all.get(key).copied())
}

/// Saves (or with `None`, forgets) the override for a file.
pub fn save_layout_override(key: &str, layout: Option<LayoutOverride>) -> anyhow::Result<()> {
    let mut all: BTreeMap<String, LayoutOverride> = read_json("layouts.json")?;
    let changed = match layout {
        Some(l) => all.insert(key.to_string(), l) != Some(l),
        None => all.remove(key).is_some(),
    };
    if changed {
        write_json("layouts.json", &all, false)?;
    }
    Ok(())
}

pub fn move_layout_override(from: &str, to: &str) -> anyhow::Result<()> {
    if let Some(l) = layout_override(from)? {
        save_layout_override(from, None)?;
        save_layout_override(to, Some(l))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_with_private_credentials() {
        let dir = std::env::temp_dir().join(format!("jv-config-{}", std::process::id()));
        // SAFETY: single-threaded within this test; other tests don't read XDG_CONFIG_HOME.
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &dir) };
        let server = Server {
            name: "PC".into(),
            url: "smb://alice@192.168.1.10".into(),
        };
        save_server(server.clone(), "secret").unwrap();
        assert_eq!(servers().unwrap(), vec![server.clone()]);
        assert_eq!(password(&server.url).unwrap().as_deref(), Some("secret"));
        let mode = std::fs::metadata(dir.join("just-video/credentials.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let key = "smb://alice@192.168.1.10/media/VR/clip.mp4";
        assert_eq!(layout_override(key).unwrap(), None);
        let l = LayoutOverride {
            projection: crate::vr::Projection::Equirect180,
            stereo: crate::vr::Stereo::SideBySide,
            swap_eyes: false,
        };
        save_layout_override(key, Some(l)).unwrap();
        move_layout_override(key, "smb://alice@192.168.1.10/media/VR/renamed.mp4").unwrap();
        assert_eq!(layout_override(key).unwrap(), None);
        assert_eq!(
            layout_override("smb://alice@192.168.1.10/media/VR/renamed.mp4").unwrap(),
            Some(l)
        );
        assert!(remove_server("PC").unwrap());
        assert!(servers().unwrap().is_empty());
        assert_eq!(password(&server.url).unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
