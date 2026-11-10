//! System tray icon with a Show and Quit menu.

use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    Quit,
}

/// Keep this value alive. Dropping it removes the icon.
pub struct Tray(#[allow(dead_code)] TrayIcon);

/// Creates the tray icon. Call it on the main thread, where gpui runs the
/// Windows message loop that the tray needs. Actions arrive on `actions`.
pub fn create(actions: async_channel::Sender<TrayAction>) -> Result<Tray, String> {
    let show = MenuItem::new("Show Cadence", true, None);
    let quit = MenuItem::new("Quit", true, None);
    let menu = Menu::new();
    menu.append_items(&[&show, &quit])
        .map_err(|e| e.to_string())?;

    // Resource 1 is the logo that build.rs embeds in the executable.
    let image = Icon::from_resource(1, Some((32, 32))).map_err(|e| e.to_string())?;
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_icon(image)
        .with_tooltip("Cadence")
        .build()
        .map_err(|e| e.to_string())?;

    let (show_id, quit_id) = (show.id().clone(), quit.id().clone());
    let menu_actions = actions.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let action = if event.id == show_id {
            TrayAction::Show
        } else if event.id == quit_id {
            TrayAction::Quit
        } else {
            return;
        };
        let _ = menu_actions.try_send(action);
    }));
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        let restore = matches!(
            event,
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } | TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            }
        );
        if restore {
            let _ = actions.try_send(TrayAction::Show);
        }
    }));
    Ok(Tray(tray))
}
