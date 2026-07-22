// engine.rs — Finny finance engine wrapper (ported verbatim from app.rs).
// Contains the `handle_chat` pipeline (classify -> plan -> optional live fetch ->
// persist) as PLAIN functions callable from the native GUI. No actix, no web::block.

use crate::db::{Db, Message, Session};
use crate::intent::{IntentClassifier, SlotBindings};
use crate::planner::QueryPlanner;
use crate::retrieval::Retriever;
use anyhow::Result;
use chrono::{DateTime, Utc};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Result of one chat turn.
#[derive(Clone, Debug)]
pub struct ChatReply {
    pub session_id: i64,
    pub sources: Vec<String>,
}

#[derive(Clone)]
pub struct Engine {
    pub db: Db,
    pub retriever: Arc<Retriever>,
    pub classifier: IntentClassifier,
    pub planner: QueryPlanner,
    pub use_network: Arc<AtomicBool>,
}

impl Engine {
    pub fn new(network_enabled: bool) -> Result<Self> {
        let db = Db::open_default()?;
        db.init()?;
        Ok(Self {
            db,
            retriever: Arc::new(Retriever::new(12)?),
            classifier: IntentClassifier::new(),
            planner: QueryPlanner::new(),
            use_network: Arc::new(AtomicBool::new(network_enabled)),
        })
    }

    /// Full chat pipeline — preserved verbatim from app.rs::handle_chat, minus
    /// the serde_json envelope. Runs on a worker thread (may block on network).
    pub fn handle_chat(&self, text: &str, session_id: Option<i64>) -> ChatReply {
        let slots = self.classifier.classify(text);
        let mut answer = self.planner.plan_and_answer(&slots, text);
        let mut sources: Vec<String> = Vec::new();

        // Network-augmented intents: anything that asks for current or
        // historical data can benefit from live sources.
        let is_query_like = matches!(
            slots.intent,
            crate::intent::IntentClass::DirectLookup
                | crate::intent::IntentClass::Comparison(_)
                | crate::intent::IntentClass::Trend
                | crate::intent::IntentClass::Forecast
                | crate::intent::IntentClass::AssetPrice
                | crate::intent::IntentClass::CausalScenario
        );

        if is_query_like && self.use_network.load(Ordering::Relaxed) {
            // Build keyword list for excerpt matching from all slot fields.
            let keywords = self.excerpt_keywords(&slots);
            let mut fetched = false;

            // Phase 1: fetch from known central-bank / stats-agency pages.
            if let Some(cands) = self.lookup_candidate_urls(&slots) {
                for url in cands {
                    if let Ok(page) = self.retriever.fetch(&url) {
                        let body = crate::retrieval::html_to_text(&page.body);
                        let excerpts = crate::retrieval::excerpts_around_keywords(
                            &body,
                            &keywords,
                            3,
                            150,
                        );
                        if !excerpts.is_empty() {
                            self.persist_source(
                                &page.url,
                                &page.content_sha256,
                                page.retrieved_at,
                            );
                            let s = format!(
                                "\n\n## Live Excerpts ({})\n{}",
                                page.url,
                                excerpts
                                    .iter()
                                    .map(|e| format!("> {}", e))
                                    .collect::<Vec<_>>()
                                    .join("\n\n")
                            );
                            answer.push_str(&s);
                            sources.push(page.url);
                            fetched = true;
                            break;
                        }
                    }
                }
            }

            // Phase 2: World Bank API — broad country × indicator coverage.
            if !fetched {
                if let Some((country, indicator)) = self.lookup_worldbank(&slots) {
                    if let Ok(data) = self.retriever.fetch_worldbank(country, indicator) {
                        self.persist_source(
                            &data.source_url,
                            &data.sha256,
                            data.retrieved_at,
                        );
                        answer.push_str(&format!(
                            "\n\n## Live Data (World Bank API)\n{}",
                            data.text
                        ));
                        sources.push(data.source_url);
                        fetched = true;
                    }
                }
            }

            // Phase 3: BLS Public API — detailed US economic series.
            if !fetched {
                if let Some(series) = self.lookup_bls(&slots) {
                    if let Ok(data) = self.retriever.fetch_bls(series) {
                        self.persist_source(
                            &data.source_url,
                            &data.sha256,
                            data.retrieved_at,
                        );
                        answer.push_str(&format!(
                            "\n\n## Live Data (BLS Public API)\n{}",
                            data.text
                        ));
                        sources.push(data.source_url);
                    }
                }
            }
        }

        let effective_session = match session_id {
            Some(id) => self.db.ensure_session(Some(id)).unwrap_or(id),
            None => self
                .db
                .list_sessions(false)
                .ok()
                .and_then(|v| v.first().map(|x| x.id))
                .unwrap_or(1),
        };

        let _ = self.db.add_message(&Message {
            id: None,
            ts: chrono::Utc::now(),
            role: "user".into(),
            text: text.into(),
            session_id: effective_session,
            intent: Some("user_input".into()),
            query_plan: None,
        });
        let _ = self.db.add_message(&Message {
            id: None,
            ts: chrono::Utc::now(),
            role: "assistant".into(),
            text: answer.clone(),
            session_id: effective_session,
            intent: Some(format!("{:?}", slots.intent.intent_class())),
            query_plan: None,
        });

        ChatReply {
            session_id: effective_session,
            sources,
        }
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>> {
        self.db.list_sessions(false)
    }

    pub fn create_session(&self, title: &str) -> Result<Session> {
        self.db.create_session(title)
    }

    pub fn history_for(&self, session_id: Option<i64>) -> Result<Vec<Message>> {
        let sid = match session_id {
            Some(id) => id,
            None => self
                .db
                .list_sessions(false)?
                .first()
                .map(|x| x.id)
                .unwrap_or(1),
        };
        self.db.messages_for(sid)
    }

    pub fn set_network(&self, enabled: bool) {
        self.use_network.store(enabled, Ordering::Relaxed);
    }

    pub fn network_on(&self) -> bool {
        self.use_network.load(Ordering::Relaxed)
    }

    pub fn archive_session(&self, id: Option<i64>) -> Result<()> {
        self.db.archive_session(id)
    }

    /// Direct engine call for the currency converter / chart panels (no network,
    /// no persistence) — reuses the planner exactly as chat does.
    pub fn quick_answer(&self, text: &str) -> String {
        let slots = self.classifier.classify(text);
        self.planner.plan_and_answer(&slots, text)
    }

    /// Record a live source in the provenance table (audit trail).
    fn persist_source(&self, url: &str, sha256: &str, retrieved_at: DateTime<Utc>) {
        let _ = self.db.add_source(&crate::db::Source {
            id: None,
            url: url.to_string(),
            retrieved_at,
            domain_tier: 1,
            purpose: format!("chat:{}", sha256),
        });
    }

    /// Build a list of excerpt keywords from all slot fields.
    fn excerpt_keywords<'a>(&self, slots: &'a SlotBindings) -> Vec<&'a str> {
        let mut kw = Vec::with_capacity(4);
        if let Some(s) = slots.subject.as_deref() {
            if !kw.contains(&s) {
                kw.push(s);
            }
        }
        if let Some(m) = slots.metric.as_deref() {
            if !kw.contains(&m) {
                kw.push(m);
            }
        }
        if let Some(g) = slots.geography.as_deref() {
            if !kw.contains(&g) {
                kw.push(g);
            }
        }
        if kw.is_empty() {
            kw.push("rate");
        }
        kw
    }

    fn lookup_candidate_urls(&self, slots: &SlotBindings) -> Option<Vec<String>> {
        let mut urls = Vec::new();
        let subject = slots.subject.as_deref()?;
        let geo = slots.geography.as_deref()?;
        match (subject, geo) {
            // --- Central banks: policy rates ---
            ("policy rate", "fed") | ("rate", "fed") => {
                urls.push("https://www.federalreserve.gov/releases/h15/".into());
            }
            ("policy rate", "ecb") | ("rate", "ecb") => {
                urls.push("https://www.ecb.europa.eu/stats/policy_and_exchange_rates/key_ecb_interest_rates/html/index.en.html".into());
            }
            ("policy rate", "boe") | ("rate", "boe") => {
                urls.push("https://www.bankofengland.co.uk/boeapps/database/Bank-Rate.asp".into());
            }
            ("policy rate", "boj") | ("rate", "boj") => {
                urls.push("https://www.boj.or.jp/en/mopo/mpmsche_minu/".into());
                urls.push("https://www.boj.or.jp/en/statistics/boj/other/short/relent.pdf".into());
            }
            // --- CPI / inflation ---
            ("cpi", "us") | ("inflation", "us") => {
                urls.push("https://www.bls.gov/opub/ted/".into());
                urls.push("https://www.bls.gov/schedule/news_release/cpi.htm".into());
            }
            ("cpi", "uk") | ("inflation", "uk") => {
                urls.push("https://www.ons.gov.uk/economy/inflationandpriceindices".into());
            }
            ("cpi", "eu") | ("inflation", "eu") | ("cpi", "eurozone") | ("inflation", "eurozone") => {
                urls.push("https://ec.europa.eu/eurostat/statistics-explained/index.php?title=Inflation_in_the_euro_area".into());
            }
            ("cpi", "turkey") | ("inflation", "turkey") => {
                urls.push("https://data.tuik.gov.tr/Kategori/GetKategori?p=Price-Statistics-106".into());
            }
            // --- GDP ---
            ("gdp", "us") => {
                urls.push("https://www.bea.gov/data/gdp/gross-domestic-product".into());
            }
            ("gdp", "uk") => {
                urls.push("https://www.ons.gov.uk/economy/grossdomesticproductgdp".into());
            }
            // --- Yields ---
            ("yield", "us") => {
                urls.push("https://home.treasury.gov/resource-center/data-chart-center/interest-rates/TextView?type=daily_treasury_yield_curve&field_tdr_date_value=2025".into());
            }
            // --- Unemployment ---
            ("unemployment", "us") => {
                urls.push("https://www.bls.gov/news.release/empsit.nr0.htm".into());
            }
            _ => {}
        }
        Some(urls).filter(|v| !v.is_empty())
    }

    /// Map (subject, geography) to a World Bank API country code + indicator ID.
    fn lookup_worldbank(&self, slots: &SlotBindings) -> Option<(&'static str, &'static str)> {
        let subject = slots.subject.as_deref()?;
        let indicator = match subject {
            // Inflation — consumer prices, annual %
            s if s.contains("cpi") || s.contains("inflation") => "FP.CPI.TOTL.ZG",
            // GDP growth — annual %
            s if s.contains("gdp") && !s.contains("capita") && !s.contains("per") => {
                "NY.GDP.MKTP.KD.ZG"
            }
            // Unemployment — % of total labour force
            s if s.contains("unemployment") => "SL.UEM.TOTL.ZS",
            // Real interest rate
            s if s.contains("interest") || s == "policy rate" || s == "rate" || s.contains("yield") => {
                "FR.INR.RINR"
            }
            // Government debt — % of GDP
            s if s.contains("debt") => "GC.DOD.TOTL.GD.ZS",
            // GDP per capita
            s if s.contains("gdp") && (s.contains("capita") || s.contains("per")) => {
                "NY.GDP.PCAP.CD"
            }
            _ => return None,
        };
        let country = self.worldbank_country(slots.geography.as_deref()?)?;
        Some((country, indicator))
    }

    /// Map Finny geography strings to World Bank 2-letter country codes.
    fn worldbank_country(&self, geo: &str) -> Option<&'static str> {
        match geo {
            "us" | "fed" => Some("US"),
            "uk" | "boe" => Some("GB"),
            "eu" | "eurozone" | "ecb" => Some("EMU"),
            "japan" | "boj" => Some("JP"),
            "china" => Some("CN"),
            "turkey" => Some("TR"),
            "brazil" => Some("BR"),
            "india" => Some("IN"),
            "russia" => Some("RU"),
            "germany" => Some("DE"),
            "france" => Some("FR"),
            "italy" => Some("IT"),
            "canada" => Some("CA"),
            "australia" => Some("AU"),
            _ => None,
        }
    }

    /// Map (subject, geography) to a BLS Public API series ID.
    /// BLS covers US data only.
    fn lookup_bls(&self, slots: &SlotBindings) -> Option<&'static str> {
        let subject = slots.subject.as_deref()?;
        let geo = slots.geography.as_deref().unwrap_or("");
        let is_fed = matches!(geo, "fed") || subject.contains("fed");
        // BLS is US-only — only proceed for US geography.
        if !matches!(geo, "us" | "fed" | "usa" | "") {
            return None;
        }
        match subject {
            // CPI-U: all items, city average
            s if s.contains("cpi") || s.contains("inflation") => Some("CUUR0000SA0"),
            // Fed Funds effective rate (subject "policy rate"/"rate" + fed geo)
            s if is_fed && (s.contains("rate") || s.contains("fund")) => Some("FEDFUNDS"),
            // Civilian unemployment rate
            s if s.contains("unemployment") => Some("UNRATE"),
            // GDP (BLS does not have GDP, fall through)
            _ => None,
        }
    }
}
