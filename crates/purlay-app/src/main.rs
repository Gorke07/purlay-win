#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Purlay for Windows.

mod actions;
mod assets;
mod banner;
mod chrome;
mod composer;
mod conversation;
mod dialogs;
mod emoji;
mod emoji_picker;
mod i18n;
mod images;
mod main_view;
mod markdown_view;
mod message;
mod nav;
mod pages;
mod palette;
mod panels;
mod root;
mod settings;
mod sidebar;
mod state;
mod theme;
mod thread;
mod tray;
mod ui;

use gpui_kit::component::TitleBar;
use gpui_kit::*;
use purlay_core::prefs::{Appearance, Prefs};
use purlay_platform::instance::{self, Claim};
use state::{AppGlobal, AppModel};

fn main() {
    // `--profile NAME` is the same as `PURLAY_PROFILE=NAME` (set before anything reads it).
    let argv: Vec<String> = std::env::args().collect();
    if let Some(i) = argv.iter().position(|a| a == "--profile") {
        if let Some(p) = argv.get(i + 1) {
            // SAFETY: nothing else runs yet; no other thread reads the environment.
            unsafe { std::env::set_var("PURLAY_PROFILE", p) };
        }
    }
    let (config_dir, data_dir) = purlay_core::prefs::data_dirs().map(|(c, d)| (Some(c), Some(d))).unwrap_or((None, None));
    init_logging(data_dir.as_deref());

    let runtime = Box::leak(Box::new(
        tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().thread_name("purlay-io").build().expect("tokio runtime"),
    ));
    let rt = runtime.handle().clone();

    let args: Vec<String> = std::env::args().collect();
    let activations = match instance::claim(&rt, args.clone()) {
        Claim::Primary(rx) => rx,
        Claim::Forwarded => return,
    };

    let prefs = Prefs::load(config_dir.as_ref().map(|d| d.join("prefs.json")));
    i18n::set_language(&prefs.get().language);

    if let Err(e) = purlay_platform::register_protocol() {
        tracing::warn!("could not register purlay: links: {e}");
    }
    let icon_path = data_dir.as_ref().map(|d| d.join("purlay.png"));
    if let Some(p) = &icon_path {
        if !p.exists() {
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            let _ = std::fs::write(p, assets::LOGO_PNG);
        }
    }
    purlay_platform::register_app_identity(icon_path.as_deref());

    let initial_url = purlay_platform::url_from_args(&args);
    let background = args.iter().any(|a| a == "--background");

    gpui_kit::application().with_assets(assets::Assets).run(move |cx| {
        gpui_kit::init(cx);
        actions::bind(cx);
        cx.on_action(|_: &actions::Quit, cx| cx.quit());

        let appearance = prefs.get().appearance;
        let dark = match appearance {
            Appearance::Dark => true,
            Appearance::Light => false,
            Appearance::System => matches!(cx.window_appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark),
        };
        theme::apply(dark, cx);

        let app = cx.new(|cx| AppModel::new(rt.clone(), prefs.clone(), data_dir.clone(), cx));
        cx.set_global(AppGlobal(app.clone()));
        let imgs = cx.new(|_| images::Images::new(app.clone()));
        cx.set_global(images::ImagesGlobal(imgs));

        let saved = prefs.get().window;
        let bounds = match saved {
            Some(w) if w.width >= 720.0 && w.height >= 480.0 => {
                let b = Bounds::new(point(px(w.x), px(w.y)), size(px(w.width), px(w.height)));
                if w.maximized { WindowBounds::Maximized(b) } else { WindowBounds::Windowed(b) }
            }
            _ => WindowBounds::Windowed(Bounds::centered(None, size(px(1240.), px(820.)), cx)),
        };
        let mut options = TitleBar::window_options();
        options.window_bounds = Some(bounds);
        options.window_min_size = Some(size(px(720.), px(480.)));
        options.app_id = Some("dev.purlay.windows".into());
        options.show = !background;
        options.focus = !background;

        let (window, _root) = gpui_kit::open_window(options, cx, |window, cx| {
            let prefs2 = prefs.clone();
            window
                .observe_window_appearance(move |window, cx| {
                    if prefs2.get().appearance == Appearance::System {
                        let dark = matches!(window.appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark);
                        theme::apply(dark, cx);
                    }
                })
                .detach();
            let prefs3 = prefs.clone();
            window.on_window_should_close(cx, move |window, cx| {
                save_window(&prefs3, window);
                // With the tray icon, closing hides the window and Purlay keeps running.
                if tray::installed(cx) && !prefs3.get().quit_on_close {
                    tray::hide_window(window);
                    if !prefs3.get().tray_hint_shown {
                        prefs3.update(|d| d.tray_hint_shown = true);
                        purlay_platform::toast(i18n::t("tray.stillRunning"), i18n::t("tray.stillRunningBody"), None, true);
                    }
                    return false;
                }
                true
            });
            cx.new(|cx| root::RootView::new(window, cx))
        })
        .expect("open window");
        cx.set_global(banner::MainWindow(window));

        // The tray: a click shows the window, the menu opens settings or quits.
        if let Some(mut commands) = tray::install(cx) {
            let app3 = app.clone();
            let prefs4 = prefs.clone();
            cx.spawn(async move |cx| {
                while let Some(cmd) = commands.recv().await {
                    cx.update(|cx| {
                        let _ = window.update(cx, |_, window, cx| match cmd {
                            tray::TrayCommand::Show => tray::show_window(window),
                            tray::TrayCommand::Settings => {
                                tray::show_window(window);
                                app3.update(cx, |m, cx| m.open_settings(cx));
                            }
                            tray::TrayCommand::Quit => save_window(&prefs4, window),
                        });
                        if cmd == tray::TrayCommand::Quit {
                            cx.quit();
                        }
                    });
                }
            })
            .detach();
        }

        if let Some(url) = initial_url.clone() {
            app.update(cx, |m, cx| m.handle_url(url, cx));
        }

        // Later launches (and purlay: links) arrive here.
        let mut activations = activations;
        let app2 = app.clone();
        cx.spawn(async move |cx| {
            while let Some(a) = activations.recv().await {
                let url = purlay_platform::url_from_args(&a.args).or_else(|| a.args.iter().find(|x| x.starts_with("purlay:")).cloned());
                cx.update(|cx| {
                    if let Some(u) = url {
                        app2.update(cx, |m, cx| m.handle_url(u, cx));
                    }
                    let _ = window.update(cx, |_, window, _| tray::show_window(window));
                });
            }
        })
        .detach();
        cx.activate(true);
    });
}

fn save_window(prefs: &Prefs, window: &Window) {
    let (b, maximized) = match window.window_bounds() {
        WindowBounds::Windowed(b) => (b, false),
        WindowBounds::Maximized(b) => (b, true),
        WindowBounds::Fullscreen(b) => (b, true),
    };
    prefs.update(|d| {
        d.window = Some(purlay_core::prefs::WindowState {
            x: f32::from(b.origin.x),
            y: f32::from(b.origin.y),
            width: f32::from(b.size.width),
            height: f32::from(b.size.height),
            maximized,
        })
    });
}

fn init_logging(dir: Option<&std::path::Path>) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env("PURLAY_LOG").unwrap_or_else(|_| EnvFilter::new("warn,purlay=info"));
    let file = dir.and_then(|d| {
        let logs = d.join("logs");
        std::fs::create_dir_all(&logs).ok()?;
        std::fs::OpenOptions::new().create(true).append(true).open(logs.join("purlay.log")).ok()
    });
    match file {
        Some(f) => {
            let _ = tracing_subscriber::fmt().with_env_filter(filter).with_ansi(false).with_writer(std::sync::Mutex::new(f)).try_init();
        }
        None => {
            let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
        }
    }
}
