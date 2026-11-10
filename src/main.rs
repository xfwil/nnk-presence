#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod proto;

use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};
use eframe::egui;
use proto::{class_name, world_name, Char};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const APP_ID: &str = "1569716367136137338";

#[derive(Default)]
pub struct Shared {
    pub ch: Char,
    pub frames: u64,
    adapters: Vec<String>,
    capture_err: Option<String>,
    discord: String,
    enabled: bool,
}

fn log(msg: &str) {
    let path = std::env::current_exe().map(|p| p.with_extension("log")).unwrap_or_else(|_| "nnk-presence.log".into());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let t = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = writeln!(f, "[{t}] {msg}");
    }
}

#[derive(PartialEq, Clone)]
struct Presence {
    details: String,
    state: String,
    class: String,
    cp: String,
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

fn build(c: &Char) -> Presence {
    let class = class_name(c.class);
    let mut details = format!("{} · {class}", c.name);
    if c.level > 0 {
        details += &format!(" Lv {}", c.level);
    }
    let mut parts = vec![];
    if let Some(w) = world_name(c.world) {
        parts.push(w.to_string());
    }
    if c.channel > 0 {
        parts.push(format!("Ch. {}", c.channel));
    }
    if let Some(d) = c.deck {
        parts.push(format!("Deck {}", d + 1));
    }
    Presence {
        details,
        state: if parts.is_empty() { "In game".into() } else { parts.join(" · ") },
        class: class.to_lowercase(),
        cp: if c.cp > 0 { format!("CP {}", group(c.cp)) } else { class.into() },
    }
}

fn presence_loop(shared: Arc<Mutex<Shared>>) {
    let start = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let mut client: Option<DiscordIpcClient> = None;
    let mut last: Option<Option<Presence>> = None;
    loop {
        std::thread::sleep(Duration::from_secs(2));
        let want = {
            let s = shared.lock().unwrap();
            (s.enabled && !s.ch.name.is_empty()).then(|| build(&s.ch))
        };
        if client.is_none() {
            match DiscordIpcClient::new(APP_ID).and_then(|mut c| c.connect().map(|_| c)) {
                Ok(c) => {
                    client = Some(c);
                    last = None;
                    shared.lock().unwrap().discord = "Connected".into();
                }
                Err(_) => {
                    shared.lock().unwrap().discord = "Discord not running".into();
                    continue;
                }
            }
        }
        if last.as_ref() == Some(&want) {
            continue;
        }
        let c = client.as_mut().unwrap();
        let r = match &want {
            Some(p) => {
                let large = format!("class_{}", p.class);
                let small = format!("icon_{}", p.class);
                c.set_activity(
                    activity::Activity::new()
                        .details(&p.details)
                        .state(&p.state)
                        .assets(activity::Assets::new().large_image(&large).large_text(&p.cp).small_image(&small).small_text(&p.class))
                        .timestamps(activity::Timestamps::new().start(start)),
                )
            }
            None => c.clear_activity(),
        };
        match r {
            Ok(_) => last = Some(want),
            Err(e) => {
                log(&format!("discord: {e}"));
                client = None;
                shared.lock().unwrap().discord = "Disconnected, retrying".into();
            }
        }
    }
}

struct App {
    shared: Arc<Mutex<Shared>>,
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut s = self.shared.lock().unwrap();
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("NNK Presence");
            ui.add_space(6.0);
            let c = &s.ch;
            let dash = |v: String| if v.is_empty() || v == "0" { "—".to_string() } else { v };
            egui::Grid::new("char").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                let rows = [
                    ("Name", dash(c.name.clone())),
                    ("Class", if c.class == 0 { "—".into() } else { class_name(c.class).into() }),
                    ("Level", dash(c.level.to_string())),
                    ("CP", if c.cp == 0 { "—".into() } else { group(c.cp) }),
                    ("Map", world_name(c.world).map(str::to_string).unwrap_or_else(|| dash(c.world.to_string()))),
                    ("Channel", dash(c.channel.to_string())),
                    ("Deck", c.deck.map(|d| (d + 1).to_string()).unwrap_or("—".into())),
                ];
                for (k, v) in rows {
                    ui.label(k);
                    ui.strong(v);
                    ui.end_row();
                }
            });
            ui.separator();
            match &s.capture_err {
                Some(e) => ui.colored_label(egui::Color32::LIGHT_RED, format!("Capture: {e}")),
                None => ui.label(format!("Capture: {} adapter(s), {} frames", s.adapters.len(), s.frames)),
            };
            ui.label(format!("Discord: {}", s.discord));
            if s.ch.name.is_empty() {
                ui.small("Waiting for character data: log in, or change map / channel.");
            }
            ui.checkbox(&mut s.enabled, "Show on Discord");
        });
        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    if let [_, flag, file] = args.as_slice() {
        if flag == "--replay" {
            match capture::replay(file) {
                Ok(c) => println!("{c:?}\n{}\n{}", build(&c).details, build(&c).state),
                Err(e) => eprintln!("replay failed: {e}"),
            }
            return Ok(());
        }
    }
    let shared = Arc::new(Mutex::new(Shared { enabled: true, discord: "Connecting…".into(), ..Default::default() }));
    match capture::start(shared.clone()) {
        Ok(a) => shared.lock().unwrap().adapters = a,
        Err(e) => {
            log(&format!("capture: {e}"));
            shared.lock().unwrap().capture_err = Some(e);
        }
    }
    let sh = shared.clone();
    std::thread::spawn(move || presence_loop(sh));
    eframe::run_native(
        "NNK Presence",
        eframe::NativeOptions { viewport: egui::ViewportBuilder::default().with_inner_size([320.0, 300.0]).with_resizable(false), ..Default::default() },
        Box::new(|_| Ok(Box::new(App { shared }))),
    )
}
