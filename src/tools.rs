//! Locates the external programs Cadence needs.

use std::env::consts::EXE_SUFFIX;
use std::path::{Path, PathBuf};

use crate::storage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    YtDlp,
    Ffmpeg,
    Ffprobe,
    Deno,
}

impl Tool {
    pub fn name(self) -> &'static str {
        match self {
            Tool::YtDlp => "yt-dlp",
            Tool::Ffmpeg => "ffmpeg",
            Tool::Ffprobe => "ffprobe",
            Tool::Deno => "deno",
        }
    }

    fn file_name(self) -> String {
        format!("{}{}", self.name(), EXE_SUFFIX)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tools {
    pub ytdlp: PathBuf,
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub deno: PathBuf,
}

impl Tools {
    /// Directory that holds ffmpeg and ffprobe, as yt-dlp expects.
    pub fn ffmpeg_dir(&self) -> &Path {
        self.ffmpeg.parent().unwrap_or(Path::new("."))
    }
}

/// Folders searched before PATH, in priority order.
pub fn bundled_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = std::env::var_os("CADENCE_TOOLS_DIR") {
        dirs.push(PathBuf::from(dir));
    }
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        dirs.push(exe_dir.join("tools"));
    }
    dirs.push(storage::data_dir().join("tools"));
    dirs
}

/// Finds all tools in the bundled folders, then in PATH.
pub fn locate() -> Result<Tools, Vec<Tool>> {
    let mut dirs = bundled_dirs();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    locate_in(&dirs)
}

/// Returns every missing tool when the search fails.
/// ffprobe is only accepted next to ffmpeg because yt-dlp looks for it there.
pub fn locate_in(dirs: &[PathBuf]) -> Result<Tools, Vec<Tool>> {
    let find = |tool: Tool| {
        dirs.iter()
            .map(|dir| dir.join(tool.file_name()))
            .find(|path| path.is_file())
    };
    let ytdlp = find(Tool::YtDlp);
    let ffmpeg = find(Tool::Ffmpeg);
    let ffprobe = ffmpeg
        .as_ref()
        .and_then(|path| path.parent())
        .map(|dir| dir.join(Tool::Ffprobe.file_name()))
        .filter(|path| path.is_file());
    let deno = find(Tool::Deno);

    match (ytdlp, ffmpeg, ffprobe, deno) {
        (Some(ytdlp), Some(ffmpeg), Some(ffprobe), Some(deno)) => Ok(Tools {
            ytdlp,
            ffmpeg,
            ffprobe,
            deno,
        }),
        (ytdlp, ffmpeg, ffprobe, deno) => Err([
            (Tool::YtDlp, ytdlp.is_none()),
            (Tool::Ffmpeg, ffmpeg.is_none()),
            (Tool::Ffprobe, ffprobe.is_none()),
            (Tool::Deno, deno.is_none()),
        ]
        .into_iter()
        .filter_map(|(tool, missing)| missing.then_some(tool))
        .collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(dir: &Path, tool: Tool) {
        fs::write(dir.join(tool.file_name()), b"").unwrap();
    }

    #[test]
    fn reports_all_missing_tools() {
        let dir = tempfile::tempdir().unwrap();
        let missing = locate_in(&[dir.path().to_path_buf()]).unwrap_err();
        assert_eq!(
            missing,
            vec![Tool::YtDlp, Tool::Ffmpeg, Tool::Ffprobe, Tool::Deno]
        );
    }

    #[test]
    fn finds_tools_across_folders() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        touch(a.path(), Tool::YtDlp);
        touch(b.path(), Tool::Ffmpeg);
        touch(b.path(), Tool::Ffprobe);
        touch(b.path(), Tool::Deno);
        let tools = locate_in(&[a.path().to_path_buf(), b.path().to_path_buf()]).unwrap();
        assert_eq!(tools.ffmpeg_dir(), b.path());
        assert_eq!(tools.ytdlp.parent().unwrap(), a.path());
    }

    #[test]
    fn ffprobe_must_sit_next_to_ffmpeg() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        touch(a.path(), Tool::YtDlp);
        touch(a.path(), Tool::Deno);
        touch(a.path(), Tool::Ffmpeg);
        touch(b.path(), Tool::Ffprobe);
        let missing = locate_in(&[a.path().to_path_buf(), b.path().to_path_buf()]).unwrap_err();
        assert_eq!(missing, vec![Tool::Ffprobe]);
    }
}
