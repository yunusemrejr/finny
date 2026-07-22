// intent.rs — Intent classifier + slot bindings for Finny.
//
// Pipeline: raw user input -> classify() -> SlotBindings { intent, subject,
// metric, geography, date_window }.
//
// Design notes:
//   * All keyword detection goes through canonical alias tables so that
//     "fed funds rate", "interest rate", "benchmark rate" etc. all resolve
//     to the canonical subject "policy rate" that the planner expects.
//   * Intent priority ordering: more specific patterns first (comparison,
//     calculation, causal) before generic fallbacks (direct lookup).
//   * The "what is X" pattern matches WITHOUT requiring the article "a"
//     (the old "what is a " pattern missed "what is cpi", "what is duration").

// ---------------------------------------------------------------------------
// Public types (unchanged layout — engine.rs and planner.rs depend on these)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum IntentClass {
    DirectLookup,
    Comparison(Vec<String>),
    Calculation,
    CausalScenario,
    Definition,
    AssetPrice,
    Trend,
    Forecast,
    UnsupportedRequest,
}

#[derive(Clone, Debug)]
pub struct SlotBindings {
    pub intent: IntentClass,
    pub subject: Option<String>,
    pub metric: Option<String>,
    pub geography: Option<String>,
    /// Temporal window for the query.  Populated by the classifier; the
    /// planner may use it to scope trend/forecast lookups.
    #[allow(dead_code)]
    pub date_window: Option<String>,
}

#[derive(Clone)]
pub struct IntentClassifier;

impl IntentClassifier {
    pub fn new() -> Self {
        Self
    }

    pub fn classify(&self, input: &str) -> SlotBindings {
        let lower = input.to_lowercase();

        // --- Slot detection (alias-aware) ---
        let subject = detect_canonical(&lower, SUBJECT_ALIASES);
        let metric = detect_canonical(&lower, METRIC_ALIASES);
        let geography = detect_canonical(&lower, GEO_ALIASES);
        let date_window = detect_date_window(&lower);

        // --- Subject promotion heuristic ---
        // If the user asks about "rate(s)" or "yield" in a region but no
        // specific subject was matched, promote to the canonical subject the
        // planner understands (e.g. "rate in the US" -> "policy rate").
        let subject = apply_subject_promotion(subject, &lower, &geography);

        // --- Intent classification (priority order: specific -> general) ---
        let intent = classify_intent(&lower, &subject, &geography);

        SlotBindings {
            intent,
            subject,
            metric,
            geography,
            date_window,
        }
    }
}

impl IntentClass {
    pub fn intent_class(&self) -> &'static str {
        match self {
            IntentClass::DirectLookup => "direct_lookup",
            IntentClass::Comparison(_) => "comparison",
            IntentClass::Calculation => "calculation",
            IntentClass::CausalScenario => "causal_scenario",
            IntentClass::Definition => "definition",
            IntentClass::AssetPrice => "asset_price",
            IntentClass::Trend => "trend",
            IntentClass::Forecast => "forecast",
            IntentClass::UnsupportedRequest => "unsupported_request",
        }
    }
}

impl Default for IntentClassifier {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Canonical alias tables
//
// Layout: (canonical_form, [aliases ... ]).  Tables are ordered from most
// specific to least specific so that longer phrases match before shorter
// ones.  Detection returns the *canonical* form so the planner always sees
// a value it recognises.
// ---------------------------------------------------------------------------

const SUBJECT_ALIASES: &[(&str, &[&str])] = &[
    // --- Rates (check before generic "rate") ---
    (
        "policy rate",
        &[
            "policy rate",
            "interest rate",
            "fed funds rate",
            "benchmark rate",
            "key rate",
            "main rate",
            "refinancing rate",
            "discount rate",
            "overnight rate",
            "bank rate",
        ],
    ),
    // --- Inflation ---
    ("cpi", &["cpi", "consumer price index", "hicp"]),
    ("ppi", &["ppi", "producer price index"]),
    ("inflation", &["inflation", "price growth", "cost of living"]),
    // --- Growth / output ---
    ("gdp", &["gdp", "gross domestic product", "economic output", "economic growth"]),
    (
        "unemployment",
        &["unemployment", "jobless rate", "joblessness", "unemployment rate", "jobless"],
    ),
    // --- Yields ---
    ("yield", &["bond yield", "treasury yield", "10y yield", "10-year yield", "10y", "yield"]),
    // --- Bonds (duration is bond-specific in finance) ---
    ("bond", &["bond", "bonds", "treasury", "treasuries", "sovereign", "duration"]),
    // --- Equities (before commodities so "S&P 500" wins over "gold"
    // in "compare S&P 500 and gold"). ---
    (
        "equity",
        &[
            "stock market",
            "s&p 500",
            "s&p 500 index",
            "s&p",
            "nasdaq",
            "dow jones",
            "dow",
            "ftse",
            "dax",
            "equity",
            "equities",
            "stocks",
            "stock",
            "shares",
            "index",
        ],
    ),
    // --- Commodities ---
    ("gold", &["gold", "xau", "xauusd"]),
    ("oil", &["crude oil", "crude", "wti", "brent", "oil"]),
    ("silver", &["silver", "xag", "xagusd"]),
    ("bitcoin", &["bitcoin", "btc", "btcusd"]),
    ("gas", &["natural gas", "henry hub", "gas"]),
    // --- Currencies ---
    ("fx", &["foreign exchange", "forex", "fx", "exchange rate", "currency"]),
    ("dollar", &["us dollar", "greenback", "dollar", "usd"]),
    ("euro", &["euro", "eur"]),
    ("yen", &["yen", "jpy"]),
    ("pound", &["pound", "sterling", "gbp"]),
    ("yuan", &["yuan", "renminbi", "cny"]),
    ("franc", &["swiss franc", "franc", "chf"]),
    // --- Corporate ---
    ("revenue", &["revenue", "sales", "turnover", "top line"]),
    ("earnings", &["net income", "earnings per share", "bottom line", "earnings", "profit", "eps"]),
    ("debt", &["debt", "liabilities", "borrowing"]),
    ("dividend", &["dividend", "dividends", "payout", "distribution"]),
];

const GEO_ALIASES: &[(&str, &[&str])] = &[
    // --- Multi-word first to avoid partial matches ---
    // NOTE: longer / more-specific entries MUST come before short codes
    // like "us", "eu", "uk" so that "deutschland" does not match "eu".
    ("us", &[
        "united states of america",
        "united states",
        "u.s.",
        "u.s.a.",
        "us",
        "usa",
        "america",
        "american",
        "stateside",
    ]),
    ("uk", &[
        "united kingdom",
        "great britain",
        "britain",
        "british",
        "uk",
        "england",
    ]),
    ("japan", &["japan", "japanese"]),
    ("china", &["people's republic of china", "mainland china", "china", "chinese"]),
    ("russia", &["russian federation", "russia", "russian"]),
    ("brazil", &["brazil", "brazilian"]),
    ("india", &["india", "indian"]),
    ("turkey", &["republic of turkey", "türkiye", "turkiye", "turkey", "turkish"]),
    ("germany", &["germany", "german", "deutschland"]),
    ("france", &["france", "french"]),
    ("canada", &["canada", "canadian"]),
    ("australia", &["australia", "australian"]),
    ("korea", &["south korea", "south korean", "korea"]),
    ("italy", &["italy", "italian"]),
    ("spain", &["spain", "spanish"]),
    ("mexico", &["mexico", "mexican"]),
    // --- "eu" MUST come AFTER country names so that "deutschland"
    // (which contains "eu") matches "germany" first.
    ("eu", &["european union", "euro area", "eurozone", "eu"]),
    // --- Central banks (for rate lookups) ---
    ("fed", &["federal reserve", "the fed", "fomc", "fed"]),
    ("ecb", &["european central bank", "ecb"]),
    ("boe", &["bank of england", "boe"]),
    ("boj", &["bank of japan", "boj"]),
];

const METRIC_ALIASES: &[(&str, &[&str])] = &[
    ("rate", &["rate", "rates"]),
    ("yield", &["yield", "yields"]),
    ("inflation", &["inflation", "cpi", "hicp", "price index"]),
    ("cpi", &["cpi", "consumer price index", "hicp"]),
    ("ppi", &["ppi", "producer price index"]),
    ("unemployment", &["unemployment", "jobless", "joblessness"]),
    ("gdp", &["gdp", "growth", "economic growth", "output"]),
    ("deficit", &["deficit", "budget deficit", "fiscal deficit"]),
    ("debt", &["debt", "national debt", "public debt", "government debt"]),
    ("revenue", &["revenue", "sales", "turnover"]),
    ("earnings", &["earnings", "profit", "income", "eps"]),
];

// --- Assets that are valid comparison targets (used by extract_comparison_targets) ---
const COMPARABLE_ASSETS: &[&str] = &[
    "gold", "oil", "silver", "bitcoin", "gas", "stocks", "dollar", "euro", "yen", "pound",
];

// ---------------------------------------------------------------------------
// Detection helpers
// ---------------------------------------------------------------------------

/// Scan *input* against a `(canonical, aliases)` table. Returns the canonical
/// form of the first entry whose alias is a substring of *input*.
fn detect_canonical(input: &str, table: &[(&str, &[&str])]) -> Option<String> {
    for (canonical, aliases) in table {
        for alias in *aliases {
            if input.contains(alias) {
                return Some(canonical.to_string());
            }
        }
    }
    None
}

/// If the user clearly asks about a rate/yield/bond but no specific subject
/// was detected, promote to the canonical form the planner understands.
fn apply_subject_promotion(
    subject: Option<String>,
    lower: &str,
    geography: &Option<String>,
) -> Option<String> {
    if subject.is_some() {
        return subject;
    }
    let has_geo = geography.is_some();
    let mentions_rate = lower.contains("rate") || lower.contains("rates") || lower.contains("policy");
    let mentions_yield = lower.contains("yield") || lower.contains("yields");
    let mentions_bond = lower.contains("bond") || lower.contains("treasury");

    if mentions_rate && has_geo {
        return Some("policy rate".to_string());
    }
    if mentions_yield {
        return Some("yield".to_string());
    }
    if mentions_bond {
        return Some("bond".to_string());
    }
    None
}

/// Detect temporal window expressions beyond the simple keyword matches the
/// original code handled.
fn detect_date_window(input: &str) -> Option<String> {
    // Simple keywords (preserved from original)
    if input.contains("latest") || input.contains("most recent") {
        return Some("latest".to_string());
    }
    if input.contains("current") {
        return Some("current".to_string());
    }
    if input.contains("this year") {
        return Some("current_year".to_string());
    }
    if input.contains("last year") || input.contains("past year") || input.contains("previous year") {
        return Some("previous_year".to_string());
    }

    // Relative windows: "last N years", "past N years", "last N months"
    if let Some(cap) = regex_or(input, r"(?:last|past)\s+(\d+)\s*years?") {
        return Some(format!("last_{}_years", cap as i64));
    }
    if let Some(cap) = regex_or(input, r"(?:last|past)\s+(\d+)\s*months?") {
        return Some(format!("last_{}_months", cap as i64));
    }

    // "past decade" / "last decade"
    if input.contains("past decade") || input.contains("last decade") {
        return Some("last_decade".to_string());
    }

    // "since YYYY"
    if let Some(cap) = regex_or(input, r"since\s+(\d{4})") {
        let y = cap as i64;
        if (1900..=2100).contains(&y) {
            return Some(format!("since_{}", y));
        }
    }

    // "from YYYY to YYYY" / "between YYYY and YYYY"
    if let Some(m) = regex_captures(input, r"(?:from|between)\s+(\d{4})\s+(?:to|and)\s+(\d{4})") {
        return Some(format!("range_{}_{}", m.0 as i64, m.1 as i64));
    }

    // "YYYY-YYYY" range
    if let Some(m) = regex_captures(input, r"(\d{4})\s*[-–]\s*(\d{4})") {
        return Some(format!("range_{}_{}", m.0 as i64, m.1 as i64));
    }

    // "over the last N years"
    if let Some(cap) = regex_or(input, r"over\s+(?:the\s+)?(?:last|past)\s+(\d+)\s*years?") {
        return Some(format!("last_{}_years", cap as i64));
    }

    None
}

// ---------------------------------------------------------------------------
// Intent classification
// ---------------------------------------------------------------------------

fn classify_intent(lower: &str, subject: &Option<String>, geography: &Option<String>) -> IntentClass {
    // 1. Comparison
    if lower.contains("compare")
        || lower.contains(" vs ")
        || lower.contains("versus")
        || lower.contains(" versus ")
    {
        return IntentClass::Comparison(extract_comparison_targets(lower));
    }

    // 2. Definition (high priority: "what is", "define", "what's" are
    //    unambiguous signals that must beat calculation keywords like
    //    "sharpe ratio" when the user asks "what's the sharpe ratio").
    let definition_signal = lower.contains("what is")
        || lower.contains("what's")
        || lower.contains("define")
        || lower.contains("definition of")
        || lower.contains("meaning of")
        || lower.contains("what does")
            && !lower.contains("calculate")
            && !lower.contains("compute");
    // Only treat bare "explain" as a definition request when it is NOT
    // followed by causal keywords (those are caught later).
    let explain_as_definition = lower.contains("explain")
        && !lower.contains("effect")
        && !lower.contains("impact")
        && !lower.contains("affect")
        && !lower.contains("influence")
        && !lower.contains("consequence")
        && !lower.contains("likely");
    if definition_signal || explain_as_definition || lower.contains("tell me about") {
        return IntentClass::Definition;
    }

    // 3. Calculation
    if lower.contains("calculate")
        || lower.contains("mark-to-market")
        || lower.contains("move from")
        || lower.contains("usdjpy")
        || lower.contains("eurusd")
        || lower.contains("convert")
        || lower.contains("amortiz")
        || lower.contains("duration")
        && (lower.contains("bond") || lower.contains("d="))
        || lower.contains("cagr")
        || lower.contains("mortgage")
        || lower.contains("present value")
        || lower.contains("inflation adjust")
        || lower.contains("p/e")
        || lower.contains("price-earnings")
        || lower.contains("debt-to-equity")
        || lower.contains("debt/equity")
        || lower.contains("return on equity")
        || lower.contains("roe")
        || lower.contains("dividend yield")
        || lower.contains("black-scholes")
        || lower.contains("option price")
        || lower.contains("call option")
        || lower.contains("put option")
        || lower.contains("sharpe ratio")
        || lower.contains("portfolio variance")
        || lower.contains("portfolio risk")
        || lower.contains("break-even")
        || lower.contains("break even")
        || lower.contains("wacc")
        || lower.contains("compound interest")
    {
        return IntentClass::Calculation;
    }

    // 3. Causal scenario
    if lower.contains("how could")
        || lower.contains("how would")
        || lower.contains("what if")
        || lower.contains("scenario")
        || lower.contains("likely effect")
        || lower.contains("likely impact")
        || lower.contains("explain likely")
    {
        return IntentClass::CausalScenario;
    }
    if lower.contains("explain")
        && (lower.contains("effect")
            || lower.contains("impact")
            || lower.contains("affect")
            || lower.contains("influence")
            || lower.contains("consequence"))
    {
        return IntentClass::CausalScenario;
    }

    // 4. Forecast
    if lower.contains("forecast")
        || lower.contains("project")
        || lower.contains("predict")
        || lower.contains("will be")
        || lower.contains("next year")
        || lower.contains("next quarter")
        || lower.contains("next month")
        || lower.contains("outlook")
        || lower.contains("going forward")
        || lower.contains("expect")
            && (lower.contains("in 20") || lower.contains("by 20") || lower.contains("next"))
    {
        return IntentClass::Forecast;
    }

    // 5. Trend (before AssetPrice so "gold price trend" -> Trend)
    if lower.contains("trend")
        || lower.contains("history")
        || lower.contains("historical")
        || lower.contains("over time")
        || lower.contains("over the past")
        || lower.contains("over the last")
        || lower.contains("chart")
            && (lower.contains("history") || lower.contains("past") || lower.contains("over"))
    {
        return IntentClass::Trend;
    }

    // 6. Asset price
    let asset_keywords = [
        "gold", "oil", "bitcoin", "btc", "silver", "gas", "platinum", "palladium", "copper",
    ];
    let price_keywords = ["price", "chart", "worth", "cost", "trading", "trades at", "valued at"];
    let has_asset = asset_keywords.iter().any(|k| lower.contains(k));
    let has_price = price_keywords.iter().any(|k| lower.contains(k));
    if has_asset && has_price {
        return IntentClass::AssetPrice;
    }

    // 7. Data-availability challenge (refuse to hallucinate)
    if lower.contains("exact volume")
        || lower.contains("stop-loss")
        || lower.contains("hidden order")
        || lower.contains("margin book")
        || lower.contains("order book")
        || lower.contains("dark pool")
        || lower.contains("private")
            && (lower.contains("position") || lower.contains("holdings") || lower.contains("portfolio"))
    {
        return IntentClass::UnsupportedRequest;
    }

    // 8. Fallback: direct lookup
    //    If we have a subject + geography, this is a targeted lookup.
    if subject.is_some() || geography.is_some() {
        return IntentClass::DirectLookup;
    }

    // Truly ambiguous — still a lookup, the planner will say "not in cache".
    IntentClass::DirectLookup
}

// ---------------------------------------------------------------------------
// Comparison target extraction
// ---------------------------------------------------------------------------

/// Extract the list of things being compared.  Prefers geography-based
/// comparison ("compare US and EU"); falls back to asset-based comparison
/// ("compare gold and oil") when no geographies are present.
fn extract_comparison_targets(input: &str) -> Vec<String> {
    let mut targets: Vec<String> = Vec::new();

    // Geography pass
    for (canonical, aliases) in GEO_ALIASES {
        for alias in *aliases {
            if input.contains(alias) {
                let c = canonical.to_string();
                if !targets.contains(&c) {
                    targets.push(c);
                }
                break; // only count each canonical once
            }
        }
    }

    if !targets.is_empty() {
        return targets;
    }

    // Asset fallback pass
    for asset in COMPARABLE_ASSETS {
        if input.contains(asset) && !targets.contains(&asset.to_string()) {
            targets.push(asset.to_string());
        }
    }

    targets
}

// ---------------------------------------------------------------------------
// Regex helpers
// ---------------------------------------------------------------------------

/// Match a regex with a single capture group, returning the f64 value or None.
fn regex_or(text: &str, pat: &str) -> Option<f64> {
    regex::Regex::new(pat).ok()?.captures(text)?.get(1)?.as_str().parse::<f64>().ok()
}

/// Match a regex with two capture groups, returning both as f64.
fn regex_captures(text: &str, pat: &str) -> Option<(f64, f64)> {
    let caps = regex::Regex::new(pat).ok()?.captures(text)?;
    let a = caps.get(1)?.as_str().parse::<f64>().ok()?;
    let b = caps.get(2)?.as_str().parse::<f64>().ok()?;
    Some((a, b))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- "what is X" bug fix ----

    #[test]
    fn what_is_duration_is_definition() {
        let c = IntentClassifier::new();
        let s = c.classify("what is duration");
        assert!(matches!(s.intent, IntentClass::Definition), "got {:?}", s.intent);
        assert_eq!(s.subject, Some("bond".to_string()));
    }

    #[test]
    fn what_is_cpi_is_definition() {
        let c = IntentClassifier::new();
        let s = c.classify("what is cpi");
        assert!(matches!(s.intent, IntentClass::Definition), "got {:?}", s.intent);
    }

    #[test]
    fn what_is_inflation_is_definition() {
        let c = IntentClassifier::new();
        let s = c.classify("what is inflation");
        assert!(matches!(s.intent, IntentClass::Definition), "got {:?}", s.intent);
        assert_eq!(s.subject, Some("inflation".to_string()));
    }

    #[test]
    fn define_duration_is_definition() {
        let c = IntentClassifier::new();
        let s = c.classify("define duration");
        assert!(matches!(s.intent, IntentClass::Definition), "got {:?}", s.intent);
    }

    #[test]
    fn whats_sharpe_ratio() {
        let c = IntentClassifier::new();
        let s = c.classify("what's the Sharpe ratio");
        assert!(matches!(s.intent, IntentClass::Definition), "got {:?}", s.intent);
    }

    // ---- Comparison ----

    #[test]
    fn compare_us_and_eu() {
        let c = IntentClassifier::new();
        let s = c.classify("compare policy rate across US and EU");
        match &s.intent {
            IntentClass::Comparison(parts) => {
                assert!(parts.contains(&"us".to_string()), "parts {:?}", parts);
                assert!(parts.contains(&"eu".to_string()), "parts {:?}", parts);
            }
            other => panic!("expected Comparison, got {:?}", other),
        }
    }

    #[test]
    fn compare_gold_and_oil() {
        let c = IntentClassifier::new();
        let s = c.classify("compare gold and oil");
        match &s.intent {
            IntentClass::Comparison(parts) => {
                assert!(parts.contains(&"gold".to_string()), "parts {:?}", parts);
                assert!(parts.contains(&"oil".to_string()), "parts {:?}", parts);
            }
            other => panic!("expected Comparison, got {:?}", other),
        }
    }

    #[test]
    fn compare_three_geographies() {
        let c = IntentClassifier::new();
        let s = c.classify("compare inflation across US, UK, and Japan");
        match &s.intent {
            IntentClass::Comparison(parts) => {
                assert!(parts.contains(&"us".to_string()), "parts {:?}", parts);
                assert!(parts.contains(&"uk".to_string()), "parts {:?}", parts);
                assert!(parts.contains(&"japan".to_string()), "parts {:?}", parts);
            }
            other => panic!("expected Comparison, got {:?}", other),
        }
    }

    // ---- Subject alias resolution ----

    #[test]
    fn fed_funds_rate_resolves_to_policy_rate() {
        let c = IntentClassifier::new();
        let s = c.classify("latest fed funds rate for the US");
        assert_eq!(s.subject, Some("policy rate".to_string()), "subject {:?}", s.subject);
        assert_eq!(s.geography, Some("us".to_string()), "geo {:?}", s.geography);
    }

    #[test]
    fn interest_rate_resolves_to_policy_rate() {
        let c = IntentClassifier::new();
        let s = c.classify("what's the interest rate in the UK?");
        assert_eq!(s.subject, Some("policy rate".to_string()), "subject {:?}", s.subject);
    }

    #[test]
    fn nasdaq_resolves_to_equity() {
        let c = IntentClassifier::new();
        let s = c.classify("show me nasdaq price");
        assert_eq!(s.subject, Some("equity".to_string()), "subject {:?}", s.subject);
    }

    #[test]
    fn s_and_p_500_resolves_to_equity() {
        let c = IntentClassifier::new();
        let s = c.classify("compare S&P 500 and gold");
        assert_eq!(s.subject, Some("equity".to_string()), "subject {:?}", s.subject);
    }

    #[test]
    fn consumer_price_index_resolves_to_cpi() {
        let c = IntentClassifier::new();
        let s = c.classify("latest Consumer Price Index for the US");
        assert_eq!(s.subject, Some("cpi".to_string()), "subject {:?}", s.subject);
    }

    // ---- Geography alias resolution ----

    #[test]
    fn america_resolves_to_us() {
        let c = IntentClassifier::new();
        let s = c.classify("CPI in America");
        assert_eq!(s.geography, Some("us".to_string()), "geo {:?}", s.geography);
    }

    #[test]
    fn britain_resolves_to_uk() {
        let c = IntentClassifier::new();
        let s = c.classify("policy rate in Britain");
        assert_eq!(s.geography, Some("uk".to_string()), "geo {:?}", s.geography);
    }

    #[test]
    fn deutschland_resolves_to_germany() {
        let c = IntentClassifier::new();
        // Germany is detected as a geography
        let s = c.classify("inflation in Deutschland");
        assert_eq!(s.geography, Some("germany".to_string()), "geo {:?}", s.geography);
    }

    // ---- Date window detection ----

    #[test]
    fn last_5_years() {
        let c = IntentClassifier::new();
        let s = c.classify("US CPI over the last 5 years");
        assert_eq!(s.date_window, Some("last_5_years".to_string()), "dw {:?}", s.date_window);
    }

    #[test]
    fn past_decade() {
        let c = IntentClassifier::new();
        let s = c.classify("gold price over the past decade");
        assert_eq!(s.date_window, Some("last_decade".to_string()), "dw {:?}", s.date_window);
    }

    #[test]
    fn since_2020() {
        let c = IntentClassifier::new();
        let s = c.classify("US inflation since 2020");
        assert_eq!(s.date_window, Some("since_2020".to_string()), "dw {:?}", s.date_window);
    }

    #[test]
    fn range_2020_to_2024() {
        let c = IntentClassifier::new();
        let s = c.classify("oil price from 2020 to 2024");
        assert_eq!(s.date_window, Some("range_2020_2024".to_string()), "dw {:?}", s.date_window);
    }

    #[test]
    fn last_year() {
        let c = IntentClassifier::new();
        let s = c.classify("US CPI last year");
        assert_eq!(s.date_window, Some("previous_year".to_string()), "dw {:?}", s.date_window);
    }

    // ---- Subject promotion heuristic ----

    #[test]
    fn rate_in_us_promotes_to_policy_rate() {
        let c = IntentClassifier::new();
        let s = c.classify("what's the rate in the US?");
        assert_eq!(s.subject, Some("policy rate".to_string()), "subject {:?}", s.subject);
    }

    #[test]
    fn yield_without_geo_promotes_to_yield() {
        let c = IntentClassifier::new();
        let s = c.classify("show me the yield");
        assert_eq!(s.subject, Some("yield".to_string()), "subject {:?}", s.subject);
    }

    // ---- Intent edge cases ----

    #[test]
    fn latest_cpi_us_is_lookup() {
        let c = IntentClassifier::new();
        let s = c.classify("latest US CPI");
        assert!(matches!(s.intent, IntentClass::DirectLookup), "got {:?}", s.intent);
        assert_eq!(s.subject, Some("cpi".to_string()));
        assert_eq!(s.geography, Some("us".to_string()));
    }

    #[test]
    fn gold_price_trend_is_trend_not_assetprice() {
        let c = IntentClassifier::new();
        let s = c.classify("gold price trend");
        assert!(matches!(s.intent, IntentClass::Trend), "got {:?}", s.intent);
    }

    #[test]
    fn stop_loss_is_unsupported() {
        let c = IntentClassifier::new();
        let s = c.classify("exact volume of stop-loss orders at 152.50");
        assert!(matches!(s.intent, IntentClass::UnsupportedRequest), "got {:?}", s.intent);
    }

    #[test]
    fn forecast_is_forecast() {
        let c = IntentClassifier::new();
        let s = c.classify("forecast US CPI next year");
        assert!(matches!(s.intent, IntentClass::Forecast), "got {:?}", s.intent);
    }

    #[test]
    fn causal_scenario_with_explain() {
        let c = IntentClassifier::new();
        let s = c.classify("explain the likely effects of a rate hike on bonds");
        assert!(matches!(s.intent, IntentClass::CausalScenario), "got {:?}", s.intent);
    }

    #[test]
    fn how_could_is_causal() {
        let c = IntentClassifier::new();
        let s = c.classify("how could yen appreciation affect carry trades?");
        assert!(matches!(s.intent, IntentClass::CausalScenario), "got {:?}", s.intent);
    }

    #[test]
    fn calculation_keywords() {
        let c = IntentClassifier::new();
        for q in &[
            "calculate bond duration D=7 and 25bp",
            "convert 100 USD to EUR",
            "CAGR from 1000 to 2000 over 5 years",
            "mortgage payment for 300k at 6.5%",
        ] {
            let s = c.classify(q);
            assert!(
                matches!(s.intent, IntentClass::Calculation),
                "query {:?} -> {:?}",
                q,
                s.intent
            );
        }
    }

    #[test]
    fn empty_input_is_lookup() {
        let c = IntentClassifier::new();
        let s = c.classify("");
        assert!(matches!(s.intent, IntentClass::DirectLookup), "got {:?}", s.intent);
    }
}
