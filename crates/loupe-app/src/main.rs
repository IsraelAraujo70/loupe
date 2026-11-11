// GPUI derive macros name the `gpui` crate; gpui-kit documents this alias.
extern crate gpui_kit as gpui;

mod assets;
mod cache;
mod github;
mod linear;
mod links;
mod route;
mod services;
mod settings;
mod shell;
mod store;
mod t3;
mod theme;
mod ui;
mod updater;

use futures::StreamExt as _;
use gpui_kit::*;

use crate::route::Route;
use crate::services::{Accounts, Services};
use crate::shell::Workspace;
use crate::store::Settings;

fn main() {
    let app = gpui_kit::application().with_assets(assets::LoupeAssets);
    // macOS hands over `loupe://` URLs and the links Finicky sends; the
    // platform callback has no `App`, so the URLs wait in a channel.
    let (urls_tx, mut urls_rx) = futures::channel::mpsc::unbounded::<Vec<String>>();
    app.on_open_urls(move |urls| {
        urls_tx.unbounded_send(urls).ok();
    });
    app.run(move |cx: &mut App| {
        gpui_kit::init(cx);
        theme::init(cx);
        shell::http::install(cx);
        shell::menus::init(cx);
        let settings = Settings::load();
        theme::apply_zoom(settings.zoom, cx);
        cx.set_global(settings);
        cx.set_global(theme::themes::Themes::load(&theme::themes::themes_dir()));
        theme::apply(cx);
        cx.set_global(Services::default());
        cx.set_global(Accounts::default());
        updater::start(cx);
        settings::session::start(cx);

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(1280.), px(820.)), cx)),
            titlebar: Some(TitlebarOptions {
                title: Some(format!("Loupe {}", env!("CARGO_PKG_VERSION")).into()),
                appears_transparent: true,
                // Centered in the 40 px top bar that holds the tabs.
                traffic_light_position: Some(point(px(14.), px(14.))),
            }),
            // macOS ignores drag areas in GPUI content; `ui::WindowDragArea`
            // moves and zooms the window itself.
            app_owns_titlebar_drag: true,
            window_background: theme::window_background(),
            window_min_size: Some(size(px(900.), px(560.))),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| Workspace::new(Route::Inbox, window, cx))
        })
        .expect("open the main window");
        cx.activate(true);
        cx.spawn(async move |cx| {
            while let Some(urls) = urls_rx.next().await {
                cx.update(|cx| shell::open_system_urls(urls, cx));
            }
        })
        .detach();
    });
}
