// main.rs — Finny native desktop GUI (eframe/egui).
// Replaces the old actix-web server + browser UI. Single binary, native window.
// The finance engine (intent/planner/db/retrieval) is reused verbatim via engine.rs.

mod db;
mod dialogue;
mod engine;
mod intent;
mod nlu;
mod planner;
mod retrieval;

use eframe::egui;
use engine::{ChatReply, Engine};
use std::sync::mpsc::{Receiver, Sender};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const CURRENCIES: [&str; 10] = [
    "USD", "EUR", "GBP", "JPY", "CHF", "CAD", "AUD", "CNY", "INR", "TRY",
];
const CHART_METRICS: [&str; 3] = ["policy_rate", "cpi", "yield"];
const CHART_REGIONS: [&str; 4] = ["us", "eu", "uk", "japan"];

fn main() -> eframe::Result<()> {
    // Network is ON by default — live data from central banks, stats agencies,
    // and the World Bank API is fetched automatically. Users can disable via
    // FINNY_NETWORK=0 or the in-app "Live network" toggle.
    let network = std::env::var("FINNY_NETWORK")
        .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
        .unwrap_or(true);
    let selftest = std::env::var("FINNY_SELFTEST")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let engine = Engine::new(network).expect("engine init failed");
    // Guarantee at least one session exists.
    if engine.list_sessions().map(|v| v.is_empty()).unwrap_or(true) {
        let _ = engine.create_session("Session 1");
    }

    let logo = load_logo();

    // Optional initial window size, e.g. FINNY_INITIAL_SIZE=900x700. Handy on
    // small laptops and for headless capture; clamped to the minimums below.
    // Any unparseable value falls back to the default 1200x880.
    let (init_w, init_h) = std::env::var("FINNY_INITIAL_SIZE")
        .ok()
        .and_then(|s| {
            let (a, b) = s.split_once(['x', 'X', ','].as_ref())?;
            let w = a.trim().parse::<f32>().ok()?;
            let h = b.trim().parse::<f32>().ok()?;
            Some((w.max(440.0), h.max(520.0)))
        })
        .unwrap_or((1200.0, 880.0));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([init_w, init_h])
            .with_min_inner_size([440.0, 520.0])
            // Open at the requested size, not auto-maximized — otherwise the
            // window fills the screen and the responsive (compact) layout can
            // never be seen. The user can still maximize via the title bar.
            .with_maximized(false)
            .with_title("Finny — Finance Assistant"),
        ..Default::default()
    };

    eframe::run_native(
        "Finny",
        options,
        Box::new(move |cc| {
            apply_retro_style(&cc.egui_ctx);
            Ok(Box::new(FinnyApp::new(cc, engine, logo, selftest)))
        }),
    )
}

/// Raw decoded logo pixels, uploaded to a texture on first frame.
struct LogoImage {
    size: [usize; 2],
    rgba: Vec<u8>,
}

fn load_logo() -> Option<LogoImage> {
    let bytes = include_bytes!("../assets/logo.png");
    match image::load_from_memory(bytes) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            Some(LogoImage {
                size: [w as usize, h as usize],
                rgba: rgba.into_raw(),
            })
        }
        Err(e) => {
            eprintln!("logo decode failed: {e}");
            None
        }
    }
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Chat,
    Converter,
    Charts,
}

/// Message from worker thread back to UI.
enum WorkerMsg {
    Chat(Result<ChatReply, String>),
    Fx(String),
}

struct FinnyApp {
    engine: Engine,
    logo_raw: Option<LogoImage>,
    logo_tex: Option<egui::TextureHandle>,

    tab: Tab,

    // chat state
    sessions: Vec<db::Session>,
    current_session: Option<i64>,
    chat: Vec<db::Message>,
    input: String,
    thinking: bool,
    last_sources: Vec<String>,
    new_session_title: String,
    scroll_to_bottom: bool,

    // worker channel
    tx: Sender<WorkerMsg>,
    rx: Receiver<WorkerMsg>,

    // converter state
    conv_amount: String,
    conv_from: usize,
    conv_to: usize,
    conv_result: String,
    conv_busy: bool,

    // chart state
    chart_metric: usize,
    chart_region: usize,
    chart_output: String,
    chart_initialized: bool,

    // settings
    text_scale: f32,
    export_status: String,
    confirm_clear: bool,

    // selftest
    selftest: bool,
    frame_count: u32,
}

impl FinnyApp {
    fn new(
        _cc: &eframe::CreationContext<'_>,
        engine: Engine,
        logo: Option<LogoImage>,
        selftest: bool,
    ) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let sessions = engine.list_sessions().unwrap_or_default();
        let current_session = sessions.first().map(|s| s.id);
        let chat = engine.history_for(current_session).unwrap_or_default();
        Self {
            engine,
            logo_raw: logo,
            logo_tex: None,
            tab: Tab::Chat,
            sessions,
            current_session,
            chat,
            input: String::new(),
            thinking: false,
            last_sources: Vec::new(),
            new_session_title: String::new(),
            scroll_to_bottom: true,
            tx,
            rx,
            conv_amount: "100".into(),
            conv_from: 0,
            conv_to: 1,
            conv_result: String::new(),
            conv_busy: false,
            chart_metric: 0,
            chart_region: 0,
            chart_output: String::new(),
            chart_initialized: false,
            text_scale: 1.35,
            export_status: String::new(),
            confirm_clear: false,
            selftest,
            frame_count: 0,
        }
    }

    fn reload_sessions(&mut self) {
        self.sessions = self.engine.list_sessions().unwrap_or_default();
        if self.current_session.is_none() {
            self.current_session = self.sessions.first().map(|s| s.id);
        }
    }

    fn reload_chat(&mut self) {
        self.chat = self.engine.history_for(self.current_session).unwrap_or_default();
    }

    fn send_chat(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() || self.thinking {
            return;
        }
        // Optimistic echo of the user message.
        self.chat.push(db::Message {
            id: None,
            ts: chrono::Utc::now(),
            role: "user".into(),
            text: text.clone(),
            session_id: self.current_session.unwrap_or(1),
            intent: Some("user_input".into()),
            query_plan: None,
        });
        self.input.clear();
        self.thinking = true;
        self.scroll_to_bottom = true;

        // Run the (possibly blocking, network) engine call off the UI thread.
        let engine = self.engine.clone();
        let sid = self.current_session;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                engine.handle_chat(&text, sid)
            }))
            .map_err(|_| "engine panicked".to_string());
            let _ = tx.send(WorkerMsg::Chat(reply));
        });
    }

    fn drain_worker(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                WorkerMsg::Chat(res) => {
                    self.thinking = false;
                    match res {
                        Ok(reply) => {
                            self.current_session = Some(reply.session_id);
                            self.last_sources = reply.sources;
                            self.reload_sessions();
                            self.reload_chat();
                            self.scroll_to_bottom = true;
                        }
                        Err(e) => {
                            self.chat.push(db::Message {
                                id: None,
                                ts: chrono::Utc::now(),
                                role: "assistant".into(),
                                text: format!("[error] {e}"),
                                session_id: self.current_session.unwrap_or(1),
                                intent: Some("error".into()),
                                query_plan: None,
                            });
                        }
                    }
                }
                WorkerMsg::Fx(text) => {
                    self.conv_busy = false;
                    self.conv_result = text;
                }
            }
        }
    }

    fn run_converter(&mut self) {
        if self.conv_busy {
            return;
        }
        let amount = self.conv_amount.trim().to_string();
        if amount.is_empty() {
            return;
        }
        let from = CURRENCIES[self.conv_from];
        let to = CURRENCIES[self.conv_to];
        self.conv_busy = true;
        self.conv_result = "Fetching live rate…".into();
        let engine = self.engine.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                engine.convert_live_or_local(&amount, from, to)
            }))
            .unwrap_or_else(|_| "[error] conversion failed".into());
            let _ = tx.send(WorkerMsg::Fx(out));
        });
    }

    fn run_chart(&mut self) {
        let metric = CHART_METRICS[self.chart_metric];
        let region = CHART_REGIONS[self.chart_region];
        let subject = match metric {
            "policy_rate" => "policy rate",
            "cpi" => "cpi",
            "yield" => "yield",
            _ => "policy rate",
        };
        let q = format!("show {} trend for {} over time", subject, region);
        self.chart_output = self.engine.quick_answer(&q);
    }

    fn export_history(&mut self) {
        let dir = std::env::var("FINNY_DATA_DIR").unwrap_or_else(|_| "./data".into());
        let name = self
            .sessions
            .iter()
            .find(|s| Some(s.id) == self.current_session)
            .map(|s| s.title.clone())
            .unwrap_or_else(|| "session".into());
        let safe: String = name
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '_' })
            .collect();
        let path = format!("{}/finny_export_{}.md", dir.trim_end_matches('/'), safe);
        let mut out = format!("# Finny chat export — {}\n\n", name);
        for m in &self.chat {
            let who = if m.role == "user" { "You" } else { "FINNY" };
            out.push_str(&format!(
                "**{}** ({}):\n\n{}\n\n---\n\n",
                who,
                m.ts.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M"),
                m.text
            ));
        }
        match std::fs::write(&path, out) {
            Ok(_) => self.export_status = format!("Exported to {}", path),
            Err(e) => self.export_status = format!("Export failed: {e}"),
        }
    }
}

impl eframe::App for FinnyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Lazy-upload logo texture.
        if self.logo_tex.is_none() {
            if let Some(l) = &self.logo_raw {
                let img = egui::ColorImage::from_rgba_unmultiplied(l.size, &l.rgba);
                self.logo_tex = Some(ctx.load_texture("finny_logo", img, egui::TextureOptions::LINEAR));
            }
        }

        ctx.set_pixels_per_point(self.text_scale);
        self.drain_worker();

        // Keep repainting while a request is in flight so results appear promptly.
        if self.thinking || self.conv_busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }

        self.render_header(ctx);
        self.render_status_bar(ctx);
        self.render_side_panel(ctx);
        self.render_central(ctx);

        // Headless self-test: render a few frames, print marker, close.
        if self.selftest {
            self.frame_count += 1;
            ctx.request_repaint();
            if self.frame_count >= 3 {
                println!("SELFTEST_OK");
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    // eframe's default clear colour is near-black (12,12,12). Because the
    // central panel uses a transparent frame, that dark colour bled through and
    // made the whole content area dark — clashing with the light retro theme and
    // crushing text contrast. Paint the viewport background with the theme colour.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Color32::from_rgb(244, 240, 232).to_normalized_gamma_f32()
    }
}

impl FinnyApp {
    fn render_header(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("header")
            .frame(
                egui::Frame::default()
                    .fill(egui::Color32::from_rgb(0, 112, 128))
                    .inner_margin(egui::Margin::symmetric(14.0, 10.0)),
            )
            .show(ctx, |ui| {
                let compact = ctx.screen_rect().width() < 720.0;
                ui.horizontal(|ui| {
                    if let Some(tex) = &self.logo_tex {
                        let [w, h] = tex.size();
                        let target_h: f32 = if compact { 44.0 } else { 96.0 };
                        let scale = target_h / h as f32;
                        let size = egui::vec2(w as f32 * scale, target_h);
                        ui.add(egui::Image::new(tex).fit_to_exact_size(size));
                    } else {
                        ui.heading(
                            egui::RichText::new("FINNY").size(if compact { 28.0 } else { 48.0 }).color(egui::Color32::WHITE),
                        );
                    }
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        ui.add_space(if compact { 4.0 } else { 16.0 });
                        ui.label(
                            egui::RichText::new("Local Finance Assistant")
                                .size(if compact { 15.0 } else { 20.0 })
                                .strong()
                                .color(egui::Color32::WHITE),
                        );
                        if !compact {
                            ui.label(
                                egui::RichText::new("native desktop · no cloud · no LLM")
                                    .size(14.0)
                                    .italics()
                                    .color(egui::Color32::from_rgb(220, 245, 245)),
                            );
                        }
                    });
                });
            });
    }

    fn render_status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::default()
                    .fill(egui::Color32::from_rgb(212, 208, 200))
                    .inner_margin(egui::Margin::same(4.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.monospace(
                        egui::RichText::new(format!(
                            " session: {} ",
                            self.sessions
                                .iter()
                                .find(|s| Some(s.id) == self.current_session)
                                .map(|s| s.title.as_str())
                                .unwrap_or("—")
                        ))
                        .size(14.0),
                    );
                    ui.separator();
                    ui.monospace(
                        egui::RichText::new(format!(
                            " network: {} ",
                            if self.engine.network_on() { "ON " } else { "OFF" }
                        ))
                        .size(14.0),
                    );
                    ui.separator();
                    ui.monospace(
                        egui::RichText::new(format!(" evidence: {} ", self.last_sources.len()))
                            .size(14.0),
                    );
                    ui.separator();
                    ui.monospace(
                        egui::RichText::new(format!(" v{} ", VERSION)).size(14.0),
                    );
                    ui.separator();
                    ui.monospace(
                        egui::RichText::new(format!(" {} ", chrono::Local::now().format("%H:%M:%S")))
                            .size(14.0),
                    );
                    if self.thinking {
                        ui.separator();
                        ui.colored_label(
                            egui::Color32::from_rgb(140, 65, 0),
                            egui::RichText::new(" ●●● thinking ").size(14.0),
                        );
                    }
                });
            });
        // Live clock refresh.
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }

    fn render_side_panel(&mut self, ctx: &egui::Context) {
        // Browser-like reflow: on a narrow window the left sidebar collapses
        // into a top navigation strip so the chat/content gets full width.
        let compact = ctx.screen_rect().width() < 720.0;
        let panel_fill = egui::Color32::from_rgb(232, 228, 220);
        if compact {
            egui::TopBottomPanel::top("nav_compact")
                .frame(
                    egui::Frame::default()
                        .fill(panel_fill)
                        .inner_margin(egui::Margin::same(6.0)),
                )
                .show(ctx, |ui| {
                    self.render_tab_bar(ui);
                    ui.horizontal(|ui| {
                        self.render_network_row(ui);
                        ui.separator();
                        ui.collapsing("Sessions & Settings", |ui| {
                            self.render_sessions(ui);
                            ui.add_space(4.0);
                            ui.separator();
                            self.render_settings(ui);
                        });
                    });
                });
        } else {
            egui::SidePanel::left("sessions")
                .resizable(true)
                .default_width(260.0)
                .frame(
                    egui::Frame::default()
                        .fill(panel_fill)
                        .inner_margin(egui::Margin::same(8.0)),
                )
                .show(ctx, |ui| {
                    self.render_tab_bar(ui);
                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(4.0);
                    self.render_network_row(ui);
                    ui.add_space(4.0);
                    ui.separator();
                    self.render_sessions(ui);
                    ui.add_space(4.0);
                    ui.separator();
                    ui.collapsing("Settings", |ui| self.render_settings(ui));
                });
        }
    }

    fn render_tab_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.tab, Tab::Chat, "  Chat  ");
            ui.selectable_value(&mut self.tab, Tab::Converter, "  FX  ");
            ui.selectable_value(&mut self.tab, Tab::Charts, " Charts ");
        });
    }

    fn render_network_row(&mut self, ui: &mut egui::Ui) {
        let mut net = self.engine.network_on();
        if ui.checkbox(&mut net, "Live network").changed() {
            self.engine.set_network(net);
        }
    }

    fn render_sessions(&mut self, ui: &mut egui::Ui) {
        ui.strong("Sessions");
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.new_session_title)
                    .hint_text("new title…")
                    .desired_width(150.0),
            );
            if ui.button(" + ").clicked() {
                let title = if self.new_session_title.trim().is_empty() {
                    format!("Session {}", self.sessions.len() + 1)
                } else {
                    self.new_session_title.trim().to_string()
                };
                if let Ok(s) = self.engine.create_session(&title) {
                    self.current_session = Some(s.id);
                    self.new_session_title.clear();
                    self.reload_sessions();
                    self.reload_chat();
                }
            }
        });
        ui.add_space(4.0);

        egui::ScrollArea::vertical()
            .max_height(320.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let ids: Vec<(i64, String)> =
                    self.sessions.iter().map(|s| (s.id, s.title.clone())).collect();
                for (id, title) in ids {
                    ui.horizontal(|ui| {
                        let selected = Some(id) == self.current_session;
                        if ui.selectable_label(selected, &title).clicked() {
                            self.current_session = Some(id);
                            self.reload_chat();
                            self.scroll_to_bottom = true;
                        }
                        ui.add_space(4.0);
                        if ui.small_button("✕").clicked() {
                            let _ = self.engine.archive_session(Some(id));
                            if self.current_session == Some(id) {
                                self.current_session = None;
                            }
                            self.reload_sessions();
                            self.reload_chat();
                        }
                    });
                }
            });
    }

    fn render_settings(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Text scale").size(14.0));
            ui.add(egui::Slider::new(&mut self.text_scale, 0.8..=2.0));
        });
        ui.horizontal(|ui| {
            if ui.button("Export chat → .md").clicked() {
                self.export_history();
            }
            // Two-step destructive action: first click arms, second confirms.
            let label = if self.confirm_clear { "Confirm clear?" } else { "Clear all data" };
            let armed = self.confirm_clear;
            if ui
                .add_enabled(true, egui::Button::new(egui::RichText::new(label).color(
                    if armed { egui::Color32::from_rgb(150, 0, 0) } else { egui::Color32::from_gray(20) },
                )))
                .clicked()
            {
                if armed {
                    if let Ok(new_id) = self.engine.clear_all_data() {
                        self.current_session = Some(new_id);
                        self.reload_sessions();
                        self.reload_chat();
                        self.last_sources.clear();
                        self.export_status = "All chats & cached data cleared.".into();
                    }
                    self.confirm_clear = false;
                } else {
                    self.confirm_clear = true;
                }
            }
            if armed && ui.small_button("cancel").clicked() {
                self.confirm_clear = false;
            }
        });
        if !self.export_status.is_empty() {
            ui.label(egui::RichText::new(self.export_status.clone()).size(13.0));
        }
    }

    fn render_central(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(egui::Color32::from_rgb(244, 240, 232))
                    .inner_margin(egui::Margin::same(8.0)),
            )
            .show(ctx, |ui| match self.tab {
                Tab::Chat => self.render_chat(ui),
                Tab::Converter => self.render_converter(ui),
                Tab::Charts => self.render_charts(ui),
            });
    }

    fn render_chat(&mut self, ui: &mut egui::Ui) {
        let avail_h = ui.available_height();

        // Chat messages area
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height((avail_h - 140.0).max(120.0))
            .stick_to_bottom(self.scroll_to_bottom)
            .show(ui, |ui| {
                for m in &self.chat {
                    self.render_message(ui, m);
                }
                if self.thinking {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("FINNY")
                                .strong()
                                .size(14.0)
                                .color(egui::Color32::from_rgb(10, 75, 45)),
                        );
                        ui.label(
                            egui::RichText::new("●●●")
                                .size(14.0)
                                .color(egui::Color32::from_rgb(140, 65, 0))
                                .italics(),
                        );
                    });
                }
            });

        // Evidence sources
        if !self.last_sources.is_empty() {
            ui.separator();
            ui.label(egui::RichText::new("Evidence sources:").strong().size(14.0));
            for s in &self.last_sources {
                ui.hyperlink(s);
            }
        }

        ui.separator();

        // Input area — sunken panel
        egui::Frame::none()
            .fill(egui::Color32::from_gray(248))
            .inner_margin(egui::Margin::same(6.0))
            .rounding(egui::Rounding::same(3.0))
            .show(ui, |ui| {
                let compact = ui.ctx().screen_rect().width() < 720.0;
                let hint = "Ask Finny…  (Enter = new line · Ctrl/Cmd+Enter = send)";
                let you_label = || {
                    egui::RichText::new("You:")
                        .strong()
                        .size(14.0)
                        .color(egui::Color32::from_rgb(30, 50, 110))
                };
                let mut send_now = false;

                // Build the editor (+ button) and capture its Response so we can
                // detect Ctrl/Cmd+Enter. Layout differs by width, but the send
                // logic is shared below.
                let resp = if compact {
                    let r = ui
                        .horizontal(|ui| {
                            ui.label(you_label());
                            ui.add(
                                egui::TextEdit::multiline(&mut self.input)
                                    .hint_text(hint)
                                    .desired_rows(2)
                                    .desired_width((ui.available_width() - 44.0).max(80.0)),
                            )
                        })
                        .inner;
                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_enabled(!self.thinking, egui::Button::new(" Ask "))
                                .clicked()
                            {
                                send_now = true;
                            }
                        });
                    });
                    r
                } else {
                    ui.horizontal(|ui| {
                        ui.label(you_label());
                        let r = ui.add(
                            egui::TextEdit::multiline(&mut self.input)
                                .hint_text(hint)
                                .desired_rows(2)
                                .desired_width((ui.available_width() - 70.0).max(80.0)),
                        );
                        if ui
                            .add_enabled(!self.thinking, egui::Button::new("  Ask  "))
                            .clicked()
                        {
                            send_now = true;
                        }
                        r
                    })
                    .inner
                };

                // Send only on Ctrl/Cmd+Enter (or the Ask button). A plain Enter
                // falls through to the multiline editor and inserts a newline,
                // matching the hint so users can type multi-line questions.
                if resp.has_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command)
                {
                    send_now = true;
                }
                if send_now {
                    self.input = self.input.trim_end().to_string();
                    self.send_chat();
                }
            });
    }

    fn render_message(&self, ui: &mut egui::Ui, m: &db::Message) {
        let is_user = m.role == "user";
        let who = if is_user { "You" } else { "FINNY" };
        let color = if is_user {
            egui::Color32::from_rgb(30, 50, 110)
        } else {
            egui::Color32::from_rgb(10, 75, 45)
        };
        let fill = if is_user {
            egui::Color32::from_gray(232)
        } else {
            egui::Color32::from_gray(240)
        };

        egui::Frame::none()
            .fill(fill)
            .inner_margin(egui::Margin::same(8.0))
            .outer_margin(egui::Margin::symmetric(2.0, 3.0))
            .rounding(egui::Rounding::same(4.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(color, egui::RichText::new(who).strong().size(14.0));
                    ui.label(
                        // Timestamps are stored in UTC; show them in the user's
                        // local zone so they agree with the status-bar clock.
                        egui::RichText::new(
                            m.ts.with_timezone(&chrono::Local)
                                .format("%H:%M")
                                .to_string(),
                        )
                            .size(11.0)
                            .color(egui::Color32::from_gray(80)),
                    );
                });
                ui.add_space(4.0);
                render_markdown(ui, &m.text);
            });
    }

    fn render_converter(&mut self, ui: &mut egui::Ui) {
        ui.heading("Currency Converter");
        ui.add_space(8.0);

        // Sunken input area
        egui::Frame::none()
            .fill(egui::Color32::from_gray(248))
            .inner_margin(egui::Margin::same(10.0))
            .rounding(egui::Rounding::same(3.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Amount");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.conv_amount)
                            .desired_width(120.0)
                            .hint_text("100"),
                    );
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    egui::ComboBox::from_label("From")
                        .selected_text(CURRENCIES[self.conv_from])
                        .show_ui(ui, |ui| {
                            for (i, c) in CURRENCIES.iter().enumerate() {
                                ui.selectable_value(&mut self.conv_from, i, *c);
                            }
                        });
                    ui.add_space(4.0);
                    if ui.button("⇄ Swap").clicked() {
                        std::mem::swap(&mut self.conv_from, &mut self.conv_to);
                    }
                    ui.add_space(4.0);
                    egui::ComboBox::from_label("To")
                        .selected_text(CURRENCIES[self.conv_to])
                        .show_ui(ui, |ui| {
                            for (i, c) in CURRENCIES.iter().enumerate() {
                                ui.selectable_value(&mut self.conv_to, i, *c);
                            }
                        });
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!self.conv_busy, egui::Button::new("  Convert  "))
                        .clicked()
                    {
                        self.run_converter();
                    }
                    if self.conv_busy {
                        ui.colored_label(
                            egui::Color32::from_rgb(140, 65, 0),
                            egui::RichText::new("●●● fetching live rate").italics().size(13.0),
                        );
                    }
                });
            });

        ui.add_space(8.0);
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                render_markdown(ui, &self.conv_result);
            });
    }

    fn render_charts(&mut self, ui: &mut egui::Ui) {
        // Auto-render on first open
        if !self.chart_initialized {
            self.run_chart();
            self.chart_initialized = true;
        }

        ui.heading("Trend Charts");
        ui.add_space(8.0);

        // Sunken control panel
        egui::Frame::none()
            .fill(egui::Color32::from_gray(248))
            .inner_margin(egui::Margin::same(10.0))
            .rounding(egui::Rounding::same(3.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_label("Metric")
                        .selected_text(CHART_METRICS[self.chart_metric])
                        .show_ui(ui, |ui| {
                            for (i, m) in CHART_METRICS.iter().enumerate() {
                                ui.selectable_value(&mut self.chart_metric, i, *m);
                            }
                        });
                    ui.add_space(8.0);
                    egui::ComboBox::from_label("Region")
                        .selected_text(CHART_REGIONS[self.chart_region])
                        .show_ui(ui, |ui| {
                            for (i, r) in CHART_REGIONS.iter().enumerate() {
                                ui.selectable_value(&mut self.chart_region, i, *r);
                            }
                        });
                    ui.add_space(8.0);
                    if ui.button("  Render  ").clicked() {
                        self.run_chart();
                    }
                });
            });

        ui.add_space(8.0);
        ui.separator();
        // Render through the markdown renderer so the fenced ASCII chart becomes a
        // boxed monospace block and the summary renders as real headings/lists
        // (previously the whole blob was dumped as raw monospace, showing literal
        // `##` / `-` markers).
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                render_markdown(ui, &self.chart_output);
            });
    }
}

/// Render markdown text into egui widgets.
/// Supports: headers (# ## ###), bold (**), italic (*), inline code (`),
/// code blocks (```), blockquotes (>), unordered lists (-), tables (| |), and --- separators.
fn render_markdown(ui: &mut egui::Ui, text: &str) {
    let mut in_code_block = false;
    let mut code_buf = String::new();
    // Each fenced code block gets its own ScrollArea id. A single hard-coded
    // id (the previous behaviour) made every code block in a frame share one
    // egui widget id, so two+ blocks (e.g. a comparison chart and a trend
    // chart in the same log) collided and the layout never settled — visible
    // as a flicker/"loop". A per-block counter keeps ids unique.
    let mut code_idx: u32 = 0;

    for line in text.lines() {
        let trimmed = line.trim();

        // Code block toggle
        if trimmed.starts_with("```") {
            if in_code_block {
                // End code block — show as a boxed, horizontally-scrollable,
                // non-wrapping monospace panel so ASCII charts keep their column
                // alignment instead of being reflowed into the prose width.
                egui::Frame::none()
                    .fill(egui::Color32::from_gray(238))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(170)))
                    .inner_margin(egui::Margin::same(6.0))
                    .rounding(egui::Rounding::same(3.0))
                    .show(ui, |ui| {
                        egui::ScrollArea::horizontal()
                            .id_source(format!("code_scroll_{code_idx}"))
                            // Shrink vertically to hug the chart (was [false,false]:
                            // the box claimed all remaining chat height, leaving a
                            // giant empty frame under short charts). Width stays
                            // full so wide charts scroll horizontally.
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(code_buf.trim_end())
                                            .monospace()
                                            .size(13.0)
                                            .color(egui::Color32::from_rgb(25, 25, 25)),
                                    )
                                    .wrap_mode(egui::TextWrapMode::Extend),
                                );
                            });
                    });
                in_code_block = false;
                code_buf.clear();
                code_idx += 1;
            } else {
                in_code_block = true;
            }
            continue;
        }

        if in_code_block {
            code_buf.push_str(line);
            code_buf.push('\n');
            continue;
        }

        // Empty line
        if trimmed.is_empty() {
            ui.add_space(4.0);
            continue;
        }

        // Horizontal rule
        if trimmed == "---" || trimmed == "***" {
            ui.separator();
            continue;
        }

        // Blockquote — fragments flow inline and wrap (horizontal_wrapped).
        if trimmed.starts_with('>') {
            let quote = trimmed.trim_start_matches('>').trim();
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.add_space(4.0);
                ui.colored_label(
                    egui::Color32::from_gray(70),
                    egui::RichText::new("│ ").monospace().size(14.0),
                );
                render_inline(ui, quote, 14.0, true);
            });
            continue;
        }

        // Unordered list — fragments flow inline and wrap.
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            let item = trimmed[2..].trim();
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.add_space(8.0);
                ui.label(egui::RichText::new("• ").size(14.0));
                render_inline(ui, item, 14.0, false);
            });
            continue;
        }

        // Table row
        if trimmed.starts_with('|') && trimmed.ends_with('|') && trimmed.contains("│") {
            // Skip separator rows like |---|---|
            if trimmed.contains("---") || trimmed.contains("───") {
                continue;
            }
            let cells: Vec<&str> = trimmed
                .trim_matches('|')
                .split('│')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();
            if !cells.is_empty() {
                ui.horizontal(|ui| {
                    for (i, cell) in cells.iter().enumerate() {
                        if i > 0 {
                            // Monospace: the proportional font lacks U+2502 and
                            // rendered it as a tofu box.
                            ui.label(egui::RichText::new("│").monospace().size(13.0));
                        }
                        render_inline(ui, cell, 13.0, false);
                    }
                });
            }
            continue;
        }
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            if trimmed.contains("---") {
                continue;
            }
            let cells: Vec<&str> = trimmed
                .trim_matches('|')
                .split('|')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();
            if !cells.is_empty() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (i, cell) in cells.iter().enumerate() {
                        if i > 0 {
                            ui.label(egui::RichText::new("│").monospace().size(13.0));
                        }
                        render_inline(ui, cell, 13.0, false);
                    }
                });
            }
            continue;
        }

        // Headers
        if trimmed.starts_with("### ") {
            ui.label(
                egui::RichText::new(trimmed[4..].trim())
                    .size(16.0)
                    .strong()
                    .color(egui::Color32::from_rgb(10, 45, 100)),
            );
            continue;
        }
        if trimmed.starts_with("## ") {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(trimmed[3..].trim())
                    .size(18.0)
                    .strong()
                    .color(egui::Color32::from_rgb(10, 45, 100)),
            );
            ui.add_space(2.0);
            continue;
        }
        if trimmed.starts_with("# ") {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(trimmed[2..].trim())
                    .size(22.0)
                    .strong()
                    .color(egui::Color32::from_rgb(0, 65, 65)),
            );
            ui.add_space(4.0);
            continue;
        }

        // Regular paragraph — bold/italic/code fragments must flow INLINE and
        // wrap with the text, not stack one-per-line (plain ui.label calls lay
        // out vertically). horizontal_wrapped + zero spacing = normal prose.
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            render_inline(ui, trimmed, 14.0, false);
        });
    }
}

/// Render inline markdown: **bold**, *italic*, _italic_, `code` — nestable.
///
/// The previous implementation searched for each delimiter TYPE in a fixed
/// order (` → ** → *), so a later `**bold**` was processed before an earlier
/// `_italic_` opener and the opener leaked into the literal text (every
/// `_Source: … **Live network** …_` footer rendered with raw underscores).
/// This version always processes the EARLIEST opening delimiter, then renders
/// the span's inner text recursively — so mixed/nested markup just works.
fn render_inline(ui: &mut egui::Ui, text: &str, size: f32, italic: bool) {
    render_inline_styled(ui, text, size, italic, false);
}

fn render_inline_styled(ui: &mut egui::Ui, text: &str, size: f32, italic: bool, bold: bool) {
    let mut remaining = text;
    while !remaining.is_empty() {
        match next_span(remaining) {
            Some((ms, is, ie, me, kind)) => {
                if ms > 0 {
                    ui.label(styled_text(&remaining[..ms], size, italic, bold));
                }
                let inner = &remaining[is..ie];
                match kind {
                    SpanKind::Code => {
                        ui.monospace(
                            egui::RichText::new(inner)
                                .size(size - 1.0)
                                .color(egui::Color32::from_rgb(30, 30, 30))
                                .background_color(egui::Color32::from_gray(230)),
                        );
                    }
                    SpanKind::Bold => render_inline_styled(ui, inner, size, italic, true),
                    SpanKind::Italic => render_inline_styled(ui, inner, size, true, bold),
                }
                remaining = &remaining[me..];
            }
            None => {
                ui.label(styled_text(remaining, size, italic, bold));
                break;
            }
        }
    }
}

/// Inline span kinds; declaration order doubles as the tie-break priority
/// (code > bold > italic) when two spans open at the same position.
#[derive(Clone, Copy, PartialEq)]
enum SpanKind {
    Code,
    Bold,
    Italic,
}

/// Find the next complete inline span of any supported kind. Returns
/// (match_start, inner_start, inner_end, match_end, kind) for the span whose
/// opening delimiter is earliest, or None when no complete span remains.
fn next_span(s: &str) -> Option<(usize, usize, usize, usize, SpanKind)> {
    let mut best: Option<(usize, usize, usize, usize, SpanKind)> = None;
    {
        let mut consider = |cand: Option<(usize, usize, usize, usize, SpanKind)>| {
            if let Some(c) = cand {
                let better = match &best {
                    None => true,
                    Some(b) => c.0 < b.0 || (c.0 == b.0 && (c.4 as u8) < (b.4 as u8)),
                };
                if better {
                    best = Some(c);
                }
            }
        };

        // `code`
        consider(s.find('`').and_then(|st| {
            s[st + 1..]
                .find('`')
                .map(|en| (st, st + 1, st + 1 + en, st + 1 + en + 1, SpanKind::Code))
        }));
        // **bold**
        consider(s.find("**").and_then(|st| {
            s[st + 2..]
                .find("**")
                .map(|en| (st, st + 2, st + 2 + en, st + 2 + en + 2, SpanKind::Bold))
        }));
        // *italic* — single '*' only (never part of a '**' pair)
        consider(single_delim_span(s, b'*', false).map(
            |(st, en)| (st, st + 1, en, en + 1, SpanKind::Italic),
        ));
        // _italic_ — CommonMark boundary rules (intraword '_' stays literal,
        // so snake_case like policy_rate is untouched)
        consider(single_delim_span(s, b'_', true).map(
            |(st, en)| (st, st + 1, en, en + 1, SpanKind::Italic),
        ));
    }
    best
}

/// Find a matched pair of single-byte delimiters. With `word_boundaries`
/// (underscores), an opener must not be intraword and a closer must end a
/// word — so `policy_rate` stays literal while `_Source: …_` emphasises.
/// Returns (open_pos, close_pos).
fn single_delim_span(s: &str, delim: u8, word_boundaries: bool) -> Option<(usize, usize)> {
    let bytes = s.as_bytes();
    let is_ws_or_punct = |b: u8| b.is_ascii_whitespace() || b.is_ascii_punctuation();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == delim {
            let prev_ok = if word_boundaries {
                // String start is a valid left boundary (e.g. "_Source: …_").
                i == 0 || is_ws_or_punct(bytes[i - 1])
            } else {
                // '*' delimiters: never split a '**' pair
                i == 0 || bytes[i - 1] != delim
            };
            let pair_ok = bytes.get(i + 1).copied() != Some(delim);
            let next_non_ws = bytes
                .get(i + 1)
                .map(|b| !b.is_ascii_whitespace())
                .unwrap_or(false);
            if prev_ok && pair_ok && next_non_ws {
                // Opening found — scan for the closer.
                let mut j = i + 1;
                while j < bytes.len() {
                    if bytes[j] == delim && bytes[j - 1] != delim {
                        let close_prev_ok = !bytes[j - 1].is_ascii_whitespace();
                        let close_next_ok = if word_boundaries {
                            bytes.get(j + 1).copied().map(is_ws_or_punct).unwrap_or(true)
                        } else {
                            bytes.get(j + 1).copied() != Some(delim)
                        };
                        if close_prev_ok && close_next_ok {
                            return Some((i, j));
                        }
                    }
                    j += 1;
                }
                return None; // opener without closer → literal
            }
        }
        i += 1;
    }
    None
}

fn styled_text(text: &str, size: f32, italic: bool, bold: bool) -> egui::RichText {
    let mut rt = egui::RichText::new(text).size(size);
    if italic {
        rt = rt.italics();
    }
    if bold {
        rt = rt.strong();
    }
    rt
}

fn apply_retro_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    let v = &mut style.visuals;
    // Classic gray beveled desktop look (Windows 2000 / Java Swing)
    v.window_fill = egui::Color32::from_rgb(240, 236, 228);
    v.panel_fill = egui::Color32::from_rgb(240, 236, 228);
    // Dark, high-contrast text on the light panels.
    let ink = egui::Color32::from_gray(20);
    v.override_text_color = Some(ink);

    // A widget's text colour is its per-state `fg_stroke.color` (and strong text
    // uses `widgets.active.fg_stroke`). The cloned base style left these at the
    // dark-theme defaults (light/medium gray), which rendered as washed-out text
    // on our light panels — measured ~0.86 and ~0.55 luminance glyphs on a 0.91
    // panel. Force every interactive state's foreground dark. (The white-on-teal
    // header is unaffected: it sets its colour explicitly via RichText.)
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.fg_stroke.color = ink;
    }

    // Widget bevels
    v.widgets.inactive.bg_fill = egui::Color32::from_rgb(220, 216, 208);
    v.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(220, 216, 208);
    v.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_gray(140));
    v.widgets.inactive.rounding = egui::Rounding::same(2.0);

    v.widgets.hovered.bg_fill = egui::Color32::from_rgb(230, 228, 220);
    v.widgets.hovered.rounding = egui::Rounding::same(2.0);

    v.widgets.active.bg_fill = egui::Color32::from_rgb(200, 196, 188);
    v.widgets.active.rounding = egui::Rounding::same(2.0);

    v.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(212, 208, 200);
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_gray(160));
    v.widgets.noninteractive.rounding = egui::Rounding::same(2.0);

    v.faint_bg_color = egui::Color32::from_gray(224);
    v.extreme_bg_color = egui::Color32::from_gray(248);

    // Selection highlight. Because some widget paths draw the *selected* text
    // with the active/open foreground (now dark) and others with selection.stroke
    // (also dark below), the highlight MUST be light so selected text stays
    // readable. A pale teal keeps the retro feel and a clear selected state.
    v.selection.bg_fill = egui::Color32::from_rgb(176, 210, 214);
    v.selection.stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(0, 90, 110));

    // Hyperlink color — must pass WCAG AA on light background
    v.hyperlink_color = egui::Color32::from_rgb(0, 100, 120);

    ctx.set_style(style);
}

#[cfg(test)]
mod md_tests {
    //! Pins for the inline-markdown span parser: the chat renderer's output
    //! for every assistant message depends on these boundaries.
    use super::*;

    fn span(s: &str) -> Option<(usize, usize, usize, usize, &'static str)> {
        next_span(s).map(|(a, b, c, d, k)| {
            (a, b, c, d, match k {
                SpanKind::Code => "code",
                SpanKind::Bold => "bold",
                SpanKind::Italic => "italic",
            })
        })
    }

    #[test]
    fn underscore_italic_at_string_start() {
        // The dialogue footer's exact shape — opener at position 0 must match.
        let s = "_Source: BLS · figures are approximate._";
        let (ms, is, ie, me, k) = span(s).unwrap();
        assert_eq!((ms, is, ie, me, k), (0, 1, s.len() - 1, s.len(), "italic"));
        assert_eq!(&s[is..ie], "Source: BLS · figures are approximate.");
    }

    #[test]
    fn intraword_underscore_stays_literal() {
        assert!(span("policy_rate").is_none());
        assert!(span("show policy_rate trend").is_none());
    }

    #[test]
    fn earliest_delimiter_wins_over_type_priority() {
        // '_' at 0 must beat '**' at 11 (the old renderer processed bold first
        // and leaked the underscore opener into literal text).
        let s = "_figures — enable **Live network** for data._";
        let (ms, _, _, _, k) = span(s).unwrap();
        assert_eq!((ms, k), (0, "italic"));
    }

    #[test]
    fn bold_code_and_star_italic() {
        assert_eq!(span("a **b** c").map(|(a, _, _, _, k)| (a, k)), Some((2, "bold")));
        assert_eq!(span("a `b` c").map(|(a, _, _, _, k)| (a, k)), Some((2, "code")));
        assert_eq!(span("a *b* c").map(|(a, _, _, _, k)| (a, k)), Some((2, "italic")));
        // '**' is never mistaken for two single-'*' italics.
        assert_eq!(span("**bold**").map(|(_, _, _, _, k)| k), Some("bold"));
    }
}
