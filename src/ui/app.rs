//! The main window view.

use std::collections::{HashMap, HashSet};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{
    div, ease_in_out, prelude::*, px, rgb, Animation, AnimationExt, AnyElement, Context, Entity,
    FontWeight, PathPromptOptions, Window, WindowControlArea,
};

use super::text_input::{Submit, TextInput};
use super::theme::*;
use super::widgets::*;
use crate::config::Config;
use crate::media::{
    format_duration, format_speed, Entry, Format, Kind, DEFAULT_HEIGHTS, MP3_BITRATES,
};
use crate::process::Cancel;
use crate::queue::{EnqueueError, Job, Manager, State};
use crate::tools::{self, Tool, Tools};
use crate::tray::{Tray, TrayAction};
use crate::win;
use crate::{thumbs, ytdlp};

const NOTICE_SECONDS: u64 = 4;
const MAX_HEIGHT_CHIPS: usize = 5;

/// Everything the view needs from `main`.
pub struct Boot {
    pub config: Config,
    pub tools: Result<Tools, Vec<Tool>>,
    pub manager: Manager,
    pub changed: async_channel::Receiver<()>,
    pub tray: Option<Tray>,
    pub tray_actions: async_channel::Receiver<TrayAction>,
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Results,
    Queue,
}

enum Lookup {
    Idle,
    Busy(Cancel),
    Failed(String),
}

struct Row {
    entry: Entry,
    selected: bool,
}

pub struct Cadence {
    input: Entity<TextInput>,
    config: Config,
    tools: Result<Tools, Vec<Tool>>,
    manager: Manager,
    jobs: Vec<Job>,
    tab: Tab,
    settings_open: bool,
    lookup: Lookup,
    lookup_seq: u64,
    results: Vec<Row>,
    /// Heights of a single resolved video. Empty when unknown.
    heights: Vec<u32>,
    thumbs: HashMap<String, PathBuf>,
    thumbs_requested: HashSet<String>,
    notice: Option<(u64, String)>,
    notice_seq: u64,
    window_handle: Option<win::Handle>,
    _tray: Option<Tray>,
}

impl Cadence {
    pub fn new(window: &mut Window, cx: &mut Context<Self>, boot: Boot) -> Self {
        let input = cx.new(|cx| TextInput::new("Link or song title", cx));
        cx.subscribe(&input, |this, _, _: &Submit, cx| this.submit(cx))
            .detach();
        window.focus(&gpui::Focusable::focus_handle(&input, cx));

        let handle = win::Handle::of(window);
        let has_tray = boot.tray.is_some();
        if let Some(handle) = handle {
            handle.set_always_on_top(boot.config.always_on_top);
            // With a tray icon, closing hides the window and downloads go on.
            window.on_window_should_close(cx, move |_, _| {
                if has_tray {
                    handle.hide();
                }
                !has_tray
            });
        }

        let jobs = boot.manager.snapshot();
        let mut this = Self {
            input,
            config: boot.config,
            tools: boot.tools,
            manager: boot.manager,
            jobs,
            tab: Tab::Results,
            settings_open: false,
            lookup: Lookup::Idle,
            lookup_seq: 0,
            results: Vec::new(),
            heights: Vec::new(),
            thumbs: HashMap::new(),
            thumbs_requested: HashSet::new(),
            notice: None,
            notice_seq: 0,
            window_handle: handle,
            _tray: boot.tray,
        };
        this.request_job_thumbs(cx);
        if !this.jobs.is_empty() {
            this.tab = Tab::Queue;
        }

        let changed = boot.changed;
        cx.spawn(async move |this, cx| {
            while changed.recv().await.is_ok() {
                let alive = this.update(cx, |this, cx| {
                    this.jobs = this.manager.snapshot();
                    this.request_job_thumbs(cx);
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();

        let tray_actions = boot.tray_actions;
        cx.spawn(async move |this, cx| {
            while let Ok(action) = tray_actions.recv().await {
                let alive = this.update(cx, |this, cx| match action {
                    TrayAction::Show => {
                        if let Some(handle) = this.window_handle {
                            handle.restore();
                        }
                    }
                    TrayAction::Quit => {
                        this.manager.shutdown();
                        cx.quit();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();

        cx.background_spawn(async { thumbs::prune() }).detach();
        this
    }

    fn save_config(&self, cx: &mut Context<Self>) {
        let config = self.config.clone();
        cx.background_spawn(async move { config.save() }).detach();
    }

    fn show_notice(&mut self, text: String, cx: &mut Context<Self>) {
        self.notice_seq += 1;
        let seq = self.notice_seq;
        self.notice = Some((seq, text));
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs(NOTICE_SECONDS))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.notice.as_ref().is_some_and(|(id, _)| *id == seq) {
                    this.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn request_thumb(&mut self, url: &str, cx: &mut Context<Self>) {
        if !self.thumbs_requested.insert(url.to_string()) {
            return;
        }
        let url = url.to_string();
        cx.spawn(async move |this, cx| {
            let key = url.clone();
            let path = cx
                .background_spawn(async move { thumbs::fetch(&url) })
                .await;
            if let Some(path) = path {
                let _ = this.update(cx, |this, cx| {
                    this.thumbs.insert(key, path);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn request_job_thumbs(&mut self, cx: &mut Context<Self>) {
        let urls: Vec<String> = self
            .jobs
            .iter()
            .filter_map(|job| job.entry.thumbnail.clone())
            .collect();
        for url in urls {
            self.request_thumb(&url, cx);
        }
    }

    fn ready(&self) -> bool {
        self.tools.is_ok()
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let Some(query) = ytdlp::Query::parse(self.input.read(cx).content()) else {
            return;
        };
        let Ok(tools) = self.tools.clone() else {
            return;
        };
        if let Lookup::Busy(previous) = &self.lookup {
            previous.cancel();
        }
        let cancel = Cancel::default();
        self.lookup = Lookup::Busy(cancel.clone());
        self.lookup_seq += 1;
        let seq = self.lookup_seq;
        self.tab = Tab::Results;
        self.settings_open = false;

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { ytdlp::resolve(&tools, &query, &cancel) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.lookup_seq == seq {
                    this.on_resolved(result, cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn on_resolved(
        &mut self,
        result: Result<crate::media::Resolved, String>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Err(message) => self.lookup = Lookup::Failed(message),
            Ok(resolved) if resolved.entries.is_empty() => {
                self.lookup = Lookup::Failed("Nothing found".to_string());
            }
            Ok(resolved) => {
                self.lookup = Lookup::Idle;
                // A single video is ready to download. A list needs a choice.
                let single = resolved.entries.len() == 1;
                self.heights = resolved.heights;
                self.results = resolved
                    .entries
                    .into_iter()
                    .map(|entry| Row {
                        entry,
                        selected: single,
                    })
                    .collect();
                let urls: Vec<String> = self
                    .results
                    .iter()
                    .filter_map(|row| row.entry.thumbnail.clone())
                    .collect();
                for url in urls {
                    self.request_thumb(&url, cx);
                }
            }
        }
        cx.notify();
    }

    fn recheck_tools(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let found = cx.background_spawn(async { tools::locate() }).await;
            let _ = this.update(cx, |this, cx| {
                this.tools = found;
                cx.notify();
            });
        })
        .detach();
    }

    /// Heights offered as chips. Real formats win over the default ladder.
    fn video_options(&self) -> Vec<u32> {
        let source: &[u32] = if self.heights.is_empty() {
            &DEFAULT_HEIGHTS
        } else {
            &self.heights
        };
        source.iter().copied().take(MAX_HEIGHT_CHIPS).collect()
    }

    /// The saved height when it is offered, otherwise the best quality.
    fn selected_height(&self) -> Option<u32> {
        self.config
            .video_height
            .filter(|h| self.video_options().contains(h))
    }

    fn format(&self) -> Format {
        match self.config.kind {
            Kind::Audio => Format::Mp3 {
                kbps: self.config.mp3_kbps,
            },
            Kind::Video => Format::Video {
                max_height: self.selected_height(),
            },
        }
    }

    fn selected_count(&self) -> usize {
        self.results.iter().filter(|row| row.selected).count()
    }

    fn download_selected(&mut self, cx: &mut Context<Self>) {
        let format = self.format();
        let dir = self.config.dir_for(format.kind()).clone();
        let (mut added, mut duplicates) = (0, 0);
        for row in self.results.iter_mut().filter(|row| row.selected) {
            match self.manager.enqueue(row.entry.clone(), format, dir.clone()) {
                Ok(_) => added += 1,
                Err(EnqueueError::Duplicate) => duplicates += 1,
            }
            row.selected = false;
        }
        if added > 0 {
            self.tab = Tab::Queue;
        }
        let message = match (added, duplicates) {
            (0, 0) => return,
            (0, _) => "Already in the queue".to_string(),
            (n, 0) => format!("Added {n}"),
            (n, d) => format!("Added {n}, {d} already in the queue"),
        };
        self.show_notice(message, cx);
    }

    fn choose_folder(&mut self, kind: Kind, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                match kind {
                    Kind::Audio => this.config.audio_dir = path,
                    Kind::Video => this.config.video_dir = path,
                }
                this.save_config(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle_always_on_top(&mut self, cx: &mut Context<Self>) {
        self.config.always_on_top = !self.config.always_on_top;
        if let Some(handle) = self.window_handle {
            handle.set_always_on_top(self.config.always_on_top);
        }
        self.save_config(cx);
        cx.notify();
    }

    fn reveal(&self, path: PathBuf, cx: &mut Context<Self>) {
        cx.background_spawn(async move { reveal_in_explorer(&path) })
            .detach();
    }

    // ---- rendering ----

    fn titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let on_top = self.config.always_on_top;
        div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(40.))
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .px(px(16.))
                    .window_control_area(WindowControlArea::Drag)
                    .text_color(rgb(MUTED))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Cadence"),
            )
            .child(
                icon_button("pin", "pin", on_top)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_always_on_top(cx))),
            )
            .child(
                icon_button("settings", "settings", self.settings_open).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.settings_open = !this.settings_open;
                        cx.notify();
                    },
                )),
            )
            .child(div().w(px(6.)))
            .child(
                div()
                    .id("minimize")
                    .w(px(44.))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(|d| d.bg(rgb(SURFACE_HOVER)))
                    .window_control_area(WindowControlArea::Min)
                    .child(icon("minus", 15., MUTED)),
            )
            .child(
                div()
                    .id("close")
                    .w(px(44.))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(|d| d.bg(rgb(0xc42b1c)))
                    .window_control_area(WindowControlArea::Close)
                    .child(icon("x", 15., MUTED)),
            )
    }

    fn banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let missing = self.tools.as_ref().err()?;
        let names: Vec<&str> = missing.iter().map(|tool| tool.name()).collect();
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(8.))
                .p(px(10.))
                .rounded(px(9.))
                .bg(rgb(SURFACE))
                .border_1()
                .border_color(rgb(ERROR))
                .child(div().flex_1().text_size(px(12.5)).child(format!(
                    "Missing {}. Run scripts\\setup-deps.ps1.",
                    names.join(", ")
                )))
                .child(
                    text_button("recheck", "Check again")
                        .on_click(cx.listener(|this, _, _, cx| this.recheck_tools(cx))),
                )
                .into_any_element(),
        )
    }

    fn options_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let kind = self.config.kind;
        let kind_switch = segmented()
            .child(
                segment("kind-audio", "Audio", kind == Kind::Audio).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.config.kind = Kind::Audio;
                        this.save_config(cx);
                        cx.notify();
                    },
                )),
            )
            .child(
                segment("kind-video", "Video", kind == Kind::Video).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.config.kind = Kind::Video;
                        this.save_config(cx);
                        cx.notify();
                    },
                )),
            );

        let quality = match kind {
            Kind::Audio => segmented().children(MP3_BITRATES.iter().map(|&kbps| {
                segment(
                    ("kbps", kbps),
                    kbps.to_string(),
                    self.config.mp3_kbps == kbps,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.config.mp3_kbps = kbps;
                    this.save_config(cx);
                    cx.notify();
                }))
            })),
            Kind::Video => {
                let selected = self.selected_height();
                segmented()
                    .child(segment("height-best", "Best", selected.is_none()).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.config.video_height = None;
                            this.save_config(cx);
                            cx.notify();
                        }),
                    ))
                    .children(self.video_options().into_iter().map(|height| {
                        segment(
                            ("height", height),
                            format!("{height}p"),
                            selected == Some(height),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.config.video_height = Some(height);
                            this.save_config(cx);
                            cx.notify();
                        }))
                    }))
            }
        };

        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .child(kind_switch)
            .child(quality)
    }

    fn results_view(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.lookup {
            Lookup::Busy(_) => {
                return centered_text("Searching", MUTED)
                    .with_animation(
                        "searching",
                        Animation::new(Duration::from_millis(1100))
                            .repeat()
                            .with_easing(ease_in_out),
                        |el, delta| el.opacity(0.35 + 0.65 * (1.0 - (delta * 2.0 - 1.0).abs())),
                    )
                    .into_any_element();
            }
            Lookup::Failed(message) => return centered_text(message, ERROR).into_any_element(),
            Lookup::Idle => {}
        }

        div()
            .id("results")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(self.results.iter().enumerate().map(|(index, row)| {
                let entry = &row.entry;
                let meta = [
                    entry.channel.clone(),
                    entry.duration_secs.map(format_duration),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
                let thumb = entry
                    .thumbnail
                    .as_ref()
                    .and_then(|url| self.thumbs.get(url));
                div()
                    .id(("result", index))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .p(px(8.))
                    .rounded(px(9.))
                    .cursor_pointer()
                    .when(row.selected, |d| d.bg(rgb(SURFACE)))
                    .hover(|d| d.bg(rgb(SURFACE_HOVER)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(row) = this.results.get_mut(index) {
                            row.selected = !row.selected;
                            cx.notify();
                        }
                    }))
                    .child(checkbox(row.selected))
                    .child(thumbnail(thumb))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(one_line(div()).child(entry.title.clone()))
                            .child(
                                div()
                                    .w_full()
                                    .overflow_hidden()
                                    .line_clamp(1)
                                    .text_ellipsis()
                                    .text_size(px(12.))
                                    .text_color(rgb(MUTED))
                                    .child(meta),
                            ),
                    )
            }))
            .into_any_element()
    }

    fn results_footer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.results.is_empty() {
            return None;
        }
        let count = self.selected_count();
        let all_selected = count == self.results.len();
        let mut download = primary_button("download", format!("Download {count}"), count > 0);
        if count > 0 && self.ready() {
            download = download.on_click(cx.listener(|this, _, _, cx| this.download_selected(cx)));
        }
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_between()
                .when(self.results.len() > 1, |d| {
                    d.child(
                        text_button(
                            "select-all",
                            if all_selected { "Clear" } else { "Select all" },
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            for row in &mut this.results {
                                row.selected = !all_selected;
                            }
                            cx.notify();
                        })),
                    )
                })
                .child(div().flex_1())
                .child(download)
                .into_any_element(),
        )
    }

    fn queue_view(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("queue")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(2.))
            .children(self.jobs.iter().map(|job| self.job_row(job, cx)))
            .into_any_element()
    }

    fn job_row(&self, job: &Job, cx: &mut Context<Self>) -> AnyElement {
        let id = job.id;
        let (status, color) = status_line(job);
        let active = job.state.is_active();
        let thumb = job
            .entry
            .thumbnail
            .as_ref()
            .and_then(|url| self.thumbs.get(url));

        let mut actions = div().flex().flex_none().items_center();
        match job.state {
            State::Queued | State::Downloading | State::Converting => {
                actions = actions.child(
                    icon_button(("cancel", id), "x", false)
                        .on_click(cx.listener(move |this, _, _, _| this.manager.cancel(id))),
                );
            }
            State::Completed => {
                if let Some(path) = job.output.clone() {
                    actions = actions.child(icon_button(("reveal", id), "folder", false).on_click(
                        cx.listener(move |this, _, _, cx| this.reveal(path.clone(), cx)),
                    ));
                }
                actions = actions.child(remove_button(id, cx));
            }
            State::Failed | State::Cancelled => {
                actions = actions
                    .child(
                        icon_button(("retry", id), "retry", false)
                            .on_click(cx.listener(move |this, _, _, _| this.manager.retry(id))),
                    )
                    .child(remove_button(id, cx));
            }
        }

        let bar = match job.state {
            State::Downloading => Some(progress_bar(job.progress.fraction).into_any_element()),
            State::Converting => Some(
                progress_bar(1.0)
                    .with_animation(
                        ("converting", id),
                        Animation::new(Duration::from_millis(1200))
                            .repeat()
                            .with_easing(ease_in_out),
                        |el, delta| el.opacity(0.3 + 0.7 * (1.0 - (delta * 2.0 - 1.0).abs())),
                    )
                    .into_any_element(),
            ),
            _ => None,
        };

        div()
            .id(("job", id))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .p(px(8.))
            .rounded(px(9.))
            .when(active, |d| d.bg(rgb(SURFACE)))
            .child(thumbnail(thumb))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(one_line(div()).child(job.entry.title.clone()))
                    .child(
                        div()
                            .w_full()
                            .overflow_hidden()
                            .line_clamp(1)
                            .text_ellipsis()
                            .text_size(px(12.))
                            .text_color(rgb(color))
                            .child(status),
                    )
                    .children(bar),
            )
            .child(actions)
            .into_any_element()
    }

    fn queue_footer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.jobs
            .iter()
            .any(|job| job.state.is_finished())
            .then(|| {
                div()
                    .flex()
                    .flex_none()
                    .justify_end()
                    .child(
                        text_button("clear", "Clear finished")
                            .on_click(cx.listener(|this, _, _, _| this.manager.clear_finished())),
                    )
                    .into_any_element()
            })
    }

    fn main_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let queue_active = self.jobs.iter().filter(|j| !j.state.is_finished()).count();
        let queue_label = if queue_active > 0 {
            format!("Queue {queue_active}")
        } else {
            "Queue".to_string()
        };
        let ready = self.ready();
        let busy = matches!(self.lookup, Lookup::Busy(_));

        let (content, footer) = match self.tab {
            Tab::Results => (self.results_view(cx), self.results_footer(cx)),
            Tab::Queue => (self.queue_view(cx), self.queue_footer(cx)),
        };

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(12.))
            .px(px(16.))
            .pb(px(16.))
            .children(self.banner(cx))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .gap(px(8.))
                    .child(div().flex_1().child(self.input.clone()))
                    .child(
                        div()
                            .id("search")
                            .flex_none()
                            .size(px(38.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(10.))
                            .bg(rgb(ACCENT))
                            .when(ready, |d| {
                                d.cursor_pointer()
                                    .hover(|d| d.bg(rgb(ACCENT_HOVER)))
                                    .on_click(cx.listener(|this, _, _, cx| this.submit(cx)))
                            })
                            .when(!ready, |d| d.opacity(0.4))
                            .when(busy, |d| d.opacity(0.7))
                            .child(icon("search", 16., ON_ACCENT)),
                    ),
            )
            .child(self.options_row(cx))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .gap(px(18.))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        tab("tab-results", "Results", self.tab == Tab::Results).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.tab = Tab::Results;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        tab("tab-queue", queue_label, self.tab == Tab::Queue).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.tab = Tab::Queue;
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .child(content)
            .children(self.notice.as_ref().map(|(_, text)| {
                div()
                    .flex_none()
                    .text_size(px(12.))
                    .text_color(rgb(MUTED))
                    .child(text.clone())
            }))
            .children(footer)
            .into_any_element()
    }

    fn settings_view(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(18.))
            .px(px(16.))
            .py(px(12.))
            .child(self.folder_row(Kind::Audio, "Audio folder", cx))
            .child(self.folder_row(Kind::Video, "Video folder", cx))
            .child(
                div()
                    .id("always-on-top")
                    .flex()
                    .items_center()
                    .justify_between()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_always_on_top(cx)))
                    .child("Always on top")
                    .child(toggle(self.config.always_on_top)),
            )
            .into_any_element()
    }

    fn folder_row(
        &self,
        kind: Kind,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let path = self.config.dir_for(kind).display().to_string();
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(div().text_size(px(12.)).text_color(rgb(MUTED)).child(label))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h(px(34.))
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .rounded(px(9.))
                            .bg(rgb(SURFACE))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .child(one_line(div()).child(path)),
                    )
                    .child(
                        div()
                            .id(("choose", kind as u32))
                            .h(px(34.))
                            .px(px(14.))
                            .flex()
                            .items_center()
                            .rounded(px(9.))
                            .bg(rgb(RAISED))
                            .cursor_pointer()
                            .hover(|d| d.bg(rgb(0x363b44)))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.choose_folder(kind, cx)),
                            )
                            .child("Change"),
                    ),
            )
    }
}

impl Render for Cadence {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if self.settings_open {
            self.settings_view(cx)
        } else {
            self.main_view(cx)
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .font_family(FONT)
            .text_size(px(13.5))
            .child(self.titlebar(cx))
            .child(body)
    }
}

fn centered_text(text: impl Into<gpui::SharedString>, color: u32) -> gpui::Div {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .px(px(24.))
        .text_color(rgb(color))
        .child(text.into())
}

fn remove_button(id: u64, cx: &mut Context<Cadence>) -> impl IntoElement {
    icon_button(("remove", id), "x", false)
        .on_click(cx.listener(move |this, _, _, _| this.manager.remove(id)))
}

/// Text and color for the second line of a queue row.
fn status_line(job: &Job) -> (String, u32) {
    let format = job.format.label();
    match job.state {
        State::Queued => (format!("{format} · Waiting"), MUTED),
        State::Downloading => {
            let p = job.progress;
            let mut parts = vec![format!("{:.0}%", p.fraction * 100.0)];
            parts.extend(p.speed.map(format_speed));
            parts.extend(p.eta_secs.map(|s| format!("{} left", format_duration(s))));
            (parts.join(" · "), MUTED)
        }
        State::Converting => ("Converting".to_string(), MUTED),
        State::Completed => (format!("{format} · Done"), OK),
        State::Failed => (
            job.error.clone().unwrap_or_else(|| "Failed".to_string()),
            ERROR,
        ),
        State::Cancelled => ("Cancelled".to_string(), MUTED),
    }
}

/// Opens Explorer with the file selected. Falls back to the folder.
fn reveal_in_explorer(path: &Path) {
    let mut cmd = std::process::Command::new("explorer.exe");
    if path.is_file() {
        // Explorer needs the quotes around the path only, not around the whole argument.
        cmd.raw_arg(format!("/select,\"{}\"", path.display()));
    } else if let Some(dir) = path.parent() {
        cmd.arg(dir);
    }
    let _ = cmd.spawn();
}
