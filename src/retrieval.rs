use anyhow::Result;
use reqwest::blocking;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::time::Duration;

pub struct Retriever {
    /// Slower client (12s) for HTML page scrapes, which can be large/slow.
    client: blocking::Client,
    /// Fast client (4s) for small JSON APIs — keeps live lookups snappy and
    /// bounded so the UI never waits long (calls run off-thread regardless).
    api: blocking::Client,
}

pub struct RetrievedPage {
    pub url: String,
    pub content_sha256: String,
    pub retrieved_at: chrono::DateTime<chrono::Utc>,
    pub body: String,
}

/// Formatted result from a structured API call (World Bank, BLS, etc).
pub struct LiveData {
    pub source_url: String,
    pub sha256: String,
    pub retrieved_at: chrono::DateTime<chrono::Utc>,
    pub text: String,
}

/// Live FX rate snapshot: units of each currency per 1 USD (USD = 1.0).
#[derive(Clone, Debug)]
pub struct FxRates {
    pub per_usd: std::collections::HashMap<String, f64>,
    pub source_url: String,
    pub sha256: String,
    pub retrieved_at: chrono::DateTime<chrono::Utc>,
}

impl Retriever {
    pub fn new(timeout_secs: u64) -> Result<Self> {
        let mk = |secs: u64| -> Result<blocking::Client> {
            Ok(blocking::Client::builder()
                .timeout(Duration::from_secs(secs))
                .user_agent("Finny/0.1 (local finance assistant)")
                .build()?)
        };
        Ok(Self {
            client: mk(timeout_secs)?,
            api: mk(timeout_secs.min(4))?,
        })
    }

    pub fn is_safe(url: &str) -> bool {
        if url.starts_with("file://") || url.starts_with("ftp://") {
            return false;
        }
        if url.contains("localhost")
            || url.contains("169.254")
            || url.contains("127.0.0.1")
            || url.contains("0.0.0.0")
        {
            return false;
        }
        if let Ok(parsed) = url::Url::parse(url) {
            if let Some(host) = parsed.host_str() {
                if host.contains(".local")
                    || host.contains(".internal")
                    || host.starts_with("10.")
                    || host.starts_with("192.168.")
                    || host.starts_with("172.")
                {
                    return false;
                }
                if host.parse::<std::net::IpAddr>().is_ok() {
                    return false;
                }
            }
        }
        true
    }

    /// Low-level fetch with SSRF guard — returns raw body text (HTML client).
    fn fetch_raw(&self, url: &str) -> Result<String> {
        fetch_with(&self.client, url)
    }

    /// Low-level fetch with SSRF guard using the fast API client.
    fn fetch_api(&self, url: &str) -> Result<String> {
        fetch_with(&self.api, url)
    }

    pub fn fetch(&self, url: &str) -> Result<RetrievedPage> {
        let body = self.fetch_raw(url)?;
        let mut hasher = Sha256::new();
        hasher.update(&body);
        let hash = format!("{:x}", hasher.finalize());
        Ok(RetrievedPage {
            url: url.to_string(),
            content_sha256: hash,
            retrieved_at: chrono::Utc::now(),
            body,
        })
    }

    /// Query the World Bank API for an economic indicator.
    /// Returns formatted text with recent data points.
    pub fn fetch_worldbank(&self, country_code: &str, indicator: &str) -> Result<LiveData> {
        let url = format!(
            "https://api.worldbank.org/v2/country/{}/indicator/{}?format=json&date=2020:2025&per_page=10",
            country_code, indicator
        );
        let body = self.fetch_api(&url)?;
        let text = format_worldbank_response(&body)?;
        let mut hasher = Sha256::new();
        hasher.update(&body);
        Ok(LiveData {
            source_url: url,
            sha256: format!("{:x}", hasher.finalize()),
            retrieved_at: chrono::Utc::now(),
            text,
        })
    }

    /// Query the BLS Public API for a US economic time series.
    pub fn fetch_bls(&self, series_id: &str) -> Result<LiveData> {
        let url = format!(
            "https://api.bls.gov/publicAPI/v2/timeseries/data/{}?latest=6",
            series_id
        );
        let body = self.fetch_api(&url)?;
        let text = format_bls_response(&body)?;
        let mut hasher = Sha256::new();
        hasher.update(&body);
        Ok(LiveData {
            source_url: url,
            sha256: format!("{:x}", hasher.finalize()),
            retrieved_at: chrono::Utc::now(),
            text,
        })
    }

    /// Fetch live FX rates vs USD from the keyless frankfurter.app API
    /// (sourced from the European Central Bank reference rates). No API key.
    /// Returns a map of currency code → units per 1 USD (USD = 1.0).
    pub fn fetch_fx_rates(&self) -> Result<FxRates> {
        let url = "https://api.frankfurter.app/latest?from=USD";
        let body = self.fetch_api(url)?;
        let mut hasher = Sha256::new();
        hasher.update(&body);
        let rates = parse_fx_rates(&body)?;
        Ok(FxRates {
            per_usd: rates,
            source_url: url.to_string(),
            sha256: format!("{:x}", hasher.finalize()),
            retrieved_at: chrono::Utc::now(),
        })
    }
}

/// Low-level GET with the SSRF guard, using the given client.
fn fetch_with(client: &blocking::Client, url: &str) -> Result<String> {
    if !Retriever::is_safe(url) {
        return Err(anyhow::anyhow!("URL blocked by retrieval policy: {}", url));
    }
    let resp = client
        .get(url)
        .send()
        .map_err(|e| anyhow::anyhow!(format!("request failed: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow::anyhow!("HTTP {status} for {url}"));
    }
    Ok(resp.text().unwrap_or_default())
}

/// Parse a frankfurter.app `{"base":"USD","rates":{...}}` body into a per-USD map.
fn parse_fx_rates(body: &str) -> Result<std::collections::HashMap<String, f64>> {
    let data: Value = serde_json::from_str(body)?;
    let rates_obj = data
        .get("rates")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow::anyhow!("unexpected FX response shape"))?;
    let mut map = std::collections::HashMap::new();
    map.insert("USD".to_string(), 1.0);
    for (code, val) in rates_obj {
        if let Some(n) = val.as_f64() {
            if n > 0.0 {
                map.insert(code.to_uppercase(), n);
            }
        }
    }
    if map.len() < 2 {
        return Err(anyhow::anyhow!("FX response had no usable rates"));
    }
    Ok(map)
}

/// Convert an HTML string to plain text by stripping tags.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut skipping = false;
    let mut tag_buf = String::new();
    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                tag_buf.clear();
            }
            '>' => {
                in_tag = false;
                let lower = tag_buf.to_lowercase();
                if lower == "script" || lower.starts_with("script ") {
                    skipping = true;
                }
                if lower == "style" || lower.starts_with("style ") {
                    skipping = true;
                }
                if lower == "/script" || lower == "/style" {
                    skipping = false;
                }
            }
            _ if in_tag => {
                tag_buf.push(ch);
            }
            _ if skipping => {}
            _ => out.push(ch),
        }
    }
    out
}

/// Extract snippets around ANY of the given keywords from text.
pub fn excerpts_around_keywords(
    text: &str,
    keywords: &[&str],
    max_snippets: usize,
    window: usize,
) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut out = Vec::new();
    for keyword in keywords {
        let key = keyword.to_lowercase();
        if key.is_empty() {
            continue;
        }
        let mut search_start = 0;
        while let Some(pos) = lower[search_start..].find(&key) {
            let abs_pos = search_start + pos;
            let start = abs_pos.saturating_sub(window);
            let end = (abs_pos + key.len() + window).min(text.len());
            let snippet = text[start..end].trim().to_string();
            if !snippet.is_empty() && !out.contains(&snippet) {
                out.push(snippet);
            }
            search_start = abs_pos + key.len();
            if out.len() >= max_snippets {
                break;
            }
        }
        if out.len() >= max_snippets {
            break;
        }
    }
    out
}

/// Parse a World Bank API JSON response into readable data-point lines.
fn format_worldbank_response(body: &str) -> Result<String> {
    let data: Value = serde_json::from_str(body)?;
    let observations = data
        .get(1)
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("unexpected World Bank response shape"))?;
    let indicator_name = observations
        .first()
        .and_then(|o| o.get("indicator"))
        .and_then(|i| i.get("value"))
        .and_then(|v| v.as_str())
        .unwrap_or("Indicator");
    let country_name = observations
        .first()
        .and_then(|o| o.get("country"))
        .and_then(|c| c.get("value"))
        .and_then(|v| v.as_str())
        .unwrap_or("Country");
    let mut lines = vec![format!("{} — {} (via World Bank API)", indicator_name, country_name)];
    for obs in observations.iter().take(6) {
        let year = obs.get("date").and_then(|v| v.as_str()).unwrap_or("?");
        match obs.get("value") {
            Some(Value::Null) | None => lines.push(format!("  {}: (no data)", year)),
            Some(v) => lines.push(format!("  {}: {}", year, v)),
        }
    }
    Ok(lines.join("\n"))
}

/// Parse a BLS Public API JSON response into readable data-point lines.
fn format_bls_response(body: &str) -> Result<String> {
    let data: Value = serde_json::from_str(body)?;
    let series = data
        .get("Results")
        .and_then(|r| r.get("series"))
        .and_then(|s| s.as_array())
        .and_then(|s| s.first())
        .ok_or_else(|| anyhow::anyhow!("unexpected BLS response shape"))?;
    let series_id = series
        .get("seriesID")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown");
    let mut lines = vec![format!("{} (via BLS Public API)", series_id)];
    if let Some(observations) = series.get("data").and_then(|d| d.as_array()) {
        for obs in observations.iter().take(6) {
            let year = obs.get("year").and_then(|v| v.as_str()).unwrap_or("?");
            let period = obs
                .get("periodName")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let value = obs.get("value").and_then(|v| v.as_str()).unwrap_or("—");
            lines.push(format!("  {} {}: {}", period, year, value));
        }
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_localhost() {
        assert!(!Retriever::is_safe("http://localhost:8080/x"));
        assert!(!Retriever::is_safe(
            "https://169.254.169.254/latest/meta-data/"
        ));
        assert!(!Retriever::is_safe("https://127.0.0.1/"));
    }

    #[test]
    fn blocks_private() {
        assert!(!Retriever::is_safe("https://10.0.0.1/x"));
        assert!(!Retriever::is_safe("https://192.168.1.1/x"));
        assert!(!Retriever::is_safe("https://172.16.0.1/x"));
    }

    #[test]
    fn allows_public() {
        assert!(Retriever::is_safe(
            "https://www.federalreserve.gov/releases/h15/"
        ));
        assert!(Retriever::is_safe(
            "https://www.bls.gov/news.release/cpi.nr0.htm"
        ));
    }

    #[test]
    fn extracts_text_from_html() {
        let html = "<html><body><p>CPI rose 3.0%</p></body></html>";
        let text = html_to_text(html);
        assert!(text.contains("CPI"));
        assert!(!text.contains('<'));
    }

    #[test]
    fn multi_keyword_excerpts() {
        let text = "Federal funds rate is 4.5%. The CPI inflation rate rose to 3.2%. Unemployment is low.";
        let kw = &["funds rate", "inflation"];
        let excerpts = excerpts_around_keywords(text, kw, 5, 20);
        assert!(!excerpts.is_empty());
        // Should find both keywords.
        let joined = excerpts.join(" ");
        assert!(joined.contains("funds rate") || joined.contains("Federal funds"));
        assert!(joined.contains("inflation"));
    }

    #[test]
    fn worldbank_json_format() {
        let json = r#"[{"page":1},[{"indicator":{"value":"Inflation"},"country":{"value":"United States"},"date":"2024","value":3.0},{"date":"2023","value":4.1}]]"#;
        let text = format_worldbank_response(json).unwrap();
        assert!(text.contains("World Bank API"));
        assert!(text.contains("United States"));
        assert!(text.contains("2024"));
        assert!(text.contains("3.0"));
    }

    #[test]
    fn bls_json_format() {
        let json = r#"{"Results":{"series":[{"seriesID":"CUUR0000SA0","data":[{"year":"2025","periodName":"June","value":"333.9"}]}]}}"#;
        let text = format_bls_response(json).unwrap();
        assert!(text.contains("BLS Public API"));
        assert!(text.contains("CUUR0000SA0"));
        assert!(text.contains("June"));
        assert!(text.contains("333.9"));
    }

    #[test]
    fn worldbank_blocks_bad_json() {
        let bad = r#"{"foo":"bar"}"#;
        assert!(format_worldbank_response(bad).is_err());
    }

    #[test]
    fn bls_blocks_bad_json() {
        let bad = r#"{"foo":"bar"}"#;
        assert!(format_bls_response(bad).is_err());
    }

    #[test]
    fn worldbank_api_is_safe() {
        // The constructed World Bank API URL must pass SSRF checks.
        assert!(Retriever::is_safe(
            "https://api.worldbank.org/v2/country/USA/indicator/FP.CPI.TOTL.ZG"
        ));
    }

    #[test]
    fn bls_api_is_safe() {
        assert!(Retriever::is_safe(
            "https://api.bls.gov/publicAPI/v2/timeseries/data/CUUR0000SA0"
        ));
    }

    #[test]
    fn fx_json_format() {
        let json = r#"{"amount":1.0,"base":"USD","date":"2025-01-01","rates":{"EUR":0.92,"JPY":155.0,"GBP":0.79}}"#;
        let m = parse_fx_rates(json).unwrap();
        assert_eq!(m.get("USD"), Some(&1.0));
        assert_eq!(m.get("EUR"), Some(&0.92));
        assert_eq!(m.get("JPY"), Some(&155.0));
    }

    #[test]
    fn fx_blocks_bad_json() {
        assert!(parse_fx_rates(r#"{"foo":"bar"}"#).is_err());
        assert!(parse_fx_rates(r#"{"rates":{}}"#).is_err());
    }

    #[test]
    fn fx_api_is_safe() {
        assert!(Retriever::is_safe("https://api.frankfurter.app/latest?from=USD"));
    }

    #[test]
    #[ignore]
    fn live_fx_fetch() {
        if std::env::var("FINNY_LIVE_TEST").is_err() {
            return;
        }
        let r = Retriever::new(6).unwrap();
        let fx = r.fetch_fx_rates().unwrap();
        assert!(fx.per_usd.contains_key("EUR"));
        println!("Live FX ({} rates): EUR={:?}", fx.per_usd.len(), fx.per_usd.get("EUR"));
    }

    // --- Live network integration tests (ignored by default) ---
    // Run with: FINNY_LIVE_TEST=1 cargo test --release -- --ignored

    #[test]
    #[ignore]
    fn live_worldbank_fetch() {
        if std::env::var("FINNY_LIVE_TEST").is_err() {
            return;
        }
        let r = Retriever::new(10).unwrap();
        let data = r.fetch_worldbank("US", "FP.CPI.TOTL.ZG").unwrap();
        assert!(data.text.contains("World Bank API"));
        assert!(!data.sha256.is_empty());
        println!("Live WorldBank:\n{}", data.text);
    }

    #[test]
    #[ignore]
    fn live_bls_fetch() {
        if std::env::var("FINNY_LIVE_TEST").is_err() {
            return;
        }
        let r = Retriever::new(10).unwrap();
        let data = r.fetch_bls("CUUR0000SA0").unwrap();
        assert!(data.text.contains("BLS Public API"));
        assert!(!data.sha256.is_empty());
        println!("Live BLS:\n{}", data.text);
    }

    #[test]
    #[ignore]
    fn live_fed_h15_fetch() {
        if std::env::var("FINNY_LIVE_TEST").is_err() {
            return;
        }
        let r = Retriever::new(10).unwrap();
        let page = r
            .fetch("https://www.federalreserve.gov/releases/h15/")
            .unwrap();
        let text = html_to_text(&page.body);
        assert!(!text.is_empty());
        println!("Live Fed H15 (first 200 chars): {}", &text[..200.min(text.len())]);
    }
}
