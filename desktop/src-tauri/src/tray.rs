//! System tray integration.
//!
//! Single icon with a small menu: show window / start / stop / quit. Left
//! click on the icon brings the window back from a minimized/hidden state.

use tauri::{
    AppHandle, Manager,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

pub const TRAY_ID: &str = "open-promux-tray";

const MENU_ID_SHOW: &str = "tray.show";
const MENU_ID_START: &str = "tray.start";
const MENU_ID_STOP: &str = "tray.stop";
const MENU_ID_QUIT: &str = "tray.quit";

pub fn install(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, MENU_ID_SHOW, "Show Window", true, None::<&str>)?;
    let start = MenuItem::with_id(app, MENU_ID_START, "Start Server", true, None::<&str>)?;
    let stop = MenuItem::with_id(app, MENU_ID_STOP, "Stop Server", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, MENU_ID_QUIT, "Quit", true, None::<&str>)?;

    let menu = Menu::with_items(app, &[&show, &start, &stop, &quit])?;

    TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("open-promux")
        .icon(
            app.default_window_icon()
                .cloned()
                .ok_or_else(|| tauri::Error::AssetNotFound("default window icon missing".into()))?,
        )
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_ID_SHOW => focus_main_window(app),
            MENU_ID_START => spawn_command(app, "start_server"),
            MENU_ID_STOP => spawn_command(app, "stop_server"),
            MENU_ID_QUIT => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                focus_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

fn focus_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn spawn_command(app: &AppHandle, name: &'static str) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<crate::state::DesktopState>();
        let result = match name {
            "start_server" => crate::commands::start_server(state).await.map(|_| ()),
            "stop_server" => crate::commands::stop_server(state).await,
            _ => Ok(()),
        };
        if let Err(err) = result {
            tracing::warn!("tray command {name} failed: {err}");
        }
    });
}
