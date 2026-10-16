//! The cache of the last USB read. Over the 2.4 GHz receiver nothing can be read from the
//! keyboard, so the app shows these values instead. The file holds the [`KeyboardView`]
//! as shown over the cable and the protocol string; no serial number, no device path.

use std::io;
use std::path::{Path, PathBuf};

use myhub_core::KeyboardView;
use serde::{Deserialize, Serialize};

/// File name inside the `cache` directory of the app config dir.
const FILE_NAME: &str = "keychron-k2he.json";

/// The last settings read over USB.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyboardCache {
    /// Protocol line of that read, e.g. `VIA 12 / Keychron HE 4`.
    pub protocol: String,
    pub view: KeyboardView,
}

/// `<config dir>/cache/keychron-k2he.json`.
pub fn cache_path(config_dir: &Path) -> PathBuf {
    config_dir.join("cache").join(FILE_NAME)
}

impl KeyboardCache {
    /// Writes the cache atomically: a temp file next to the target, then a rename.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let json = serde_json::to_vec(self).map_err(io::Error::other)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }

    /// Reads the cache; `None` when the file is missing or does not parse.
    pub fn load(path: &Path) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }
}
