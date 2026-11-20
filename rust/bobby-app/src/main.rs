// Bobby's desktop app: tray icon, control window, supervisor of the three parts, metrics and journal.
// Closing the window destroys the webview (so it costs nothing) and leaves Bobby running; "Quitter" stops Bobby and exits.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bridge;
mod config;
mod gpu_procs;
mod host;
mod journal;
mod metrics;
mod server;
mod sleep;
mod supervisor;

use std::sync::{Arc, Mutex};
use supervisor::Supervisor;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

struct Port(u16);

/// A 32x32 disc for the tray: green = running, amber = started elsewhere, grey = stopped.
fn dot(rgb: [u8; 3]) -> Image<'static> {
    let n = 32usize;
    let c = (n as f32 - 1.0) / 2.0;
    let mut px = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            let a = (14.5 - d).clamp(0.0, 1.0); // anti-aliased edge
            let i = (y * n + x) * 4;
            px[i..i + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], (a * 255.0) as u8]);
        }
    }
    Image::new_owned(px, n as u32, n as u32)
}

fn tray_look(st: &serde_json::Value) -> (Image<'static>, &'static str) {
    if st["running"] == true {
        (dot([63, 185, 122]), "Bobby : en marche")
    } else if st["external"] == true {
        (dot([224, 164, 58]), "Bobby : lancé ailleurs (start.cmd)")
    } else {
        (dot([125, 132, 148]), "Bobby : arrêté")
    }
}

fn open_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let port = app.state::<Port>().0;
    let url = format!("http://127.0.0.1:{port}/").parse().expect("valid url");
    let _ = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
        .title("Bobby - Contrôle")
        .inner_size(1280.0, 860.0)
        .min_inner_size(720.0, 480.0)
        .build();
}

fn main() {
    let cfg = config::load();
    let logs = cfg.logs();
    let _ = std::fs::create_dir_all(&logs);
    let (bus, _) = tokio::sync::broadcast::channel(512);
    let sup = Supervisor::new(&cfg, bus.clone());
    let hidden = std::env::args().any(|a| a == "--hidden");

    let app = tauri::Builder::default()
        // must come first: a second launch only focuses the window of the first one
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| open_window(app)))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec!["--hidden"])))
        .manage(Port(cfg.port))
        .manage(sup.clone())
        .setup(move |app| {
            let handle = app.handle().clone();

            // metrics database, sampler, journal follower, HTTP server
            let db = Arc::new(Mutex::new(metrics::open(&logs.join("metrics.db"))?));
            tauri::async_runtime::spawn(metrics::run(sup.clone(), db.clone(), bus.clone()));
            tauri::async_runtime::spawn(journal::follow(logs.join("conversations.jsonl"), bus.clone()));
            let port = cfg.port;
            let listener = tauri::async_runtime::block_on(server::bind(port))
                .map_err(|e| format!("port {port} is taken (is the Node app, Bobby.cmd, still running?): {e}"))?;
            let bridge = bridge::Bridge::new(&cfg, sup.clone());
            if cfg.rust_bobby() {
                // one brain for Discord and the PC, started and stopped with Bobby (rollback: [run] brain/discord = "node")
                let host = host::Host::new(&cfg, bridge.clone(), sup.clone());
                sup.on_run(Box::new(move |on| if on { host.start() } else { host.stop() }));
                sup.log("bobby", "Rust brain and Discord client: the bot part is not started");
            }
            let state = server::AppState { sup: sup.clone(), db, bus: bus.clone(), logs: logs.clone(), tts: bridge.tts_slot() };
            let bridge = bridge::routes(bridge);
            tauri::async_runtime::spawn(async move {
                let _ = axum::serve(listener, server::router(state, bridge)).await;
            });

            // tray
            let autostart = handle.autolaunch().is_enabled().unwrap_or(false);
            let items = (
                MenuItem::with_id(app, "open", "Ouvrir", true, None::<&str>)?,
                MenuItem::with_id(app, "start", "Démarrer", true, None::<&str>)?,
                MenuItem::with_id(app, "stop", "Arrêter", true, None::<&str>)?,
                MenuItem::with_id(app, "restart", "Redémarrer", true, None::<&str>)?,
                CheckMenuItem::with_id(app, "autostart", "Lancer au démarrage de Windows", true, autostart, None::<&str>)?,
                MenuItem::with_id(app, "quit", "Quitter", true, None::<&str>)?,
                MenuItem::with_id(app, "sleep", "Deep Sleep / Réveiller", true, None::<&str>)?,
            );
            let sep = || PredefinedMenuItem::separator(app);
            let menu = Menu::with_items(app, &[&items.0, &sep()?, &items.1, &items.2, &items.3, &items.6, &sep()?, &items.4, &sep()?, &items.5])?;
            let auto_item = items.4.clone();
            let (icon, tip) = tray_look(&sup.status());
            let tray = TrayIconBuilder::with_id("bobby")
                .icon(icon)
                .tooltip(tip)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, ev| {
                    let sup = app.state::<Arc<Supervisor>>().inner().clone();
                    match ev.id.as_ref() {
                        "open" => open_window(app),
                        "sleep" => {
                            let what = if sup.status()["sleep"] == "awake" { "sleep" } else { "wake" };
                            tauri::async_runtime::spawn_blocking(move || {
                                let _ = sup.act(what);
                            });
                        }
                        what @ ("start" | "stop" | "restart") => {
                            let what = what.to_string();
                            // off the UI thread (stop waits for taskkill), inside the runtime (start spawns tasks)
                            tauri::async_runtime::spawn_blocking(move || {
                                if let Err(e) = sup.act(&what) {
                                    sup.log("bobby", &format!("{what} refused: {e}"));
                                }
                            });
                        }
                        "autostart" => {
                            let al = app.autolaunch();
                            let on = !al.is_enabled().unwrap_or(false);
                            let _ = if on { al.enable() } else { al.disable() };
                            let _ = auto_item.set_checked(al.is_enabled().unwrap_or(false));
                        }
                        "quit" => {
                            sup.stop(); // decided: quitting the app stops Bobby (nothing would supervise it otherwise)
                            app.exit(0);
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, ev| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                        open_window(tray.app_handle());
                    }
                })
                .build(app)?;

            // keep the tray icon in step with Bobby's state
            let mut rx = bus.subscribe();
            tauri::async_runtime::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(e) if e.name == "status" => {
                            let (icon, tip) = tray_look(&e.data);
                            let _ = tray.set_icon(Some(icon));
                            let _ = tray.set_tooltip(Some(tip));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        _ => {}
                    }
                }
            });

            // Bobby itself: started unless it already runs elsewhere
            if cfg.start_bobby_on_launch {
                let mut sys = sysinfo::System::new().expect("sysinfo");
                sys.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, sysinfo::ProcessRefreshKind::nothing());
                sup.set_external(sys.processes().values().any(|p| p.name().eq_ignore_ascii_case("llama-server.exe")));
                if !sup.is_external() {
                    let sup = sup.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = sup.start() {
                            sup.log("bobby", &format!("start refused: {e}"));
                        }
                    });
                }
            }
            if !hidden {
                open_window(&handle);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("cannot build the Tauri app");

    app.run(|app, event| match event {
        // closing the last window keeps the app in the tray; only "Quitter" (exit with a code) ends it
        RunEvent::ExitRequested { code: None, api, .. } => api.prevent_exit(),
        RunEvent::Exit => {
            app.state::<Arc<Supervisor>>().stop();
        }
        _ => {}
    });
}
