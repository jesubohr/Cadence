//! Locations and JSON persistence for application data.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;

/// Folder for config, queue and caches. `CADENCE_DATA_DIR` overrides it.
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CADENCE_DATA_DIR") {
        return PathBuf::from(dir);
    }
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Cadence")
}

/// Reads JSON from `path`. A missing or invalid file yields the default value.
pub fn load_json<T: DeserializeOwned + Default>(path: &Path) -> T {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Writes JSON through a temporary file so a crash never leaves a partial file.
pub fn save_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_default_on_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("v.json");
        save_json(&path, &vec![1u32, 2, 3]).unwrap();
        assert_eq!(load_json::<Vec<u32>>(&path), vec![1, 2, 3]);

        fs::write(&path, b"not json").unwrap();
        assert!(load_json::<Vec<u32>>(&path).is_empty());
        assert!(load_json::<Vec<u32>>(&dir.path().join("missing.json")).is_empty());
    }

    /// Saving over an existing file must replace it. Windows needs a replacing rename for this.
    #[test]
    fn repeated_saves_replace_the_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.json");
        for round in 0u32..5 {
            save_json(&path, &vec![round; 3]).unwrap();
            assert_eq!(load_json::<Vec<u32>>(&path), vec![round; 3]);
        }
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["queue.json"]);
    }
}
