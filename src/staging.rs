//! Private work folders for downloads, and a move that never replaces a file.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use windows::core::HSTRING;
use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVE_FILE_FLAGS};

use crate::paths;

const FOLDER_PREFIX: &str = ".cadence-part-";
const REMOVE_TRIES: u32 = 5;
const REMOVE_WAIT: Duration = Duration::from_millis(100);
const PUBLISH_TRIES: u32 = 100;

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A folder that only one job uses. It is deleted with everything in it when
/// the value is dropped, so success, failure and cancel all clean up.
pub struct Staging {
    path: PathBuf,
}

impl Staging {
    /// Creates a new empty folder in `dir`. `dir` should be the folder of the
    /// final file, so the final move stays on one volume.
    pub fn create(dir: &Path) -> io::Result<Self> {
        loop {
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = dir.join(format!("{FOLDER_PREFIX}{}-{n}", std::process::id()));
            // create_dir fails when the name exists, so the folder is always new.
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        // A killed child can hold its files for a short time.
        for _ in 0..REMOVE_TRIES {
            match fs::remove_dir_all(&self.path) {
                Ok(()) => return,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return,
                Err(_) => thread::sleep(REMOVE_WAIT),
            }
        }
    }
}

/// Renames `from` to `to`. Fails with `AlreadyExists` when `to` exists.
fn move_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    let (from, to) = (
        HSTRING::from(from.as_os_str()),
        HSTRING::from(to.as_os_str()),
    );
    // No flags: Windows does not replace `to`, and does not copy across volumes.
    unsafe { MoveFileExW(&from, &to, MOVE_FILE_FLAGS(0)) }
        .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xFFFF))
}

/// Moves `file` next to `target`, with the extension of `file`. The name is
/// `target` when it is free, else the next free `Name (n)` name. A file that
/// exists, or appears while this runs, is never replaced.
pub fn publish(file: &Path, target: &Path) -> io::Result<PathBuf> {
    let invalid = |what: &str| io::Error::new(io::ErrorKind::InvalidInput, what.to_string());
    let dir = target.parent().ok_or_else(|| invalid("no folder"))?;
    let stem = target.file_stem().ok_or_else(|| invalid("no file name"))?;
    let ext = file.extension().ok_or_else(|| invalid("no extension"))?;
    let (stem, ext) = (stem.to_string_lossy(), ext.to_string_lossy());

    for _ in 0..PUBLISH_TRIES {
        let path = paths::unique_path(dir, &stem, &ext, |p| p.exists());
        match move_no_replace(file, &path) {
            Ok(()) => return Ok(path),
            // Another writer took the name after the check. Pick again.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free file name found",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Barrier;

    #[test]
    fn every_staging_folder_is_new_and_removed_with_its_content() {
        let dir = tempfile::tempdir().unwrap();
        let a = Staging::create(dir.path()).unwrap();
        let b = Staging::create(dir.path()).unwrap();
        assert_ne!(a.path(), b.path());
        assert_eq!(a.path().parent(), Some(dir.path()));

        fs::create_dir(a.path().join("nested")).unwrap();
        fs::write(a.path().join("nested").join("x.part"), b"x").unwrap();
        fs::write(b.path().join("y.part"), b"y").unwrap();
        let (a_path, b_path) = (a.path().to_path_buf(), b.path().to_path_buf());

        drop(a);
        assert!(!a_path.exists());
        assert!(b_path.join("y.part").exists());
        drop(b);
        assert!(!b_path.exists());
    }

    #[test]
    fn a_move_never_replaces_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from.mp3");
        let to = dir.path().join("to.mp3");
        fs::write(&from, b"new").unwrap();
        fs::write(&to, b"mine").unwrap();

        let err = move_no_replace(&from, &to).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&to).unwrap(), b"mine");
        assert!(from.exists());

        fs::remove_file(&to).unwrap();
        move_no_replace(&from, &to).unwrap();
        assert_eq!(fs::read(&to).unwrap(), b"new");
        assert!(!from.exists());
    }

    #[test]
    fn publish_uses_the_next_free_name_and_keeps_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("Song.mp3");
        fs::write(&target, b"mine").unwrap();
        fs::write(dir.path().join("Song (1).mp3"), b"mine too").unwrap();
        let file = dir.path().join("work.mp3");
        fs::write(&file, b"new").unwrap();

        let published = publish(&file, &target).unwrap();
        assert_eq!(published, dir.path().join("Song (2).mp3"));
        assert_eq!(fs::read(&published).unwrap(), b"new");
        assert_eq!(fs::read(&target).unwrap(), b"mine");
        assert_eq!(
            fs::read(dir.path().join("Song (1).mp3")).unwrap(),
            b"mine too"
        );
    }

    #[test]
    fn parallel_publishes_to_one_target_all_survive() {
        const JOBS: usize = 8;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("Song.mp3");
        let start = Barrier::new(JOBS);
        let files: Vec<PathBuf> = (0..JOBS)
            .map(|i| {
                let file = dir.path().join(format!("work{i}.mp3"));
                fs::write(&file, format!("job {i}")).unwrap();
                file
            })
            .collect();

        let published: Vec<PathBuf> = thread::scope(|scope| {
            let handles: Vec<_> = files
                .iter()
                .map(|file| {
                    scope.spawn(|| {
                        start.wait();
                        publish(file, &target).unwrap()
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        let distinct: HashSet<&PathBuf> = published.iter().collect();
        assert_eq!(distinct.len(), JOBS);
        for (i, path) in published.iter().enumerate() {
            assert_eq!(fs::read_to_string(path).unwrap(), format!("job {i}"));
        }
    }
}
