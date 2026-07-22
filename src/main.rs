// main.rs — Finny native desktop GUI (eframe/egui).
// Replaces the old actix-web server + browser UI. Single binary, native window.
// The finance engine (intent/planner/db/retrieval) is reused verbatim via engine.rs.

mod db;
mod engine;
mod intent;
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

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 880.0])
            .with_min_inner_size([960.0, 680.0])
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

    // chart state
    chart_metric: usize,
    chart_region: usize,
    chart_output: String,
    chart_initialized: bool,

    // settings
    text_scale: f32,
    export_status: String,

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
            chart_metric: 0,
            chart_region: 0,
            chart_output: String::new(),
            chart_initialized: false,
            text_scale: 1.35,
            export_status: String::new(),
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
            }
        }
    }

    fn run_converter(&mut self) {
        let amount = self.conv_amount.trim();
        let from = CURRENCIES[self.conv_from];
        let to = CURRENCIES[self.conv_to];
        let q = format!("convert {} {} to {}", amount, from, to);
        self.conv_result = self.engine.quick_answer(&q);
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
                m.ts.format("%Y-%m-%d %H:%M"),
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
        if self.thinking {
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
                ui.horizontal(|ui| {
                    if let Some(tex) = &self.logo_tex {
                        let [w, h] = tex.size();
                        let target_h: f32 = 96.0;
                        let scale = target_h / h as f32;
                        let size = egui::vec2(w as f32 * scale, target_h);
                        ui.add(egui::Image::new(tex).fit_to_exact_size(size));
                    } else {
                        ui.heading(
                            egui::RichText::new("FINNY").size(48.0).color(egui::Color32::WHITE),
                        );
                    }
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        ui.add_space(16.0);
                        ui.label(
                            egui::RichText::new("Local Finance Assistant")
                                .size(20.0)
                                .strong()
                                .color(egui::Color32::WHITE),
                        );
                        ui.label(
                            egui::RichText::new("native desktop · no cloud · no LLM")
                                .size(14.0)
                                .italics()
                                .color(egui::Color32::from_rgb(220, 245, 245)),
                        );
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
        egui::SidePanel::left("sessions")
            .resizable(true)
            .default_width(260.0)
            .frame(
                egui::Frame::default()
                    .fill(egui::Color32::from_rgb(232, 228, 220))
                    .inner_margin(egui::Margin::same(8.0)),
            )
            .show(ctx, |ui| {
                // Tab bar
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tab, Tab::Chat, "  Chat  ");
                    ui.selectable_value(&mut self.tab, Tab::Converter, "  FX  ");
                    ui.selectable_value(&mut self.tab, Tab::Charts, " Charts ");
                });
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                // Network toggle
                let mut net = self.engine.network_on();
                if ui.checkbox(&mut net, "Live network").changed() {
                    self.engine.set_network(net);
                }
                ui.add_space(4.0);
                ui.separator();

                // Sessions
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

                ui.add_space(4.0);
                ui.separator();
                ui.collapsing("Settings", |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Text scale").size(14.0));
                        ui.add(egui::Slider::new(&mut self.text_scale, 0.8..=2.0));
                    });
                    if ui.button("Export chat → .md").clicked() {
                        self.export_history();
                    }
                    if !self.export_status.is_empty() {
                        ui.label(egui::RichText::new(self.export_status.clone()).size(13.0));
                    }
                });
            });
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
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("You:")
                            .strong()
                            .size(14.0)
                            .color(egui::Color32::from_rgb(30, 50, 110)),
                    );
                    let resp = ui.add(
                        egui::TextEdit::multiline(&mut self.input)
                            .hint_text("Ask Finny…  (Enter = new line · Ctrl/Cmd+Enter = send)")
                            .desired_rows(2)
                            .desired_width(ui.available_width() - 70.0),
                    );
                    let mut send_now = false;
                    // Send only on Ctrl/Cmd+Enter (or the Ask button). A plain Enter
                    // falls through to the multiline editor and inserts a newline,
                    // which matches the hint and lets users type multi-line questions.
                    // The previous plain-Enter branch sent on every Return, so a user
                    // following the hint (pressing Enter for a newline) sent the
                    // message prematurely.
                    if resp.has_focus()
                        && ui.input(|i| {
                            i.key_pressed(egui::Key::Enter) && i.modifiers.command
                        })
                    {
                        send_now = true;
                    }
                    if ui
                        .add_enabled(!self.thinking, egui::Button::new("  Ask  "))
                        .clicked()
                    {
                        send_now = true;
                    }
                    if send_now {
                        // Ctrl+Enter makes the multiline editor drop a stray newline
                        // at the cursor; trim trailing whitespace before sending.
                        self.input = self.input.trim_end().to_string();
                        self.send_chat();
                    }
                });
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
                        egui::RichText::new(m.ts.format("%H:%M").to_string())
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
                if ui.button("  Convert  ").clicked() {
                    self.run_converter();
                }
            });

        ui.add_space(8.0);
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
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
                            .id_source("code_scroll")
                            .auto_shrink([false, false])
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

        // Blockquote
        if trimmed.starts_with('>') {
            let quote = trimmed.trim_start_matches('>').trim();
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                ui.colored_label(
                    egui::Color32::from_gray(70),
                    egui::RichText::new("│ ").monospace().size(14.0),
                );
                render_inline(ui, quote, 14.0, true);
            });
            continue;
        }

        // Unordered list
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            let item = trimmed[2..].trim();
            ui.horizontal(|ui| {
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
                            ui.label(egui::RichText::new(" │ ").size(13.0));
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
                    for (i, cell) in cells.iter().enumerate() {
                        if i > 0 {
                            ui.label(egui::RichText::new(" │ ").size(13.0));
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

        // Regular paragraph
        render_inline(ui, trimmed, 14.0, false);
    }
}

/// Render inline markdown: **bold**, *italic*, `code`.
fn render_inline(ui: &mut egui::Ui, text: &str, size: f32, italic: bool) {
    let mut remaining = text;
    while !remaining.is_empty() {
        // Inline code
        if let Some(start) = remaining.find('`') {
            let before = &remaining[..start];
            if let Some(end) = remaining[start + 1..].find('`') {
                let code = &remaining[start + 1..start + 1 + end];
                let after = &remaining[start + 1 + end + 1..];
                if !before.is_empty() {
                    ui.label(styled_text(before, size, italic, false));
                }
                ui.monospace(
                    egui::RichText::new(code)
                        .size(size - 1.0)
                        .color(egui::Color32::from_rgb(30, 30, 30))
                        .background_color(egui::Color32::from_gray(230)),
                );
                remaining = after;
                continue;
            }
        }

        // Bold
        if let Some(start) = remaining.find("**") {
            let before = &remaining[..start];
            if let Some(end) = remaining[start + 2..].find("**") {
                let bold_text = &remaining[start + 2..start + 2 + end];
                let after = &remaining[start + 2 + end + 2..];
                if !before.is_empty() {
                    ui.label(styled_text(before, size, italic, false));
                }
                ui.label(
                    egui::RichText::new(bold_text)
                        .size(size)
                        .strong()
                        .color(egui::Color32::BLACK),
                );
                remaining = after;
                continue;
            }
        }

        // Italic
        if let Some(start) = remaining.find('*') {
            let before = &remaining[..start];
            if let Some(end) = remaining[start + 1..].find('*') {
                let italic_text = &remaining[start + 1..start + 1 + end];
                let after = &remaining[start + 1 + end + 1..];
                if !before.is_empty() {
                    ui.label(styled_text(before, size, italic, false));
                }
                ui.label(
                    egui::RichText::new(italic_text)
                        .size(size)
                        .italics()
                        .color(egui::Color32::from_gray(40)),
                );
                remaining = after;
                continue;
            }
        }

        // No more markdown
        ui.label(styled_text(remaining, size, italic, false));
        break;
    }
}

fn styled_text(text: &str, size: f32, italic: bool, _bold: bool) -> egui::RichText {
    let mut rt = egui::RichText::new(text).size(size);
    if italic {
        rt = rt.italics();
    }
    rt
}

fn apply_retro_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    let v = &mut style.visuals;
    // Classic gray beveled desktop look (Windows 2000 / Java Swing)
    v.window_fill = egui::Color32::from_rgb(240, 236, 228);
    v.panel_fill = egui::Color32::from_rgb(240, 236, 228);
    v.override_text_color = Some(egui::Color32::from_gray(20));

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

    // Selection color (classic teal)
    v.selection.bg_fill = egui::Color32::from_rgb(0, 120, 140);
    v.selection.stroke = egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(0, 80, 100));

    // Hyperlink color — must pass WCAG AA on light background
    v.hyperlink_color = egui::Color32::from_rgb(0, 100, 120);

    ctx.set_style(style);
}
