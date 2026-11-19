use crate::{net::{self, Download}, platform::{self, Event}, vpn};
use eframe::egui::{self, Color32, CornerRadius, FontId, Frame, Margin, RichText, Stroke, TextStyle, Vec2};
use hostnr_core::{hosts::Hosts, store::{self, Profile, State, Store}};
use std::{collections::VecDeque, path::PathBuf, sync::{atomic::Ordering, mpsc, Arc, Mutex}, time::{Duration, Instant}};

#[derive(Clone, Copy)]
struct Colors {
    dark: bool,
    primary: Color32,
    on_primary: Color32,
    container: Color32,
    on_container: Color32,
    background: Color32,
    text: Color32,
    surface: Color32,
    secondary: Color32,
}

fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Colors {
    fn read(dynamic: bool, values: &[i32]) -> Self {
        let dark = values.first().copied().unwrap_or(1) != 0;
        if dynamic && values.len() >= 10 && values[9] != 0 {
            return Self {
                dark, primary: rgb(values[1] as u32), on_primary: rgb(values[2] as u32),
                container: rgb(values[3] as u32), on_container: rgb(values[4] as u32),
                background: rgb(values[5] as u32), text: rgb(values[6] as u32),
                surface: rgb(values[7] as u32), secondary: rgb(values[8] as u32),
            };
        }
        if dark {
            Self { dark, primary: rgb(0x8ed5c3), on_primary: rgb(0x00382f), container: rgb(0x005145),
                on_container: rgb(0xabf2df), background: rgb(0x101512), text: rgb(0xe0e4df), surface: rgb(0x202b27), secondary: rgb(0xb2ccc2) }
        } else {
            Self { dark, primary: rgb(0x006b5b), on_primary: Color32::WHITE, container: rgb(0xa6f2dc),
                on_container: rgb(0x002019), background: rgb(0xf5fbf5), text: rgb(0x171d19), surface: rgb(0xe7efe8), secondary: rgb(0x4b635b) }
        }
    }

    fn apply(self, ctx: &egui::Context) {
        let mut style = (*ctx.style()).clone();
        style.visuals = if self.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        style.visuals.panel_fill = self.background;
        style.visuals.window_fill = self.surface;
        style.visuals.extreme_bg_color = self.background;
        style.visuals.override_text_color = Some(self.text);
        style.visuals.selection.bg_fill = self.container;
        style.visuals.selection.stroke = Stroke::new(1.0, self.on_container);
        style.visuals.hyperlink_color = self.primary;
        style.visuals.window_corner_radius = CornerRadius::same(28);
        for widget in [&mut style.visuals.widgets.inactive, &mut style.visuals.widgets.hovered, &mut style.visuals.widgets.active, &mut style.visuals.widgets.open] {
            widget.corner_radius = CornerRadius::same(24);
            widget.bg_fill = self.container;
            widget.weak_bg_fill = self.container;
            widget.fg_stroke = Stroke::new(1.5, self.on_container);
            widget.bg_stroke = Stroke::NONE;
        }
        style.spacing.item_spacing = egui::vec2(10.0, 12.0);
        style.spacing.button_padding = egui::vec2(16.0, 12.0);
        style.spacing.interact_size = egui::vec2(48.0, 48.0);
        style.text_styles.insert(TextStyle::Body, FontId::proportional(15.0));
        style.text_styles.insert(TextStyle::Button, FontId::proportional(14.0));
        style.text_styles.insert(TextStyle::Small, FontId::proportional(12.0));
        style.text_styles.insert(TextStyle::Heading, FontId::proportional(28.0));
        style.text_styles.insert(TextStyle::Monospace, FontId::monospace(12.0));
        style.animation_time = 0.2;
        ctx.set_style(style);
    }
}

pub struct HostApp {
    repo: Option<Arc<Mutex<Store>>>,
    state: State,
    tab: usize,
    message: Option<String>,
    busy: bool,
    tx: mpsc::Sender<Result<String, String>>,
    rx: mpsc::Receiver<Result<String, String>>,
    files: VecDeque<(String, String)>,
    url_dialog: bool,
    theme_dialog: bool,
    url: String,
    name: String,
    preview: Option<(String, String)>,
    delete: Option<Profile>,
    palette: Vec<i32>,
    palette_at: Instant,
    was_running: bool,
}

impl HostApp {
    pub fn new(cc: &eframe::CreationContext<'_>, root: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel();
        let (repo, state, message) = match Store::open(root) {
            Ok(s) => {
                let selected = s.selected().map(|p| s.text(p).and_then(|t| Hosts::parse(&t))).transpose();
                let message = match selected { Ok(h) => { vpn::set_hosts(h); None }, Err(e) => Some(e) };
                let state = s.state.clone();
                (Some(Arc::new(Mutex::new(s))), state, message)
            }
            Err(e) => (None, State::default(), Some(e)),
        };
        let palette = platform::palette().unwrap_or_default();
        Colors::read(state.dynamic, &palette).apply(&cc.egui_ctx);
        Self { repo, state, tab: 0, message, busy: false, tx, rx, files: VecDeque::new(),
            url_dialog: false, theme_dialog: false, url: String::new(), name: String::new(),
            preview: None, delete: None, palette, palette_at: Instant::now(), was_running: vpn::running() }
    }

    fn task(&mut self, f: impl FnOnce(&mut Store) -> Result<String, String> + Send + 'static) {
        if self.busy { return; }
        let Some(repo) = self.repo.clone() else { return; };
        self.busy = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let mut store = repo.lock().map_err(|e| e.to_string())?;
                let message = f(&mut store)?;
                let hosts = store.selected().map(|p| store.text(p).and_then(|s| Hosts::parse(&s))).transpose()?;
                let empty = hosts.is_none();
                vpn::set_hosts(hosts);
                if empty && vpn::running() { platform::action("stopVpn")?; }
                Ok(message)
            })();
            let _ = tx.send(result);
        });
    }

    fn selected(&self) -> Option<&Profile> {
        self.state.profiles.iter().find(|p| Some(&p.id) == self.state.selected.as_ref())
    }

    fn action(&mut self, name: &str) {
        if let Err(e) = platform::action(name) { self.message = Some(e); }
    }

    fn download(&mut self, profile: Option<Profile>) {
        let url = profile.as_ref().and_then(|p| p.url.clone()).unwrap_or_else(|| self.url.trim().to_owned());
        let name = profile.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| self.name.clone());
        self.task(move |store| match net::fetch(&url, profile.as_ref())? {
            Download::Unchanged => { store.record("Source checked")?; Ok("Already up to date".into()) }
            Download::Text { text, etag, modified } => {
                store.import(name, Some(url), &text, profile.as_ref().map(|p| p.id.as_str()), etag, modified)?;
                Ok("Saved offline".into())
            }
        });
    }

    fn card(ui: &mut egui::Ui, color: Color32, f: impl FnOnce(&mut egui::Ui)) {
        Frame::new().fill(color).corner_radius(28).inner_margin(20).show(ui, |ui| {
            ui.set_width(ui.available_width());
            f(ui);
        });
    }

    fn button(ui: &mut egui::Ui, text: &str, c: Colors) -> bool {
        ui.add_sized([ui.available_width(), 52.0], egui::Button::new(RichText::new(text).color(c.on_primary)).fill(c.primary).corner_radius(28)).clicked()
    }

    fn imports(&mut self, ui: &mut egui::Ui) {
        ui.add_enabled_ui(!self.busy && self.repo.is_some(), |ui| {
            ui.columns(2, |cols| {
                let width = cols[0].available_width();
                if cols[0].add_sized([width, 52.0], egui::Button::new("Import file")).clicked() { self.action("pickHosts"); }
                let width = cols[1].available_width();
                if cols[1].add_sized([width, 52.0], egui::Button::new("Add URL")).clicked() { self.url_dialog = true; }
            });
        });
    }

    fn home(&mut self, ui: &mut egui::Ui, c: Colors) {
        ui.add_space(20.0);
        let running = vpn::running();
        Self::card(ui, c.container, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(16.0);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(64.0, 64.0), egui::Sense::hover());
                let center = rect.center();
                let painter = ui.painter();
                painter.circle_stroke(center, 26.0, Stroke::new(2.0, c.on_container));
                painter.line_segment([center + egui::vec2(0.0, -14.0), center + egui::vec2(0.0, 2.0)], Stroke::new(3.0, c.on_container));
                if running { painter.circle_filled(center + egui::vec2(0.0, 13.0), 3.0, c.on_container); }
                ui.add_space(8.0);
                ui.label(RichText::new(if running { "Connected" } else { "Disconnected" }).size(30.0).color(c.on_container));
                ui.label(RichText::new(self.selected().map(|p| p.name.as_str()).unwrap_or("Add hosts to get started")).color(c.on_container));
                ui.add_space(16.0);
            });
            ui.add_enabled_ui(running || (!self.busy && self.selected().is_some() && vpn::has_hosts()), |ui| {
                if Self::button(ui, if running { "Disconnect" } else { "Connect" }, c) {
                    self.action(if running { "stopVpn" } else { "requestVpn" });
                }
            });
            ui.add_space(8.0);
        });
        ui.add_space(8.0);
        if let Some(p) = self.selected() {
            ui.label(RichText::new(format!("{} records  ·  {} wildcards", p.records, p.wildcards)).color(c.secondary));
        }
        self.imports(ui);
        if running {
            ui.label(RichText::new(format!("{} local answers", vpn::HITS.load(Ordering::Relaxed))).small().color(c.secondary));
        }
        ui.add_space(8.0);
        ui.label(RichText::new("Local DNS · No root").small().color(c.secondary));
    }

    fn library(&mut self, ui: &mut egui::Ui, c: Colors) {
        ui.heading("Your hosts");
        self.imports(ui);
        if self.state.profiles.is_empty() {
            Self::card(ui, c.surface, |ui| { ui.label("No hosts yet"); ui.label(RichText::new("Import a file or add a URL.").color(c.secondary)); });
        }
        for p in self.state.profiles.clone() {
            let selected = self.state.selected.as_deref() == Some(p.id.as_str());
            ui.push_id(&p.id, |ui| Self::card(ui, if selected { c.container } else { c.surface }, |ui| {
                ui.label(RichText::new(&p.name).size(22.0));
                ui.label(RichText::new(format!("{} records · {}", p.records, if selected { "Selected" } else { "Offline" })).small());
                ui.add_enabled_ui(!self.busy, |ui| {
                    if !selected && ui.button("Use hosts").clicked() {
                        let id = p.id.clone();
                        self.task(move |s| { s.select(&id)?; Ok("Hosts loaded".into()) });
                    }
                    ui.collapsing("More", |ui| {
                        if let Some(url) = &p.url { ui.add(egui::Label::new(RichText::new(url).small()).wrap()); }
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("View").clicked() {
                                if let Some(repo) = &self.repo {
                                    if let Ok(s) = repo.try_lock() {
                                        match s.text(&p) { Ok(t) => self.preview = Some((p.name.clone(), t)), Err(e) => self.message = Some(e) }
                                    }
                                }
                            }
                            if ui.button("Export").clicked() {
                                let profile = p.clone();
                                self.task(move |s| { platform::export(s.text(&profile)?)?; Ok(String::new()) });
                            }
                            if p.url.is_some() && ui.button("Refresh").clicked() { self.download(Some(p.clone())); }
                            if ui.button("Delete").clicked() { self.delete = Some(p.clone()); }
                        });
                    });
                });
            }));
        }
    }

    fn history(&mut self, ui: &mut egui::Ui, c: Colors) {
        ui.heading("History");
        if self.state.history.is_empty() { ui.label("Nothing here yet"); }
        for (i, event) in self.state.history.clone().into_iter().enumerate() {
            ui.push_id(i, |ui| Self::card(ui, c.surface, |ui| {
                ui.label(RichText::new(&event.action).strong());
                ui.label(RichText::new(relative(event.time)).small().color(c.secondary));
                if let Some(profile) = event.snapshot {
                    ui.label(&profile.name);
                    if ui.add_enabled(!self.busy, egui::Button::new("Restore")).clicked() {
                        self.task(move |s| { s.restore(&profile)?; Ok("Restored as a new profile".into()) });
                    }
                }
            }));
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context, c: Colors) {
        let width = (ctx.screen_rect().width() - 64.0).clamp(200.0, 420.0);
        if self.url_dialog {
            let mut close = false;
            egui::Window::new("Add URL").collapsible(false).resizable(false).default_width(width).anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
                for field in 0..2 {
                    let value = if field == 0 { &self.name } else { &self.url };
                    let hint = if field == 0 { "Name (optional)" } else { "https://example.com/hosts.txt" };
                    let text = if value.is_empty() { RichText::new(hint).color(c.secondary) } else { RichText::new(value) };
                    if ui.add_sized([ui.available_width(), 52.0], egui::Button::new(text).fill(c.background)).clicked() {
                        if let Err(e) = platform::edit(field, value) { self.message = Some(e); }
                    }
                }
                ui.label(RichText::new("Use a raw hosts file URL.").small());
                if ui.add_enabled(!self.busy && !self.url.trim().is_empty(), egui::Button::new("Save")).clicked() { self.download(None); close = true; }
                if ui.button("Cancel").clicked() { close = true; }
            });
            if close { self.url_dialog = false; self.action("finishText"); }
        }
        if let Some((name, text)) = &self.preview {
            let mut open = true;
            egui::Window::new(name).open(&mut open).default_width(width).max_height(ctx.screen_rect().height() * 0.65).resizable(false).show(ctx, |ui| {
                ui.label(RichText::new("Preview · first 200 lines").small());
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(text.lines().take(200).collect::<Vec<_>>().join("\n")).monospace()).wrap());
                });
            });
            if !open { self.preview = None; }
        }
        if let Some(p) = self.delete.clone() {
            let mut close = false;
            egui::Window::new("Delete hosts?").collapsible(false).resizable(false).default_width(width).anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
                ui.label("You can restore this version from History.");
                if Self::button(ui, "Delete", c) {
                    self.task(move |s| { s.delete(&p.id)?; Ok("Hosts deleted".into()) });
                    close = true;
                }
                if ui.button("Cancel").clicked() { close = true; }
            });
            if close { self.delete = None; }
        }
        if self.theme_dialog {
            let mut open = true;
            egui::Window::new("Preferences").open(&mut open).resizable(false).collapsible(false).default_width(width).show(ctx, |ui| {
                let mut dynamic = self.state.dynamic;
                if ui.add_enabled(!self.busy, egui::Checkbox::new(&mut dynamic, "Wallpaper colors")).changed() {
                    self.task(move |s| { s.dynamic(dynamic)?; Ok(String::new()) });
                }
                ui.label(RichText::new("Light / dark follows your system.").small());
                ui.separator();
                ui.collapsing("About", |ui| {
                    ui.label("HostNR 0.1.0 · Rust");
                    ui.label("Local DNS, not an encrypted VPN. Only one VPN can run at a time.");
                    ui.label("Private DNS / DoH may bypass hosts. DNS TCP is not supported yet.");
                    ui.label("Unmatched DNS uses Cloudflare, then Quad9. No query logs.");
                });
            });
            if !open { self.theme_dialog = false; }
        }
    }
}

impl eframe::App for HostApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(result) = self.rx.try_recv() {
            self.busy = false;
            self.message = match result { Ok(s) if s.is_empty() => None, Ok(s) => Some(s), Err(e) => Some(e) };
        }
        if let Some(repo) = &self.repo { if let Ok(s) = repo.try_lock() { self.state = s.state.clone(); } }
        for event in platform::events() {
            match event {
                Event::File(name, text) => self.files.push_back((name, text)),
                Event::Message(s) => self.message = Some(s),
                Event::Text(0, text) => self.name = text,
                Event::Text(1, text) => self.url = text,
                Event::Text(_, _) => {}
            }
        }
        if !self.busy {
            if let Some((name, text)) = self.files.pop_front() {
                self.task(move |s| { s.import(name, None, &text, None, None, None)?; Ok("Saved offline".into()) });
            }
        }
        let running = vpn::running();
        if running != self.was_running && !self.busy {
            self.was_running = running;
            self.task(move |s| { s.record(if running { "Connected" } else { "Disconnected" })?; Ok(String::new()) });
        }
        if self.palette_at.elapsed() > Duration::from_secs(1) {
            self.palette = platform::palette().unwrap_or_default();
            self.palette_at = Instant::now();
        }
        let c = Colors::read(self.state.dynamic, &self.palette);
        c.apply(ctx);
        let top = self.palette.get(10).copied().unwrap_or(0).max(0) as f32;
        let bottom = self.palette.get(11).copied().unwrap_or(0).max(0) as f32;
        egui::TopBottomPanel::top("brand").frame(Frame::new().fill(c.background).inner_margin(Margin::symmetric(24, 8))).show(ctx, |ui| {
            ui.add_space(top);
            ui.horizontal(|ui| {
                ui.label(RichText::new("HostNR").size(24.0).strong().color(c.primary));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Theme").clicked() { self.theme_dialog = true; }
                });
            });
        });
        egui::TopBottomPanel::bottom("navigation").frame(Frame::new().fill(c.surface).inner_margin(Margin::symmetric(12, 8))).show(ctx, |ui| {
            ui.columns(3, |cols| {
                for (i, name) in ["Home", "Hosts", "History"].iter().enumerate() {
                    let color = if self.tab == i { c.container } else { c.surface };
                    let width = cols[i].available_width();
                    if cols[i].add_sized([width, 48.0], egui::Button::new(*name).fill(color)).clicked() { self.tab = i; }
                }
            });
            ui.add_space(bottom);
        });
        egui::CentralPanel::default().frame(Frame::new().fill(c.background).inner_margin(Margin::symmetric(20, 12))).show(ctx, |ui| {
            egui::ScrollArea::vertical().id_salt(("page", self.tab)).show(ui, |ui| {
                if self.busy { ui.horizontal(|ui| { ui.spinner(); ui.label("Working…"); }); }
                if let Some(message) = self.message.clone() {
                    Frame::new().fill(c.surface).corner_radius(16).inner_margin(12).show(ui, |ui| {
                        ui.label(message);
                        if ui.small_button("Dismiss").clicked() { self.message = None; }
                    });
                }
                match self.tab { 0 => self.home(ui, c), 1 => self.library(ui, c), _ => self.history(ui, c) }
                ui.add_space(12.0);
            });
        });
        self.dialogs(ctx, c);
        ctx.request_repaint_after(Duration::from_millis(if self.busy { 100 } else { 500 }));
    }
}

fn relative(timestamp: u64) -> String {
    let seconds = store::now().saturating_sub(timestamp);
    if seconds < 60 { "Just now".into() }
    else if seconds < 3600 { format!("{}m ago", seconds / 60) }
    else if seconds < 86400 { format!("{}h ago", seconds / 3600) }
    else { format!("{}d ago", seconds / 86400) }
}
