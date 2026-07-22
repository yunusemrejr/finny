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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    pub nlu: crate::nlu::Nlu,
    pub planner: QueryPlanner,
    pub use_network: Arc<AtomicBool>,
    /// Cached live FX rates (TTL 1h) so repeated conversions don't re-hit the net.
    fx_cache: Arc<Mutex<Option<(Instant, crate::retrieval::FxRates)>>>,
}

impl Engine {
    pub fn new(network_enabled: bool) -> Result<Self> {
        let db = Db::open_default()?;
        db.init()?;
        Ok(Self {
            db,
            retriever: Arc::new(Retriever::new(12)?),
            classifier: IntentClassifier::new(),
            nlu: crate::nlu::Nlu::new(),
            planner: QueryPlanner::new(),
            use_network: Arc::new(AtomicBool::new(network_enabled)),
            fx_cache: Arc::new(Mutex::new(None)),
        })
    }

    /// Full chat pipeline — preserved verbatim from app.rs::handle_chat, minus
    /// the serde_json envelope. Runs on a worker thread (may block on network).
    pub fn handle_chat(&self, text: &str, session_id: Option<i64>) -> ChatReply {
        let slots = self.nlu.resolve(text);
        let mut sources: Vec<String> = Vec::new();

        // Chitchat is answered conversationally by the dialogue layer — no
        // planner and no network round-trips. Finance queries get the
        // deterministic answer (optionally augmented with live data below),
        // then wrapped into a conversational reply.
        let chitchat_kind = match &slots.intent {
            crate::intent::IntentClass::Chitchat(kind) => Some(*kind),
            _ => None,
        };
        let mut answer = match chitchat_kind {
            Some(kind) => crate::dialogue::chitchat_reply(kind, text),
            None => {
                // Currency conversions get live rates (cached ≤1h) when the
                // network is on — every other query stays fully deterministic.
                let live_fx = if matches!(slots.intent, crate::intent::IntentClass::Calculation)
                    && crate::planner::is_conversion_query(&text.to_lowercase())
                    && self.use_network.load(Ordering::Relaxed)
                {
                    self.fresh_fx().map(|fx| {
                        self.persist_source(&fx.source_url, &fx.sha256, fx.retrieved_at);
                        crate::planner::FxTable {
                            rates: fx.per_usd.clone(),
                            source: format!(
                                "frankfurter.app (ECB reference, fetched {} UTC)",
                                fx.retrieved_at.format("%Y-%m-%d %H:%M")
                            ),
                        }
                    })
                } else {
                    None
                };
                self.planner.plan_and_answer_with_fx(&slots, text, live_fx.as_ref())
            }
        };

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

            // JSON APIs are tried FIRST: they are small, fast, structured and
            // bounded (4s client). The slow HTML scrape runs LAST and only if
            // no JSON source answered — this stops the UI worker from "wandering"
            // through flaky pages when a clean API answer is available.

            // Phase 1: World Bank API — broad country × indicator coverage.
            if let Some((country, indicator)) = self.lookup_worldbank(&slots) {
                if let Ok(data) = self.retriever.fetch_worldbank(country, indicator) {
                    self.persist_source(&data.source_url, &data.sha256, data.retrieved_at);
                    answer.push_str(&format!("\n\n## Live Data (World Bank API)\n{}", data.text));
                    sources.push(data.source_url);
                    fetched = true;
                }
            }

            // Phase 2: BLS Public API — detailed US economic series.
            if !fetched {
                if let Some(series) = self.lookup_bls(&slots) {
                    if let Ok(data) = self.retriever.fetch_bls(series) {
                        self.persist_source(&data.source_url, &data.sha256, data.retrieved_at);
                        answer.push_str(&format!("\n\n## Live Data (BLS Public API)\n{}", data.text));
                        sources.push(data.source_url);
                        fetched = true;
                    }
                }
            }

            // Phase 3 (fallback): scrape known central-bank / stats-agency pages.
            if !fetched {
                if let Some(cands) = self.lookup_candidate_urls(&slots) {
                    for url in cands {
                        if let Ok(page) = self.retriever.fetch(&url) {
                            let body = crate::retrieval::html_to_text(&page.body);
                            let excerpts = crate::retrieval::excerpts_around_keywords(
                                &body, &keywords, 3, 150,
                            );
                            if !excerpts.is_empty() {
                                self.persist_source(&page.url, &page.content_sha256, page.retrieved_at);
                                answer.push_str(&format!(
                                    "\n\n## Live Excerpts ({})\n{}",
                                    page.url,
                                    excerpts.iter().map(|e| format!("> {}", e)).collect::<Vec<_>>().join("\n\n")
                                ));
                                sources.push(page.url);
                                break;
                            }
                        }
                    }
                }
            }
        }

        // Make the deterministic finance answer conversational. Chitchat is
        // already conversational and skips this wrapping.
        if chitchat_kind.is_none() {
            answer = crate::dialogue::wrap(&slots, &answer, text);
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

    /// Wipe all chats + cached sources + sessions (the in-app "Clear data").
    /// Leaves a single fresh session so the UI always has somewhere to write.
    pub fn clear_all_data(&self) -> Result<i64> {
        self.db.clear_all_data()?;
        let s = self.db.create_session("Session 1")?;
        Ok(s.id)
    }

    /// Direct engine call for the currency converter / chart panels (no network,
    /// no persistence) — reuses the planner exactly as chat does.
    pub fn quick_answer(&self, text: &str) -> String {
        let slots = self.classifier.classify(text);
        self.planner.plan_and_answer(&slots, text)
    }

    /// Currency conversion: live rate (frankfurter.app / ECB, cached 1h) when
    /// the network is on, falling back to the deterministic local matrix.
    /// Runs on a worker thread (the live fetch may block briefly).
    pub fn convert_live_or_local(&self, amount_raw: &str, from: &str, to: &str) -> String {
        let from = from.to_uppercase();
        let to = to.to_uppercase();
        let amount: f64 = amount_raw.trim().parse().unwrap_or(0.0);

        if amount > 0.0 && from != to && self.use_network.load(Ordering::Relaxed) {
            if let Some(fx) = self.fresh_fx() {
                let fu = fx.per_usd.get(&from).copied();
                let tu = fx.per_usd.get(&to).copied();
                if let (Some(fu), Some(tu)) = (fu, tu) {
                    let result = amount / fu * tu;
                    let rate = tu / fu;
                    self.persist_source(&fx.source_url, &fx.sha256, fx.retrieved_at);
                    return format!(
                        "# Currency Conversion\n\n**{:.2} {} ≈ {:.2} {}**\n\n\
                         - Live rate: 1 {} = {:.4} {}\n\
                         - Source: frankfurter.app (ECB reference rates)\n\
                         - Fetched: {} UTC\n\n\
                         _Rates are reference rates updated daily; provenance saved. \
                         Turn off **Live network** to use the built-in approximate matrix._",
                        amount, from, result, to, from, rate, to,
                        fx.retrieved_at.format("%Y-%m-%d %H:%M")
                    );
                }
            }
        }

        // Fallback: deterministic local matrix (no network, always available).
        self.quick_answer(&format!("convert {} {} to {}", amount_raw, from, to))
    }

    /// Return cached live FX rates if fresh (TTL 1h), else fetch and cache.
    /// Returns None if the fetch fails (caller falls back to local rates).
    fn fresh_fx(&self) -> Option<crate::retrieval::FxRates> {
        const TTL: Duration = Duration::from_secs(3600);
        let mut guard = self.fx_cache.lock().ok()?;
        if let Some((t, fx)) = guard.as_ref() {
            if t.elapsed() < TTL {
                return Some(fx.clone());
            }
        }
        let fx = self.retriever.fetch_fx_rates().ok()?;
        *guard = Some((Instant::now(), fx.clone()));
        Some(fx)
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

#[cfg(test)]
mod conversational_tests {
    //! End-to-end regression tests for the user's #1 complaint: chat used to
    //! return rigid boilerplate ("# Answer — subject ((region)) / Not in local
    //! cache") even for greetings. These run the FULL handle_chat pipeline
    //! (classify → plan → dialogue) against a throwaway DB with network OFF
    //! (fast + deterministic) and assert the replies are now conversational.
    //!
    //! One sequential test (not parallel) so the process-global FINNY_DATA_DIR
    //! and the shared temp DB are never accessed concurrently.
    use super::*;

    #[test]
    fn conversational_end_to_end() {
        // Throwaway DB dir — never touch the user's real data.
        let dir = std::env::temp_dir().join(format!("finny_conv_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("FINNY_DATA_DIR", &dir);

        let e = Engine::new(false).unwrap(); // network OFF
        let sid = e.list_sessions().unwrap().first().map(|s| s.id);

        // Send a question and read back the assistant reply that was persisted.
        let ask = |q: &str| -> String {
            e.handle_chat(q, sid);
            e.history_for(sid)
                .unwrap()
                .into_iter()
                .filter(|m| m.role == "assistant")
                .last()
                .map(|m| m.text)
                .unwrap_or_default()
        };

        // 1. Greetings → friendly, no placeholder/dead-end.
        for q in ["hi", "hello finny", "hey"] {
            let r = ask(q);
            assert!(!r.contains("((region))"), "{:?} leaked placeholder: {}", q, r);
            assert!(!r.contains("Not in local cache"), "{:?} dead-end: {}", q, r);
            assert!(r.to_lowercase().contains("finny"), "{:?} not a greeting: {}", q, r);
        }

        // 2. Capabilities / identity.
        let cap = ask("what can you do");
        assert!(cap.contains("convert") || cap.contains("CPI"), "capabilities: {}", cap);
        let id = ask("who are you");
        assert!(id.to_lowercase().contains("finny"), "identity: {}", id);

        // 3. Finance answer: substance kept, boilerplate stripped, source footer present.
        let cpi = ask("latest US CPI");
        assert!(cpi.contains("3.0%"), "missing data: {}", cpi);
        assert!(!cpi.contains("## Reliability note"), "boilerplate kept: {}", cpi);
        assert!(!cpi.contains("## Observed fact"), "boilerplate kept: {}", cpi);
        assert!(cpi.to_lowercase().contains("source"), "no source footer: {}", cpi);

        // 4. Calculator keeps its result.
        let bond = ask("calculate bond duration D=7 and 25bp");
        assert!(bond.contains("-1.75%"), "calc result missing: {}", bond);

        // 5. Typo'd query is rescued (geography recovered), not a dead-end.
        let turky = ask("inflation in turky");
        assert!(!turky.contains("((region))"), "placeholder leaked: {}", turky);
        assert!(!turky.contains("# Answer — subject"), "dead-end: {}", turky);

        // 6. The user's exact complaint must no longer dead-end.
        let complaint = ask("hi finny. can you summarize recent policy rate changes?");
        assert!(!complaint.contains("((region))"), "placeholder leaked: {}", complaint);
        assert!(!complaint.contains("Not in local cache"), "dead-end: {}", complaint);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
