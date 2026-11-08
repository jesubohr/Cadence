//! Small building blocks used by the main view.

use std::path::PathBuf;

use gpui::{
    div, img, prelude::*, px, rgb, svg, Div, ElementId, FontWeight, ObjectFit, SharedString,
    Stateful,
};

use super::theme::*;

pub fn icon(name: &'static str, size: f32, color: u32) -> impl IntoElement {
    svg()
        .path(format!("icons/{name}.svg"))
        .size(px(size))
        .flex_none()
        .text_color(rgb(color))
}

pub fn icon_button(id: impl Into<ElementId>, name: &'static str, active: bool) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(30.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(7.))
        .cursor_pointer()
        .when(active, |d| d.bg(rgb(RAISED)))
        .hover(|d| d.bg(rgb(SURFACE_HOVER)))
        .child(icon(name, 15., if active { TEXT } else { MUTED }))
}

/// Container for a group of `segment`s.
pub fn segmented() -> Div {
    div()
        .flex()
        .p(px(2.))
        .gap(px(2.))
        .rounded(px(9.))
        .bg(rgb(SURFACE))
}

pub fn segment(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(10.))
        .h(px(26.))
        .flex()
        .items_center()
        .rounded(px(7.))
        .cursor_pointer()
        .text_size(px(12.5))
        .when(active, |d| {
            d.bg(rgb(RAISED))
                .text_color(rgb(TEXT))
                .font_weight(FontWeight::MEDIUM)
        })
        .when(!active, |d| {
            d.text_color(rgb(MUTED)).hover(|d| d.text_color(rgb(TEXT)))
        })
        .child(label.into())
}

pub fn tab(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(30.))
        .flex()
        .items_center()
        .cursor_pointer()
        .border_b_2()
        .border_color(rgb(if active { ACCENT } else { BG }))
        .text_color(rgb(if active { TEXT } else { MUTED }))
        .font_weight(FontWeight::MEDIUM)
        .child(label.into())
}

pub fn primary_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    enabled: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(34.))
        .px(px(14.))
        .flex()
        .items_center()
        .gap(px(7.))
        .rounded(px(9.))
        .bg(rgb(ACCENT))
        .text_color(rgb(ON_ACCENT))
        .font_weight(FontWeight::SEMIBOLD)
        .when(enabled, |d| {
            d.cursor_pointer().hover(|d| d.bg(rgb(ACCENT_HOVER)))
        })
        .when(!enabled, |d| d.opacity(0.4))
        .child(icon("download", 14., ON_ACCENT))
        .child(label.into())
}

pub fn text_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(30.))
        .px(px(8.))
        .flex()
        .items_center()
        .rounded(px(7.))
        .cursor_pointer()
        .text_color(rgb(MUTED))
        .hover(|d| d.bg(rgb(SURFACE_HOVER)).text_color(rgb(TEXT)))
        .child(label.into())
}

pub fn checkbox(checked: bool) -> Div {
    div()
        .flex_none()
        .size(px(18.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .border_1()
        .border_color(rgb(if checked { ACCENT } else { RAISED }))
        .when(checked, |d| {
            d.bg(rgb(ACCENT)).child(icon("check", 12., ON_ACCENT))
        })
}

pub fn toggle(on: bool) -> Div {
    div()
        .flex_none()
        .w(px(38.))
        .h(px(22.))
        .p(px(2.))
        .flex()
        .rounded_full()
        .bg(rgb(if on { ACCENT } else { RAISED }))
        .when(on, |d| d.justify_end())
        .child(div().size(px(18.)).rounded_full().bg(rgb(0xffffff)))
}

pub fn thumbnail(path: Option<&PathBuf>) -> Div {
    let frame = div()
        .flex_none()
        .w(px(64.))
        .h(px(36.))
        .rounded(px(5.))
        .overflow_hidden()
        .bg(rgb(RAISED));
    match path {
        Some(path) => frame.child(img(path.clone()).size_full().object_fit(ObjectFit::Cover)),
        None => frame,
    }
}

pub fn progress_bar(fraction: f32) -> Div {
    div()
        .h(px(3.))
        .w_full()
        .rounded_full()
        .bg(rgb(RAISED))
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .w(gpui::relative(fraction.clamp(0.0, 1.0)))
                .rounded_full()
                .bg(rgb(ACCENT)),
        )
}

/// Text that stays on one line and ends with an ellipsis when it is too long.
/// gpui measures no-wrap text once, so a line clamp is used instead.
pub fn one_line(el: Div) -> Div {
    el.w_full().text_ellipsis().line_clamp(1)
}
