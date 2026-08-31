mod model;
mod net;
mod notify;
mod term;
mod theme;
mod ui;

use gpui::{App, AppContext as _, px, size};

fn main() {
    env_logger::init();
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = gpui::Bounds::centered(None, size(px(1160.), px(760.)), cx);
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("AAA-UI".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| ui::RootView::new(window, cx)),
        )
        .unwrap();
        cx.activate(true);
    });
}
