#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod discord;
mod proto;

use eframe::egui;
use proto::{activity_label, class_name, is_open_world, Char};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const APP_ID: &str = "1569716367136137338";
const TITLE: &str = "NNK Presence";
const ICON: &[u8] = include_bytes!("../res/icon64.rgba"); // 64x64 RGBA
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

#[link(name = "user32")]
extern "system" {
    fn FindWindowW(class: *const u16, title: *const u16) -> isize;
    fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
    fn SetForegroundWindow(hwnd: isize) -> i32;
}

/// Hide/show via Win32: a hidden eframe window stops running update(), so it can't unhide itself.
fn show_window(show: bool) {
    let t: Vec<u16> = TITLE.encode_utf16().chain([0]).collect();
    unsafe {
        let h = FindWindowW(std::ptr::null(), t.as_ptr());
        if h != 0 {
            ShowWindow(h, if show { 5 } else { 0 }); // SW_SHOW / SW_HIDE
            if show {
                SetForegroundWindow(h);
            }
        }
    }
}

fn reg(args: &[&str]) -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("reg")
        .args(args)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn autostart() -> bool {
    reg(&["query", RUN_KEY, "/v", TITLE])
}

fn set_autostart(on: bool) {
    let ok = if on {
        let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
        reg(&["add", RUN_KEY, "/v", TITLE, "/t", "REG_SZ", "/d", &format!("\"{exe}\" --minimized"), "/f"])
    } else {
        reg(&["delete", RUN_KEY, "/v", TITLE, "/f"])
    };
    if !ok {
        log("autostart: registry update failed");
    }
}

#[derive(Default)]
pub struct Shared {
    pub ch: Char,
    pub frames: u64,
    capture_err: Option<String>,
    discord_clients: usize,
    enabled: bool,
}

pub fn log(msg: &str) {
    let path = std::env::current_exe().map(|p| p.with_extension("log")).unwrap_or_else(|_| "nnk-presence.log".into());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let t = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = writeln!(f, "[{t}] {msg}");
    }
}

fn group(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// 10216846 -> "10.2M"
fn short(n: u32) -> String {
    match n {
        1_000_000.. => format!("{:.1}M", n as f64 / 1e6),
        1_000.. => format!("{:.1}K", n as f64 / 1e3),
        _ => n.to_string(),
    }
}

/// (details, state): "Hajime - 10.2M CP" / "Farming at Sagartha · Ch. 3"
fn lines(c: &Char) -> (String, String) {
    let details = if c.cp > 0 { format!("{} - {} CP", c.name, short(c.cp)) } else { c.name.clone() };
    let mut state = activity_label(c.world).unwrap_or_else(|| "In game".into());
    if c.channel > 0 && is_open_world(c.world) {
        state += &format!(" · Ch. {}", c.channel);
    }
    (details, state)
}

fn activity(c: &Char, start: u64) -> discord::Activity {
    let (details, state) = lines(c);
    let class = class_name(c.class);
    let mut large_text = class.to_string();
    if c.level > 0 {
        large_text += &format!(" · Lv {}", c.level);
    }
    if c.cp > 0 {
        large_text += &format!(" · CP {}", group(c.cp));
    }
    discord::Activity {
        details,
        state,
        large_image: format!("class_{}", class.to_lowercase()),
        large_text,
        small_image: format!("icon_{}", class.to_lowercase()),
        small_text: c.deck.map(|d| format!("Deck {}", d + 1)).unwrap_or_else(|| class.into()),
        start,
    }
}

fn presence_loop(shared: Arc<Mutex<Shared>>) {
    let start = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut rpc = discord::Rpc::new(APP_ID);
    let mut last: Option<(String, String, String)> = None;
    loop {
        let a = {
            let s = shared.lock().unwrap();
            (s.enabled && !s.ch.name.is_empty()).then(|| activity(&s.ch, start))
        };
        let key = a.as_ref().map(|a| (a.details.clone(), a.state.clone(), a.large_text.clone()));
        if key != last {
            rpc.changed();
            last = key;
        }
        let n = rpc.tick(a.as_ref());
        shared.lock().unwrap().discord_clients = n;
        std::thread::sleep(Duration::from_secs(2));
    }
}

struct App {
    shared: Arc<Mutex<Shared>>,
    autostart: bool,
    start_hidden: bool,
    _tray: Option<tray_icon::TrayIcon>,
}

fn tray(ctx: egui::Context) -> Option<tray_icon::TrayIcon> {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    let show = MenuItem::new("Show", true, None);
    let quit = MenuItem::new("Quit", true, None);
    let menu = Menu::new();
    menu.append_items(&[&show, &quit]).ok()?;
    let (show_id, quit_id) = (show.id().clone(), quit.id().clone());
    let c = ctx.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if e.id == quit_id {
            std::process::exit(0); // closing the pipes clears the Discord presence
        }
        if e.id == show_id {
            show_window(true);
            c.request_repaint();
        }
    }));
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if matches!(e, TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }) {
            show_window(true);
            ctx.request_repaint();
        }
    }));
    TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(TITLE)
        .with_icon(tray_icon::Icon::from_rgba(ICON.to_vec(), 64, 64).ok()?)
        .build()
        .map_err(|e| log(&format!("tray: {e}")))
        .ok()
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if std::mem::take(&mut self.start_hidden) || ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            show_window(false); // close = minimise to tray; Quit lives in the tray menu
        }
        let mut s = self.shared.lock().unwrap();
        egui::TopBottomPanel::bottom("footer").show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.small("Created by wilrubyjane with");
                    ui.colored_label(egui::Color32::from_rgb(235, 80, 100), "❤");
                });
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(4.0);
            if let Some(e) = &s.capture_err {
                ui.colored_label(egui::Color32::LIGHT_RED, e);
            } else if s.ch.name.is_empty() {
                ui.label("Waiting for the game…");
                ui.small("Log in, or go to character select and re-enter.");
            } else {
                let (d, st) = lines(&s.ch);
                ui.strong(d);
                ui.label(st);
            }
            ui.add_space(8.0);
            ui.checkbox(&mut s.enabled, "Show on Discord");
            if ui.checkbox(&mut self.autostart, "Start with Windows").changed() {
                set_autostart(self.autostart);
            }
            let n = s.discord_clients;
            ui.small(match n {
                0 => "Discord not running".to_string(),
                1 => "Connected to Discord".to_string(),
                n => format!("Connected to {n} Discord clients"),
            });
        });
        ctx.request_repaint_after(Duration::from_secs(1));
    }
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    let start_hidden = args.iter().any(|a| a == "--minimized");
    if let [_, flag, file] = args.as_slice() {
        if flag == "--replay" {
            match capture::replay(file) {
                Ok(c) => {
                    let (d, s) = lines(&c);
                    println!("{c:?}\n{d}\n{s}");
                }
                Err(e) => eprintln!("replay failed: {e}"),
            }
            return Ok(());
        }
    }
    let shared = Arc::new(Mutex::new(Shared { enabled: true, ..Default::default() }));
    if let Err(e) = capture::start(shared.clone()) {
        log(&format!("capture: {e}"));
        shared.lock().unwrap().capture_err = Some(e);
    }
    let sh = shared.clone();
    std::thread::spawn(move || presence_loop(sh));
    let viewport = egui::ViewportBuilder::default()
        .with_title(TITLE)
        .with_inner_size([300.0, 170.0])
        .with_resizable(false)
        .with_maximize_button(false)
        .with_icon(egui::IconData { rgba: ICON.to_vec(), width: 64, height: 64 });
    eframe::run_native(
        TITLE,
        eframe::NativeOptions { viewport, ..Default::default() },
        Box::new(move |cc| Ok(Box::new(App { shared, autostart: autostart(), start_hidden, _tray: tray(cc.egui_ctx.clone()) }))),
    )
}
