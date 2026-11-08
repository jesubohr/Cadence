//! Thumbnail download and disk cache.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

use crate::storage;

const MAX_BYTES: u64 = 2_000_000;
const MAX_AGE: Duration = Duration::from_secs(14 * 24 * 3600);

fn cache_dir() -> PathBuf {
    storage::data_dir().join("thumbs")
}

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .tls_config(
                TlsConfig::builder()
                    .provider(TlsProvider::NativeTls)
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            )
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .into()
    })
}

fn cache_path(url: &str) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    cache_dir().join(format!("{:016x}.img", hasher.finish()))
}

fn download(url: &str) -> Result<Vec<u8>, ureq::Error> {
    agent()
        .get(url)
        .call()?
        .body_mut()
        .with_config()
        .limit(MAX_BYTES)
        .read_to_vec()
}

/// Returns the cached file, downloading it first when needed. Blocks.
pub fn fetch(url: &str) -> Option<PathBuf> {
    let path = cache_path(url);
    if path.is_file() {
        return Some(path);
    }
    let bytes = download(url)
        .map_err(|err| eprintln!("thumbnail {url}: {err}"))
        .ok()?;
    fs::create_dir_all(cache_dir()).ok()?;
    // Write under another name first so a reader never sees half an image.
    let partial = path.with_extension("part");
    fs::write(&partial, bytes).ok()?;
    fs::rename(&partial, &path).ok()?;
    Some(path)
}

/// Deletes cached thumbnails that are older than two weeks. Blocks.
pub fn prune() {
    let Ok(entries) = fs::read_dir(cache_dir()) else {
        return;
    };
    let limit = SystemTime::now() - MAX_AGE;
    for item in entries.flatten() {
        let old = item
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|modified| modified < limit);
        if old {
            let _ = fs::remove_file(item.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs network access: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn downloads_a_real_thumbnail() {
        let bytes = download("https://i.ytimg.com/vi/K0HSD_i2DvA/mqdefault.jpg").unwrap();
        assert!(bytes.len() > 1000);
    }
}
