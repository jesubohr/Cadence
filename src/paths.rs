//! File name rules for downloads.

use std::path::{Path, PathBuf};

const MAX_STEM_CHARS: usize = 120;
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Turns a title into a safe Windows file name stem.
pub fn sanitize(title: &str, fallback: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let truncated: String = collapsed.chars().take(MAX_STEM_CHARS).collect();
    let mut stem = truncated
        .trim_matches(|c: char| c == '.' || c == ' ')
        .to_string();
    if stem.is_empty() {
        stem = fallback.to_string();
    }
    if RESERVED.contains(&stem.to_ascii_uppercase().as_str()) {
        stem.insert(0, '_');
    }
    stem
}

/// First free path `dir/stem.ext`, then `dir/stem (1).ext`, and so on.
/// `taken` reports paths that are in use for any reason.
pub fn unique_path(dir: &Path, stem: &str, ext: &str, taken: impl Fn(&Path) -> bool) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !taken(&first) {
        return first;
    }
    (1u32..)
        .map(|n| dir.join(format!("{stem} ({n}).{ext}")))
        .find(|path| !taken(path))
        .expect("an unbounded range always yields a free path")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn sanitizes_forbidden_characters() {
        assert_eq!(
            sanitize("AC/DC: Back <in> Black?", "x"),
            "AC_DC_ Back _in_ Black_"
        );
        assert_eq!(sanitize("  ..dots..  ", "x"), "dots");
        assert_eq!(sanitize("a\tb\nc", "x"), "a b c");
    }

    #[test]
    fn handles_empty_reserved_and_long_titles() {
        assert_eq!(sanitize("???", "fallback"), "___");
        assert_eq!(sanitize("", "fallback"), "fallback");
        assert_eq!(sanitize("con", "x"), "_con");
        assert_eq!(
            sanitize(&"é".repeat(300), "x").chars().count(),
            MAX_STEM_CHARS
        );
    }

    #[test]
    fn picks_first_free_name() {
        let dir = Path::new("D:\\Music");
        let taken: HashSet<PathBuf> = [dir.join("Song.mp3"), dir.join("Song (1).mp3")]
            .into_iter()
            .collect();
        let path = unique_path(dir, "Song", "mp3", |p| taken.contains(p));
        assert_eq!(path, dir.join("Song (2).mp3"));
        let free = unique_path(dir, "Other", "mp3", |p| taken.contains(p));
        assert_eq!(free, dir.join("Other.mp3"));
    }
}
