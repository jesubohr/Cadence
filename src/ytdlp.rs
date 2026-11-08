//! Everything that talks to the yt-dlp executable.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use url::Url;

use crate::media::{Entry, Format, Resolved};
use crate::process::{self, Cancel};
use crate::queue::{Job, Progress, Runner, Update};
use crate::staging::{self, Staging};
use crate::tools::{self, Tools};

pub const SEARCH_LIMIT: usize = 15;
const PLAYLIST_LIMIT: usize = 50;

const PROGRESS_PREFIX: &str = "CADENCE_PROGRESS ";
const POST_PREFIX: &str = "CADENCE_POST ";
const FILE_PREFIX: &str = "CADENCE_FILE ";

/// File name stem of every yt-dlp output inside the job's staging folder.
const OUTPUT_STEM: &str = "media";

/// Share of the bar used by the first downloaded stream. The rest is for the
/// following streams (the audio of a video). Converting then ends the bar.
const FIRST_STREAM_SHARE: f32 = 0.92;
const LAST_STREAM_END: f32 = 0.99;

/// What the user typed in the input field.
#[derive(Debug, Clone, PartialEq)]
pub enum Query {
    Url(String),
    Search(String),
}

impl Query {
    pub fn parse(input: &str) -> Option<Query> {
        let input = input.trim();
        if input.is_empty() {
            return None;
        }
        match Url::parse(input) {
            Ok(url) if matches!(url.scheme(), "http" | "https") && url.host().is_some() => {
                Some(Query::Url(input.to_string()))
            }
            _ => Some(Query::Search(input.to_string())),
        }
    }
}

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

/// Arguments that every yt-dlp call needs.
fn common_args(tools: &Tools) -> Vec<OsString> {
    let mut args = os(&["--ignore-config", "--no-playlist"]);
    args.push("--js-runtimes".into());
    args.push(format!("deno:{}", tools.deno.display()).into());
    args
}

pub fn resolve_args(tools: &Tools, query: &Query) -> Vec<OsString> {
    let mut args = common_args(tools);
    args.extend(os(&["--no-warnings", "--flat-playlist", "-J"]));
    match query {
        Query::Url(url) => {
            args.extend(os(&["--playlist-end", &PLAYLIST_LIMIT.to_string(), "--"]));
            args.push(url.into());
        }
        Query::Search(text) => {
            args.push("--".into());
            args.push(format!("ytsearch{SEARCH_LIMIT}:{text}").into());
        }
    }
    args
}

/// Escapes `%` so yt-dlp does not read it as an output template field.
fn escape_template(text: &str) -> String {
    text.replace('%', "%%")
}

/// Arguments for one download. yt-dlp writes all its files into `staging`.
pub fn download_args(tools: &Tools, job: &Job, staging: &Path) -> Vec<OsString> {
    let mut args = common_args(tools);
    args.extend(os(&["--newline", "--no-mtime", "--ffmpeg-location"]));
    args.push(tools.ffmpeg_dir().into());

    match job.format {
        Format::Mp3 { kbps } => {
            args.extend(os(&[
                "-f",
                "bestaudio/best",
                "-x",
                "--audio-format",
                "mp3",
                "--audio-quality",
            ]));
            args.push(format!("{kbps}K").into());
            args.extend(os(&[
                "--embed-metadata",
                "--embed-thumbnail",
                "--convert-thumbnails",
                "jpg",
            ]));
        }
        Format::Video { max_height } => {
            let cap = max_height
                .map(|h| format!("[height<={h}]"))
                .unwrap_or_default();
            args.push("-f".into());
            args.push(format!("bv*{cap}+ba/b{cap}").into());
            args.extend(os(&[
                "-S",
                "res,vcodec:h264,acodec:aac",
                "--merge-output-format",
                "mp4",
                "--embed-metadata",
            ]));
        }
    }

    args.extend(os(&[
        "--progress",
        "--progress-template",
        "download:CADENCE_PROGRESS %(progress)j",
        "--progress-template",
        "postprocess:CADENCE_POST %(progress.postprocessor)s|%(progress.status)s",
        "--print",
        "after_move:CADENCE_FILE %(filepath)s",
        "-o",
    ]));
    let base = staging.join(OUTPUT_STEM);
    let template = format!("{}.%(ext)s", escape_template(&base.to_string_lossy()));
    args.push(template.into());
    args.push("--".into());
    args.push(job.entry.url.clone().into());
    args
}

/// Smallest listed thumbnail, which is enough for a 56 px preview.
fn thumbnail(value: &Value) -> Option<String> {
    let smallest = value
        .get("thumbnails")
        .and_then(Value::as_array)
        .and_then(|list| {
            list.iter()
                .filter(|t| t.get("url").is_some())
                .min_by_key(|t| t.get("width").and_then(Value::as_u64).unwrap_or(u64::MAX))
        })
        .and_then(|t| t.get("url"))
        .and_then(Value::as_str);
    smallest
        .or_else(|| value.get("thumbnail").and_then(Value::as_str))
        .map(str::to_string)
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn entry(value: &Value) -> Option<Entry> {
    let id = text(value, "id").unwrap_or_default();
    let url = text(value, "webpage_url")
        .or_else(|| text(value, "url"))
        .or_else(|| text(value, "original_url"))?;
    Some(Entry {
        title: text(value, "title").unwrap_or_else(|| url.clone()),
        channel: text(value, "channel").or_else(|| text(value, "uploader")),
        duration_secs: value
            .get("duration")
            .and_then(Value::as_f64)
            .map(|d| d as u64),
        thumbnail: thumbnail(value),
        id,
        url,
    })
}

/// Parses the JSON that `yt-dlp -J --flat-playlist` prints.
pub fn parse_resolved(json: &str) -> Result<Resolved, String> {
    let root: Value = serde_json::from_str(json).map_err(|e| format!("Unreadable result: {e}"))?;

    if root.get("_type").and_then(Value::as_str) == Some("playlist") {
        let entries: Vec<Entry> = root
            .get("entries")
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(entry).collect())
            .unwrap_or_default();
        return Ok(Resolved {
            entries,
            heights: Vec::new(),
        });
    }

    let single = entry(&root).ok_or("The result has no usable URL")?;
    let mut heights: Vec<u32> = root
        .get("formats")
        .and_then(Value::as_array)
        .map(|formats| {
            formats
                .iter()
                .filter(|f| f.get("vcodec").and_then(Value::as_str) != Some("none"))
                .filter_map(|f| f.get("height").and_then(Value::as_u64))
                .map(|h| h as u32)
                .collect()
        })
        .unwrap_or_default();
    heights.sort_unstable_by(|a, b| b.cmp(a));
    heights.dedup();
    Ok(Resolved {
        entries: vec![single],
        heights,
    })
}

/// Picks the most useful error line from yt-dlp's stderr.
pub fn error_message(stderr: &[String]) -> String {
    stderr
        .iter()
        .rev()
        .find(|line| line.starts_with("ERROR"))
        .or_else(|| stderr.last())
        .map(|line| line.trim_start_matches("ERROR: ").to_string())
        .unwrap_or_else(|| "yt-dlp stopped without a message".to_string())
}

/// Resolves a URL or search text into entries. Blocks until yt-dlp ends.
pub fn resolve(tools: &Tools, query: &Query, cancel: &Cancel) -> Result<Resolved, String> {
    let cmd = process::command(&tools.ytdlp, &resolve_args(tools, query));
    let mut json = String::new();
    let finished = process::run(cmd, cancel, |line| {
        // The JSON is a single line. Other lines are not part of it.
        if line.starts_with('{') {
            json = line.to_string();
        }
    })
    .map_err(|e| format!("Could not start yt-dlp: {e}"))?;

    if cancel.is_cancelled() {
        return Err("Cancelled".to_string());
    }
    if !finished.success || json.is_empty() {
        return Err(error_message(&finished.stderr));
    }
    parse_resolved(&json)
}

/// Runs queue jobs with yt-dlp. Tools are located for every run, so a fresh
/// setup works without restarting the app.
pub struct YtDlpRunner;

impl Runner for YtDlpRunner {
    fn run(
        &self,
        job: &Job,
        cancel: &Cancel,
        report: &mut dyn FnMut(Update),
    ) -> Result<PathBuf, String> {
        let tools = tools::locate().map_err(|missing| {
            let names: Vec<&str> = missing.iter().map(|t| t.name()).collect();
            format!("Missing {}. Run scripts\\setup-deps.ps1.", names.join(", "))
        })?;
        download(&tools, job, cancel, report)
    }
}

/// One parsed output line of a download.
#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    Downloading {
        downloaded: u64,
        total: Option<u64>,
        speed: Option<f64>,
        eta: Option<u64>,
        filename: String,
    },
    /// A post-processing step began.
    Converting,
    File(String),
    Other,
}

pub fn parse_line(line: &str) -> Line {
    if let Some(json) = line.strip_prefix(PROGRESS_PREFIX) {
        let Ok(value) = serde_json::from_str::<Value>(json) else {
            return Line::Other;
        };
        if value.get("status").and_then(Value::as_str) != Some("downloading") {
            return Line::Other;
        }
        let number = |key: &str| value.get(key).and_then(Value::as_f64);
        return Line::Downloading {
            downloaded: number("downloaded_bytes").unwrap_or(0.0) as u64,
            total: number("total_bytes")
                .or_else(|| number("total_bytes_estimate"))
                .map(|t| t as u64),
            speed: number("speed"),
            eta: number("eta").map(|e| e as u64),
            filename: text(&value, "filename").unwrap_or_default(),
        };
    }
    if let Some(rest) = line.strip_prefix(POST_PREFIX) {
        // The thumbnail conversion runs before the download starts.
        return match rest.split_once('|') {
            Some((name, "started")) if name != "ThumbnailsConvertor" => Line::Converting,
            _ => Line::Other,
        };
    }
    match line.strip_prefix(FILE_PREFIX) {
        Some(path) => Line::File(path.to_string()),
        None => Line::Other,
    }
}

/// Turns per-stream byte counts into one steady fraction for the whole job.
#[derive(Default)]
pub struct ProgressTracker {
    stream: u32,
    filename: Option<String>,
    fraction: f32,
}

impl ProgressTracker {
    pub fn update(&mut self, line: &Line) -> Option<Progress> {
        let Line::Downloading {
            downloaded,
            total,
            speed,
            eta,
            filename,
        } = line
        else {
            return None;
        };
        match &self.filename {
            Some(previous) if previous != filename => self.stream += 1,
            _ => {}
        }
        self.filename = Some(filename.clone());

        let within = total
            .filter(|t| *t > 0)
            .map(|t| (*downloaded as f32 / t as f32).clamp(0.0, 1.0))
            .unwrap_or(0.0);
        let mapped = if self.stream == 0 {
            within * FIRST_STREAM_SHARE
        } else {
            FIRST_STREAM_SHARE + within * (LAST_STREAM_END - FIRST_STREAM_SHARE)
        };
        self.fraction = self.fraction.max(mapped);
        Some(Progress {
            fraction: self.fraction,
            speed: *speed,
            eta_secs: *eta,
        })
    }
}

/// Runs one download. Returns the path of the finished file.
pub fn download(
    tools: &Tools,
    job: &Job,
    cancel: &Cancel,
    report: &mut dyn FnMut(Update),
) -> Result<PathBuf, String> {
    let target = job.target.clone().ok_or("No target file was planned")?;
    staged_download(&target, |staging| {
        let cmd = process::command(&tools.ytdlp, &download_args(tools, job, staging));
        let mut tracker = ProgressTracker::default();
        let mut reported_file = None;
        let result = process::run(cmd, cancel, |text| match parse_line(text) {
            line @ Line::Downloading { .. } => {
                if let Some(progress) = tracker.update(&line) {
                    report(Update::Downloading(progress));
                }
            }
            Line::Converting => report(Update::Converting),
            Line::File(path) => reported_file = Some(PathBuf::from(path)),
            Line::Other => {}
        });

        match result {
            Ok(_) if cancel.is_cancelled() => Err("Cancelled".to_string()),
            Ok(finished) if finished.success => Ok(reported_file),
            Ok(finished) => Err(error_message(&finished.stderr)),
            Err(e) => Err(format!("Could not start yt-dlp: {e}")),
        }
    })
}

/// Gives `run` a private folder next to `target`, then moves the finished
/// file to `target` without replacing anything. `run` returns the file that
/// yt-dlp reported, if any. The folder is deleted on every exit and no other
/// file is touched, so parallel jobs and unrelated files cannot interfere.
fn staged_download(
    target: &Path,
    run: impl FnOnce(&Path) -> Result<Option<PathBuf>, String>,
) -> Result<PathBuf, String> {
    let dir = target.parent().ok_or("The target file has no folder")?;
    fs::create_dir_all(dir).map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
    let staging = Staging::create(dir)
        .map_err(|e| format!("Cannot create a work folder in {}: {e}", dir.display()))?;

    let reported = run(staging.path())?;
    let produced = find_output(staging.path(), target, reported)
        .ok_or("The download finished but the file is missing")?;
    staging::publish(&produced, target)
        .map_err(|e| format!("Cannot save {}: {e}", target.display()))
}

/// The finished file in the staging folder. It has the extension of `target`,
/// unless yt-dlp had to keep the format of the source.
fn find_output(staging: &Path, target: &Path, reported: Option<PathBuf>) -> Option<PathBuf> {
    let expected = target
        .extension()
        .map(|ext| staging.join(OUTPUT_STEM).with_extension(ext));
    [expected, reported.filter(|p| p.starts_with(staging))]
        .into_iter()
        .flatten()
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::{Job, State};
    use std::sync::Barrier;
    use std::thread;

    fn tools() -> Tools {
        Tools {
            ytdlp: "C:\\t\\yt-dlp.exe".into(),
            ffmpeg: "C:\\t\\ffmpeg.exe".into(),
            ffprobe: "C:\\t\\ffprobe.exe".into(),
            deno: "C:\\t\\deno.exe".into(),
        }
    }

    fn job(format: Format) -> Job {
        Job {
            id: 1,
            entry: Entry {
                id: "abc".into(),
                title: "100% Song".into(),
                url: "https://example.com/watch?v=abc".into(),
                channel: None,
                duration_secs: None,
                thumbnail: None,
            },
            format,
            dir: "D:\\Music".into(),
            state: State::Queued,
            progress: Progress::default(),
            target: Some("D:\\Music\\100% Song.mp3".into()),
            output: None,
            error: None,
        }
    }

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().to_string())
            .collect()
    }

    #[test]
    fn classifies_input() {
        assert_eq!(
            Query::parse(" https://youtu.be/abc "),
            Some(Query::Url("https://youtu.be/abc".into()))
        );
        assert_eq!(
            Query::parse("daft punk"),
            Some(Query::Search("daft punk".into()))
        );
        assert_eq!(
            Query::parse("file:///c:/x"),
            Some(Query::Search("file:///c:/x".into()))
        );
        assert_eq!(Query::parse("   "), None);
    }

    #[test]
    fn search_is_flat_and_limited_to_fifteen() {
        let args = strings(&resolve_args(&tools(), &Query::Search("-x evil".into())));
        let query_index = args.iter().position(|a| a == "ytsearch15:-x evil").unwrap();
        // The query comes after "--" and stays one argument.
        assert_eq!(args[query_index - 1], "--");
        assert!(args.contains(&"--flat-playlist".to_string()));
        assert!(args.contains(&"deno:C:\\t\\deno.exe".to_string()));
    }

    #[test]
    fn mp3_download_args() {
        let args = strings(&download_args(
            &tools(),
            &job(Format::Mp3 { kbps: 256 }),
            Path::new("D:\\100% Music\\.cadence-part-1-0"),
        ));
        let pair = |a: &str, b: &str| args.windows(2).any(|w| w[0] == a && w[1] == b);
        assert!(pair("--audio-quality", "256K"));
        assert!(pair("--audio-format", "mp3"));
        assert!(pair("--ffmpeg-location", "C:\\t"));
        assert!(pair(
            "-o",
            "D:\\100%% Music\\.cadence-part-1-0\\media.%(ext)s"
        ));
        assert_eq!(args.last().unwrap(), "https://example.com/watch?v=abc");
        assert_eq!(args[args.len() - 2], "--");
    }

    #[test]
    fn video_download_args_cap_the_height() {
        let capped = strings(&download_args(
            &tools(),
            &job(Format::Video {
                max_height: Some(720),
            }),
            Path::new("D:\\Videos\\.cadence-part-1-0"),
        ));
        assert!(capped.contains(&"bv*[height<=720]+ba/b[height<=720]".to_string()));
        assert!(capped.contains(&"mp4".to_string()));

        let best = strings(&download_args(
            &tools(),
            &job(Format::Video { max_height: None }),
            Path::new("D:\\Videos\\.cadence-part-1-0"),
        ));
        assert!(best.contains(&"bv*+ba/b".to_string()));
    }

    const SEARCH_JSON: &str = r#"{"_type":"playlist","entries":[
        {"id":"a1","url":"https://www.youtube.com/watch?v=a1","title":"First","duration":242.0,
         "channel":"Chan","thumbnails":[{"url":"https://i/big.jpg","width":720},{"url":"https://i/small.jpg","width":360}]},
        null,
        {"id":"b2","url":"https://www.youtube.com/watch?v=b2","title":"Second","uploader":"Up"}]}"#;

    #[test]
    fn parses_search_results() {
        let resolved = parse_resolved(SEARCH_JSON).unwrap();
        assert_eq!(resolved.entries.len(), 2);
        let first = &resolved.entries[0];
        assert_eq!(first.title, "First");
        assert_eq!(first.duration_secs, Some(242));
        assert_eq!(first.thumbnail.as_deref(), Some("https://i/small.jpg"));
        assert_eq!(resolved.entries[1].channel.as_deref(), Some("Up"));
        assert!(resolved.heights.is_empty());
    }

    #[test]
    fn parses_single_video_heights() {
        let json = r#"{"id":"v","title":"T","webpage_url":"https://x/v","thumbnail":"https://x/t.jpg",
            "formats":[{"vcodec":"none","height":null},{"vcodec":"avc1","height":720},
            {"vcodec":"vp9","height":1080},{"vcodec":"avc1","height":720},{"vcodec":"vp9","height":360}]}"#;
        let resolved = parse_resolved(json).unwrap();
        assert_eq!(resolved.heights, vec![1080, 720, 360]);
        assert_eq!(resolved.entries[0].url, "https://x/v");
        assert_eq!(
            resolved.entries[0].thumbnail.as_deref(),
            Some("https://x/t.jpg")
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_resolved("nope").is_err());
        assert!(parse_resolved("{}").is_err());
    }

    #[test]
    fn parses_progress_lines() {
        let line = r#"CADENCE_PROGRESS {"status": "downloading", "downloaded_bytes": 50, "total_bytes_estimate": 200, "filename": "a.webm", "speed": 10.5, "eta": null}"#;
        assert_eq!(
            parse_line(line),
            Line::Downloading {
                downloaded: 50,
                total: Some(200),
                speed: Some(10.5),
                eta: None,
                filename: "a.webm".into()
            }
        );
        // A finished stream is not progress.
        assert_eq!(
            parse_line(r#"CADENCE_PROGRESS {"status": "finished"}"#),
            Line::Other
        );
    }

    #[test]
    fn converting_ignores_the_early_thumbnail_step() {
        assert_eq!(
            parse_line("CADENCE_POST ThumbnailsConvertor|started"),
            Line::Other
        );
        assert_eq!(
            parse_line("CADENCE_POST ExtractAudio|started"),
            Line::Converting
        );
        assert_eq!(parse_line("CADENCE_POST Merger|started"), Line::Converting);
        assert_eq!(parse_line("CADENCE_POST Merger|finished"), Line::Other);
        assert_eq!(
            parse_line("CADENCE_FILE D:\\Music\\a.mp3"),
            Line::File("D:\\Music\\a.mp3".into())
        );
        assert_eq!(parse_line("[download] Destination: x"), Line::Other);
    }

    #[test]
    fn tracker_never_goes_backwards_across_streams() {
        let mut tracker = ProgressTracker::default();
        let line = |name: &str, done: u64| Line::Downloading {
            downloaded: done,
            total: Some(100),
            speed: None,
            eta: None,
            filename: name.into(),
        };
        let first_end = tracker.update(&line("v.mp4", 100)).unwrap().fraction;
        assert!((first_end - FIRST_STREAM_SHARE).abs() < 1e-6);
        let second_start = tracker.update(&line("a.m4a", 1)).unwrap().fraction;
        assert!(second_start >= first_end);
        let second_end = tracker.update(&line("a.m4a", 100)).unwrap().fraction;
        assert!((second_end - LAST_STREAM_END).abs() < 1e-6);
    }

    #[test]
    fn error_message_prefers_error_lines() {
        let lines = vec![
            "WARNING: old".to_string(),
            "ERROR: [youtube] abc: Video unavailable".to_string(),
            "tail".to_string(),
        ];
        assert_eq!(error_message(&lines), "[youtube] abc: Video unavailable");
        assert_eq!(error_message(&["only".to_string()]), "only");
        assert!(!error_message(&[]).is_empty());
    }

    /// Names of everything in `dir`, sorted.
    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        names
    }

    /// Writes the files yt-dlp leaves in its output folder. Every job uses
    /// the same names, as two jobs with the same title would.
    fn write_intermediates(staging: &Path, who: &str) {
        for name in [
            "media.f251.webm",
            "media.webm.part",
            "media.webp",
            "media.jpg",
        ] {
            fs::write(staging.join(name), who).unwrap();
        }
    }

    #[test]
    fn same_title_audio_and_video_jobs_do_not_share_files() {
        let dir = tempfile::tempdir().unwrap();
        let audio_target = dir.path().join("Song.mp3");
        let video_target = dir.path().join("Song.mp4");
        let both_have_written = Barrier::new(2);

        let (audio, video) = thread::scope(|scope| {
            let video = scope.spawn(|| {
                staged_download(&video_target, |staging| {
                    write_intermediates(staging, "video");
                    fs::write(staging.join("media.mp4"), "video").unwrap();
                    both_have_written.wait();
                    // The audio job ends here. Its cleanup must not reach this folder.
                    both_have_written.wait();
                    for name in [
                        "media.f251.webm",
                        "media.webm.part",
                        "media.webp",
                        "media.jpg",
                    ] {
                        assert_eq!(fs::read_to_string(staging.join(name)).unwrap(), "video");
                    }
                    Ok(None)
                })
            });
            let audio = staged_download(&audio_target, |staging| {
                write_intermediates(staging, "audio");
                fs::write(staging.join("media.mp3"), "audio").unwrap();
                both_have_written.wait();
                Ok(None)
            });
            both_have_written.wait();
            (audio, video.join().unwrap())
        });

        assert_eq!(audio.unwrap(), audio_target);
        assert_eq!(video.unwrap(), video_target);
        assert_eq!(fs::read_to_string(&audio_target).unwrap(), "audio");
        assert_eq!(fs::read_to_string(&video_target).unwrap(), "video");
        // Intermediates are gone and no work folder is left.
        assert_eq!(listing(dir.path()), ["Song.mp3", "Song.mp4"]);
    }

    #[test]
    fn cancelling_a_job_keeps_other_jobs_and_unrelated_files() {
        let dir = tempfile::tempdir().unwrap();
        // Fresh files that share the name of the target. The old cleanup removed them.
        let unrelated = [
            "Song.jpg",
            "Song.f251.webm",
            "Song.webm.part",
            "Song.temp.mp3",
            "Song (1).mp3",
            "Songbook.txt",
        ];
        for name in unrelated {
            fs::write(dir.path().join(name), "keep").unwrap();
        }
        let cancelled_target = dir.path().join("Song.mp3");
        let running_target = dir.path().join("Song.mp4");
        let running_has_written = Barrier::new(2);
        let cancel_is_done = Barrier::new(2);

        let (cancelled, running) = thread::scope(|scope| {
            let running = scope.spawn(|| {
                staged_download(&running_target, |staging| {
                    write_intermediates(staging, "running");
                    running_has_written.wait();
                    cancel_is_done.wait();
                    for name in [
                        "media.f251.webm",
                        "media.webm.part",
                        "media.webp",
                        "media.jpg",
                    ] {
                        assert_eq!(fs::read_to_string(staging.join(name)).unwrap(), "running");
                    }
                    fs::write(staging.join("media.mp4"), "video").unwrap();
                    Ok(None)
                })
            });
            let cancelled = staged_download(&cancelled_target, |staging| {
                write_intermediates(staging, "cancelled");
                running_has_written.wait();
                Err("Cancelled".to_string())
            });
            cancel_is_done.wait();
            (cancelled, running.join().unwrap())
        });

        assert_eq!(cancelled, Err("Cancelled".to_string()));
        assert_eq!(running.unwrap(), running_target);
        assert!(!cancelled_target.exists());
        assert_eq!(fs::read_to_string(&running_target).unwrap(), "video");
        for name in unrelated {
            assert_eq!(fs::read_to_string(dir.path().join(name)).unwrap(), "keep");
        }
        assert_eq!(listing(dir.path()).len(), unrelated.len() + 1);
    }

    #[test]
    fn a_failed_job_removes_its_own_folder() {
        let dir = tempfile::tempdir().unwrap();
        let result = staged_download(&dir.path().join("Song.mp3"), |staging| {
            write_intermediates(staging, "x");
            Err("boom".to_string())
        });
        assert_eq!(result, Err("boom".to_string()));
        assert!(listing(dir.path()).is_empty());

        let missing = staged_download(&dir.path().join("Song.mp3"), |_| Ok(None));
        assert!(missing.is_err());
        assert!(listing(dir.path()).is_empty());
    }

    #[test]
    fn a_file_that_appears_during_the_download_is_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("Song.mp3");
        // The target was free when the job was planned. Someone creates it later.
        let result = staged_download(&target, |staging| {
            fs::write(&target, "external").unwrap();
            fs::write(staging.join("media.mp3"), "download").unwrap();
            Ok(None)
        });

        let published = result.unwrap();
        assert_eq!(published, dir.path().join("Song (1).mp3"));
        assert_eq!(fs::read_to_string(&target).unwrap(), "external");
        assert_eq!(fs::read_to_string(&published).unwrap(), "download");
        assert_eq!(listing(dir.path()), ["Song (1).mp3", "Song.mp3"]);
    }

    #[test]
    fn the_returned_path_is_the_file_that_was_written() {
        let dir = tempfile::tempdir().unwrap();
        // yt-dlp keeps the source format when it has nothing to merge.
        let target = dir.path().join("Clip.mp4");
        let result = staged_download(&target, |staging| {
            let webm = staging.join("media.webm");
            fs::write(&webm, "video").unwrap();
            Ok(Some(webm))
        });
        assert_eq!(result.unwrap(), dir.path().join("Clip.webm"));
        assert_eq!(listing(dir.path()), ["Clip.webm"]);

        // A reported file outside the work folder is never moved.
        let outside = dir.path().join("Clip.webm");
        let result = staged_download(&target, |_| Ok(Some(outside.clone())));
        assert!(result.is_err());
        assert!(outside.exists());
    }
}
