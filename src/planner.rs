use crate::intent::SlotBindings;
use std::collections::HashSet;

#[derive(Clone)]
pub struct QueryPlanner;

impl QueryPlanner {
    pub fn new() -> Self { Self }
    pub fn plan_and_answer(&self, slots: &SlotBindings, question: &str) -> String {
        use crate::intent::IntentClass;
        match &slots.intent {
            IntentClass::DirectLookup => self.direct_lookup(slots),
            IntentClass::Comparison(parts) => self.compare(slots, parts),
            IntentClass::Calculation => self.calculate(slots, question),
            IntentClass::CausalScenario => self.scenario(slots, question),
            IntentClass::Definition => self.define_term(question),
            IntentClass::AssetPrice => self.asset_price(slots, question),
            IntentClass::Trend => self.trend(slots, question),
            IntentClass::Forecast => self.forecast(slots, question),
            IntentClass::UnsupportedRequest => self.unsupported(question),
        }
    }
    fn direct_lookup(&self, s: &SlotBindings) -> String {
        let subj = s.subject.as_deref().unwrap_or("subject");
        let geo = s.geography.as_deref().unwrap_or("(region)");
        match (subj, geo) {
            ("cpi","us") => ans("CPI (US)", "Latest BLS annual YoY ≈ 3.0% (verify against release calendar).", "BLS"),
            ("policy rate","fed") => ans("Fed Funds Rate", "Target range 4.25–4.50% post latest FOMC.", "Federal Reserve"),
            ("policy rate","ecb") => ans("ECB Main Rate", "Main refinancing ≈ 3.15%.", "ECB"),
            ("policy rate","boe") => ans("BoE Bank Rate", "Bank rate ≈ 4.50%.", "BoE"),
            ("policy rate","boj") => ans("BoJ Rate", "Short-term target ≈ 0.50%.", "BoJ"),
            ("yield",_) => ans("Generic 10Y", "Approx: US 4.3%, DE 2.5%, JP 1.0%. Verify.", "Various"),
            _ => ans(&format!("{} ({})", subj, geo), "Not in local cache. Try with network enabled.", "TBD"),
        }
    }
    fn compare(&self, s: &SlotBindings, parts: &[String]) -> String {
        // Determine whether we are comparing geographies (policy rates) or
        // assets (commodities / currencies).  The two cases need different
        // data and chart scaling.
        let geo_codes: HashSet<&str> = [
            "us", "eu", "uk", "japan", "china", "turkey", "fed", "ecb", "boe", "boj",
        ]
        .iter()
        .cloned()
        .collect();

        let all_geo = parts.iter().all(|p| geo_codes.contains(p.as_str()));

        if all_geo && !parts.is_empty() {
            self.compare_geographies(s, parts)
        } else {
            self.compare_assets(parts)
        }
    }

    /// Geography-based comparison (policy rates across central banks).
    fn compare_geographies(&self, s: &SlotBindings, parts: &[String]) -> String {
        let mut out = String::new();
        out.push_str("# Comparison: ");
        out.push_str(s.metric.as_deref().unwrap_or("metric"));
        out.push_str("\n\n| Entity | Value | Source tier |\n|--------|-------|-------------|\n");
        let mut rows: Vec<(String, String, f64)> = Vec::new();
        for p in parts {
            let (e, v, num) = match p.as_str() {
                "us"|"fed" => ("US/Fed", "≈ 4.25–4.50%", 4.375),
                "ecb"|"eu" => ("EU/ECB", "≈ 3.15%", 3.15),
                "uk"|"boe" => ("UK/BoE", "≈ 4.50%", 4.50),
                "japan"|"boj" => ("Japan/BoJ", "≈ 0.50%", 0.50),
                "china" => ("China/PBoC", "≈ 3.45%", 3.45),
                "turkey" => ("Turkey/CBRT", "≈ 47.50%", 47.50),
                o => (o, "—", 0.0),
            };
            out.push_str(&format!("| {} | {} | central-bank primary |\n", e, v));
            rows.push((e.to_string(), v.to_string(), num));
        }
        out.push_str("\n## Chart\n\n```\n");
        let max_bars = 30usize;
        let scale = 2.0;
        for (name, _val, num) in &rows {
            let bars = (num * scale).round() as usize;
            let bars = if *num > 0.0 && bars == 0 { 1 } else { bars };
            let bar_str = "█".repeat(bars.min(max_bars));
            out.push_str(&format!("{:10} │{} {:.2}%\n", name, bar_str, num));
        }
        out.push_str("```\n\nFigures are approximate. Always verify against releases.\n");
        out.push_str("Source Profile: federalreserve.gov | ecb.europa.eu | bankofengland.co.uk | boj.or.jp\n");
        out
    }

    /// Asset-based comparison (commodities, currencies).
    /// Uses approximate latest prices and a normalised bar chart.
    fn compare_assets(&self, parts: &[String]) -> String {
        // Approximate latest prices (consistent with asset_price()).
        let price = |asset: &str| -> (String, f64, &'static str) {
            match asset {
                "gold" => ("$2,386/oz".into(), 2386.0, "World Gold Council / LBMA"),
                "oil" => ("$76/bbl".into(), 76.0, "EIA / CME"),
                "silver" => ("$30/oz".into(), 30.0, "Silver Institute / LBMA"),
                "bitcoin" => ("$93,000".into(), 93000.0, "CoinGecko"),
                "gas" => ("$3.20/MMBtu".into(), 3.20, "EIA / CME"),
                "dollar" => ("DXY ≈ 103".into(), 103.0, "ICE"),
                "euro" => ("EUR/USD ≈ 0.92".into(), 0.92, "ECB"),
                "yen" => ("USD/JPY ≈ 155".into(), 155.0, "BoJ"),
                "pound" => ("GBP/USD ≈ 1.27".into(), 1.27, "BoE"),
                _ => ("—".into(), 0.0, "unknown"),
            }
        };

        let mut out = String::new();
        out.push_str("# Comparison: Assets\n\n");
        out.push_str("| Asset | Approx latest | Source |\n|-------|---------------|--------|\n");

        let mut rows: Vec<(String, f64, String)> = Vec::new();
        for p in parts {
            let (label, val, src) = price(p);
            out.push_str(&format!("| {} | {} | {} |\n", p, label, src));
            rows.push((p.clone(), val, label));
        }

        // Normalised bar chart (each bar relative to the max value).
        out.push_str("\n## Relative Chart (normalised)\n\n```\n");
        let max_val = rows.iter().map(|(_, v, _)| *v).fold(0.0_f64, f64::max);
        if max_val > 0.0 {
            let max_bars = 30usize;
            for (name, val, label) in &rows {
                let frac = *val / max_val;
                let bars = (frac * max_bars as f64).round() as usize;
                let bars = if *val > 0.0 && bars == 0 { 1 } else { bars };
                let bar_str = "█".repeat(bars);
                out.push_str(&format!("{:10} │{} {}\n", name, bar_str, label));
            }
        }
        out.push_str("```\n\nFigures are approximate reference values. Enable Network for live prices.\n");
        out
    }
    fn calculate(&self, _s: &SlotBindings, q: &str) -> String {
        let lower = q.to_lowercase();
        if lower.contains("amort") || lower.contains("amortization") || lower.contains("loan schedule") { self.amortization_schedule(&lower) }
        else if lower.contains("bond") || lower.contains("duration") { self.bond(&lower) }
        else if lower.contains("fx") || lower.contains("usdjpy") || lower.contains("move from") || lower.contains("eurusd") { self.fx(&lower) }
        else if lower.contains("compound interest") { self.compound_interest(&lower) }
        else if lower.contains("compound") || lower.contains("cagr") { self.cagr(&lower) }
        else if lower.contains("mortgage") || lower.contains("loan") || lower.contains("monthly payment") { self.mortgage(&lower) }
        else if lower.contains("present value") || lower.contains("pv ") || lower.contains("discount") { self.present_value(&lower) }
        else if lower.contains("inflation adjust") || lower.contains("real value") || lower.contains("inflation-adjusted") { self.inflation_adjust(&lower) }
        else if lower.contains("p/e") || lower.contains("price-earnings") || lower.contains("pe ratio") { self.pe_ratio(&lower) }
        else if lower.contains("debt-to-equity") || lower.contains("debt/equity") || lower.contains("d/e ratio") { self.debt_equity(&lower) }
        else if lower.contains("current ratio") { self.current_ratio(&lower) }
        else if lower.contains("return on equity") || lower.contains("roe") { self.roe(&lower) }
        else if lower.contains("dividend yield") { self.dividend_yield(&lower) }
        else if lower.contains("black-scholes") || lower.contains("option price") || lower.contains("call option") || lower.contains("put option") { self.black_scholes(&lower) }
        else if lower.contains("sharpe ratio") { self.sharpe_ratio(&lower) }
        else if lower.contains("portfolio variance") || lower.contains("portfolio risk") { self.portfolio_variance(&lower) }
        else if lower.contains("break-even") || lower.contains("break even") { self.break_even(&lower) }
        else if lower.contains("wacc") || lower.contains("weighted average cost of capital") { self.wacc(&lower) }
        else if lower.contains("convert") { self.convert_currency(&lower) }
        else { "I couldn't parse a supported formula. Try one of:\\n
- Currency conversion: 'convert 100 USD to EUR' or 'convert 50000 JPY to USD'".to_string() }
    }
    fn amortization_schedule(&self, lower: &str) -> String {
        let principal = regex_or(lower, r"([\d\.]+)\s*k", 300.0) * if lower.contains("k") { 1000.0 } else { 1.0 };
        let principal = if principal < 100.0 {
            regex_or(lower, r"([\d\.]+)", 300000.0)
        } else {
            principal
        };
        let annual_rate = regex_or(lower, r"([\d\.]+)%", 6.5) / 100.0;
        let years = regex_or(lower, r"over\s*(\d+)", 30.0);
        let n = years * 12.0;
        let r = annual_rate / 12.0;
        let payment = if r > 0.0 {
            principal * (r * (1.0 + r).powf(n)) / ((1.0 + r).powf(n) - 1.0)
        } else {
            principal / n
        };

        let mut balance = principal;
        let mut total_interest = 0.0;
        let mut total_principal = 0.0;
        let mut yearly_rows: Vec<(i32, f64, f64, f64, f64)> = Vec::new();

        for month in 1..=n as i32 {
            let interest_payment = balance * r;
            let principal_payment = payment - interest_payment;
            balance -= principal_payment;
            if balance < 0.0 { balance = 0.0; }
            total_interest += interest_payment;
            total_principal += principal_payment;
            if month % 12 == 0 {
                let year = month / 12;
                yearly_rows.push((year, payment * 12.0, total_principal, total_interest, balance));
            }
        }

        let mut table = String::new();
        table.push_str("| Year | Payment | Principal paid | Interest paid | Balance |\n");
        table.push_str("|------|---------|----------------|---------------|---------|\n");
        for (year, pmt, pp, ip, bal) in yearly_rows.iter().take(30) {
            table.push_str(&format!("| {} | ${:.0} | ${:.0} | ${:.0} | ${:.0} |\n", year, pmt, pp, ip, bal));
        }

        format!("# Amortization Schedule\n\n## Inputs\n- Loan amount: ${:.0}\n- Annual rate: {:.2}%\n- Term: {} years\n- Monthly payment: ${:.2}\n\n## Year-by-Year Breakdown\n\n{}\n## Totals\n- Total payments: ${:.2}\n- Total interest: ${:.2}\n- Total principal: ${:.2}\n\n## Note\nEarly payments are mostly interest; later payments shift toward principal.",
            principal, annual_rate * 100.0, years as i64, payment, table, payment * n, total_interest, total_principal)
    }
    fn bond(&self, lower: &str) -> String {
        let d = regex_or(lower, r"(?:duration\s+)?[dD]\s*[=:]\s*(\d+(?:\.\d+)?)", 7.0);
        let bp = regex_or(lower, r"(\d+(?:\.\d+)?)\s*bp", 25.0);
        let pct = -d * (bp/10000.0) * 100.0;
        format!("# Bond Duration Approximation\n\n## Inputs\n- Duration (D): {}\n- Yield change (Δ): {} bp\n\n## Formula\nΔ%price ≈ −D × Δyield\nΔ%price ≈ −{} × {}bp\n\n## Result\nΔ%price ≈ {:.2}%\n\n## Notes\n- Linear approximation; accurate for small moves only\n- Convexity ignored — real loss is slightly less\n- Source: standard fixed-income textbooks", d, bp, d, bp, pct)
    }
    fn fx(&self, lower: &str) -> String {
        let from = regex_or(lower, r"(?:from|@)\s*([\d\.]+)", 150.0);
        let to = regex_or(lower, r"(?:to|→)\s*([\d\.]+)", 155.0);
        let pct = (to - from) / from * 100.0;
        format!("# FX Percentage Change\n\n## Inputs\n- From: {}\n- To: {}\n\n## Formula\n%Δ = (to − from) / from × 100\n%Δ = ({} − {}) / {} × 100\n\n## Result\n%Δ = {:.3}%\n\n## Interpretation\nA {:.2}% {} in the quoted pair.", from, to, to, from, from, pct, pct.abs(), if pct > 0.0 { "appreciation" } else { "depreciation" })
    }
    fn cagr(&self, lower: &str) -> String {
        let start = regex_or(lower, r"(\d+)\s*(?:to|→)", 1000.0);
        let end = regex_or(lower, r"(?:to|→)\s*(\d+)", 2000.0);
        let years = regex_or(lower, r"over\s*(\d+)", 5.0);
        let cagr = ((end / start).powf(1.0 / years) - 1.0) * 100.0;
        format!("# CAGR (Compound Annual Growth Rate)\n\n## Inputs\n- Starting value: {}\n- Ending value: {}\n- Period: {} years\n\n## Formula\nCAGR = (end / start)^(1/years) − 1\nCAGR = ({}/{})^(1/{}) − 1\n\n## Result\nCAGR ≈ {:.2}%", start, end, years, end, start, years, cagr)
    }
    fn mortgage(&self, lower: &str) -> String {
        let principal = regex_or(lower, r"([\d\.]+)\s*k", 300.0) * if lower.contains("k") { 1000.0 } else { 1.0 };
        let principal = if principal < 100.0 { regex_or(lower, r"([\d\.]+)", 300000.0) } else { principal };
        let annual_rate = regex_or(lower, r"([\d\.]+)%", 6.5) / 100.0;
        let years = regex_or(lower, r"over\s*(\d+)", 30.0);
        let n = years * 12.0;
        let r = annual_rate / 12.0;
        let payment = if r > 0.0 {
            principal * (r * (1.0 + r).powf(n)) / ((1.0 + r).powf(n) - 1.0)
        } else {
            principal / n
        };
        let total_paid = payment * n;
        let total_interest = total_paid - principal;
        format!("# Mortgage / Loan Payment\n\n## Inputs\n- Principal: ${:.0}\n- Annual rate: {:.2}%\n- Term: {} years\n\n## Formula\nM = P × [r(1+r)^n] / [(1+r)^n − 1]\n\n## Result\nMonthly payment: ${:.2}\nTotal paid: ${:.2}\nTotal interest: ${:.2}\n\n## Note\nDoes not include taxes, insurance, or fees.", principal, annual_rate * 100.0, years as i64, payment, total_paid, total_interest)
    }
    fn present_value(&self, lower: &str) -> String {
        let future = regex_or(lower, r"([\d\.]+)", 10000.0);
        let rate = regex_or(lower, r"(\d+\.?\d*)%", 3.0) / 100.0;
        let years = regex_or(lower, r"(\d+)\s*years?", 5.0);
        let pv = future / (1.0 + rate).powf(years);
        format!("# Present Value\n\n## Inputs\n- Future value: ${:.0}\n- Discount rate: {:.2}%\n- Period: {} years\n\n## Formula\nPV = FV / (1 + r)^n\nPV = {:.0} / (1 + {:.4})^{}\n\n## Result\nPresent value: ${:.2}", future, rate * 100.0, years as i64, future, rate, years as i64, pv)
    }
    fn inflation_adjust(&self, lower: &str) -> String {
        let amount = regex_or(lower, r"([\d\.]+)", 1000.0);
        let rate = regex_or(lower, r"(\d+\.?\d*)%", 3.0) / 100.0;
        let years = regex_or(lower, r"over\s*(\d+)", 10.0);
        let real_value = amount / (1.0 + rate).powf(years);
        format!("# Inflation-Adjusted Value\n\n## Inputs\n- Nominal amount: ${:.0}\n- Annual inflation: {:.2}%\n- Period: {} years\n\n## Formula\nReal value = Amount / (1 + inflation)^n\nReal value = {:.0} / (1 + {:.4})^{}\n\n## Result\nReal value: ${:.2}\n(purchasing power in today's dollars)", amount, rate * 100.0, years as i64, amount, rate, years as i64, real_value)
    }
    fn pe_ratio(&self, lower: &str) -> String {
        let price = regex_or(lower, r"price\s*([\d\.]+)", 150.0);
        let eps = regex_or(lower, r"eps\s*([\d\.]+)", 10.0);
        let pe = price / eps;
        format!("# P/E Ratio (Price-to-Earnings)\n\n## Inputs\n- Stock price: ${:.2}\n- EPS (earnings per share): ${:.2}\n\n## Formula\nP/E = Price / EPS\nP/E = {:.2} / {:.2}\n\n## Result\nP/E ratio = {:.2}\n\n## Interpretation\n- P/E > 20: potentially overvalued or high growth\n- P/E < 10: potentially undervalued or low growth\n- Compare against industry average", price, eps, price, eps, pe)
    }
    fn debt_equity(&self, lower: &str) -> String {
        let debt = regex_or(lower, r"debt\s*([\d\.]+)", 500000.0);
        let equity = regex_or(lower, r"equity\s*([\d\.]+)", 1000000.0);
        let ratio = debt / equity;
        format!("# Debt-to-Equity Ratio\n\n## Inputs\n- Total debt: ${:.0}\n- Total equity: ${:.0}\n\n## Formula\nD/E = Total Debt / Total Equity\nD/E = {:.0} / {:.0}\n\n## Result\nD/E ratio = {:.2}\n\n## Interpretation\n- D/E < 1: conservative leverage\n- D/E > 2: aggressive leverage\n- Varies by industry", debt, equity, debt, equity, ratio)
    }
    fn current_ratio(&self, lower: &str) -> String {
        let assets = regex_or(lower, r"assets\s*([\d\.]+)", 200000.0);
        let liabilities = regex_or(lower, r"liabilities\s*([\d\.]+)", 100000.0);
        let ratio = assets / liabilities;
        format!("# Current Ratio\n\n## Inputs\n- Current assets: ${:.0}\n- Current liabilities: ${:.0}\n\n## Formula\nCurrent Ratio = Current Assets / Current Liabilities\nCurrent Ratio = {:.0} / {:.0}\n\n## Result\nCurrent ratio = {:.2}\n\n## Interpretation\n- Ratio > 1: can cover short-term obligations\n- Ratio < 1: potential liquidity risk\n- Ideal range: 1.5–3.0", assets, liabilities, assets, liabilities, ratio)
    }
    fn roe(&self, lower: &str) -> String {
        let income = regex_or(lower, r"income\s*([\d\.]+)", 50000.0);
        let equity = regex_or(lower, r"equity\s*([\d\.]+)", 250000.0);
        let roe = (income / equity) * 100.0;
        format!("# Return on Equity (ROE)\n\n## Inputs\n- Net income: ${:.0}\n- Shareholders' equity: ${:.0}\n\n## Formula\nROE = (Net Income / Equity) × 100\nROE = ({:.0} / {:.0}) × 100\n\n## Result\nROE = {:.2}%\n\n## Interpretation\n- ROE > 15%: strong profitability\n- ROE < 10%: below average\n- Compare against industry peers", income, equity, income, equity, roe)
    }
    fn dividend_yield(&self, lower: &str) -> String {
        let dividend = regex_or(lower, r"dividend\s*([\d\.]+)", 4.0);
        let price = regex_or(lower, r"price\s*([\d\.]+)", 100.0);
        let yield_pct = (dividend / price) * 100.0;
        format!("# Dividend Yield\n\n## Inputs\n- Annual dividend per share: ${:.2}\n- Stock price: ${:.2}\n\n## Formula\nDividend Yield = (Annual Dividend / Price) × 100\nDividend Yield = ({:.2} / {:.2}) × 100\n\n## Result\nDividend yield = {:.2}%\n\n## Interpretation\n- Yield > 4%: high income potential\n- Yield < 2%: growth-oriented stock\n- Ensure dividend is sustainable", dividend, price, dividend, price, yield_pct)
    }
    fn black_scholes(&self, lower: &str) -> String {
        let stock_price = regex_or(lower, r"stock\s*(?:price)?\s*([\d\.]+)", 100.0);
        let strike = regex_or(lower, r"strike\s*([\d\.]+)", 100.0);
        let time = regex_or(lower, r"time\s*([\d\.]+)", 1.0);
        let rate = regex_or(lower, r"rate\s*([\d\.]+)", 5.0) / 100.0;
        let vol = regex_or(lower, r"vol(?:atility)?\s*([\d\.]+)", 20.0) / 100.0;
        let d1 = ((stock_price / strike).ln() + (rate + vol * vol / 2.0) * time) / (vol * time.sqrt());
        let d2 = d1 - vol * time.sqrt();
        let call = stock_price * normal_cdf(d1) - strike * (-rate * time).exp() * normal_cdf(d2);
        let put = strike * (-rate * time).exp() * normal_cdf(-d2) - stock_price * normal_cdf(-d1);
        format!("# Black-Scholes Option Pricing\n\n## Inputs\n- Stock price: ${:.2}\n- Strike price: ${:.2}\n- Time to expiry: {} years\n- Risk-free rate: {:.2}%\n- Volatility: {:.2}%\n\n## Formula\nCall = S·N(d₁) − K·e^(−rT)·N(d₂)\nPut  = K·e^(−rT)·N(−d₂) − S·N(−d₁)\n\n## Result\nCall option: ${:.2}\nPut option: ${:.2}\n\n## Note\nTheoretical price under Black-Scholes assumptions. Actual market prices may differ due to supply/demand, discrete hedging, and model limitations.\n\n**Source**: Black & Scholes (1973)", stock_price, strike, time as i64, rate * 100.0, vol * 100.0, call, put)
    }
    fn sharpe_ratio(&self, lower: &str) -> String {
        let expected_return = regex_or(lower, r"return\s*([\d\.]+)", 10.0);
        let risk_free_rate = regex_or(lower, r"risk.?free\s*([\d\.]+)", 3.0);
        let std_dev = regex_or(lower, r"dev(?:iation)?\s*([\d\.]+)", 15.0);
        let sharpe = (expected_return - risk_free_rate) / std_dev;
        format!("# Sharpe Ratio\n\n## Inputs\n- Expected return: {:.2}%\n- Risk-free rate: {:.2}%\n- Standard deviation: {:.2}%\n\n## Formula\nSharpe = (Rₚ − Rբ) / σ\nSharpe = ({:.2} − {:.2}) / {:.2}\n\n## Result\nSharpe ratio = {:.2}\n\n## Interpretation\n- Sharpe > 1: good risk-adjusted return\n- Sharpe > 2: very good\n- Sharpe > 3: excellent\n- Sharpe < 0: worse than risk-free", expected_return, risk_free_rate, std_dev, expected_return, risk_free_rate, std_dev, sharpe)
    }
    fn portfolio_variance(&self, lower: &str) -> String {
        let weight_a = regex_or(lower, r"weight.?a\s*([\d\.]+)", 60.0) / 100.0;
        let weight_b = regex_or(lower, r"weight.?b\s*([\d\.]+)", 40.0) / 100.0;
        let vol_a = regex_or(lower, r"vol(?:atility)?.?a\s*([\d\.]+)", 20.0) / 100.0;
        let vol_b = regex_or(lower, r"vol(?:atility)?.?b\s*([\d\.]+)", 15.0) / 100.0;
        let correlation = regex_or(lower, r"corr(?:elation)?\s*([\d\.]+)", 0.3);
        let var = weight_a*weight_a*vol_a*vol_a + weight_b*weight_b*vol_b*vol_b + 2.0*weight_a*weight_b*vol_a*vol_b*correlation;
        let std_dev = var.sqrt();
        format!("# Portfolio Variance / Risk\n\n## Inputs\n- Weight A: {:.0}%, Volatility A: {:.0}%\n- Weight B: {:.0}%, Volatility B: {:.0}%\n- Correlation: {:.2}\n\n## Formula\nσ²ₚ = wₐ²σₐ² + wᵦ²σᵦ² + 2wₐwᵦσₐσᵦρ\n\n## Result\nPortfolio variance: {:.4}\nPortfolio std dev: {:.2}%\n\n## Interpretation\nDiversification benefit: lower portfolio risk than weighted average of individual risks.\nCorrelation < 1 provides diversification benefit.", weight_a*100.0, vol_a*100.0, weight_b*100.0, vol_b*100.0, correlation, var, std_dev*100.0)
    }
    fn break_even(&self, lower: &str) -> String {
        let fixed = regex_or(lower, r"fixed\s*costs?\s*([\d\.]+)", 10000.0);
        let price = regex_or(lower, r"price\s*([\d\.]+)", 50.0);
        let variable = regex_or(lower, r"variable\s*([\d\.]+)", 30.0);
        let contribution = price - variable;
        let break_even_units = fixed / contribution;
        let break_even_revenue = break_even_units * price;
        format!("# Break-Even Analysis\n\n## Inputs\n- Fixed costs: ${:.0}\n- Price per unit: ${:.2}\n- Variable cost per unit: ${:.2}\n\n## Formula\nContribution margin = Price − Variable cost = ${:.2}\nBreak-even (units) = Fixed costs / Contribution margin\nBreak-even (units) = {:.0} / {:.2}\n\n## Result\nBreak-even point = {:.0} units\nBreak-even revenue = ${:.2}\n\n## Interpretation\nYou need to sell {:.0} units to cover all costs.\nEvery unit beyond this contributes ${:.2} to profit.", fixed, price, variable, contribution, fixed, contribution, break_even_units, break_even_revenue, break_even_units, contribution)
    }
    fn wacc(&self, lower: &str) -> String {
        let equity = regex_or(lower, r"equity\s*([\d\.]+)", 60000.0);
        let debt = regex_or(lower, r"debt\s*([\d\.]+)", 40000.0);
        let cost_equity = regex_or(lower, r"cost\s*equity\s*([\d\.]+)", 10.0) / 100.0;
        let cost_debt = regex_or(lower, r"cost\s*debt\s*([\d\.]+)", 5.0) / 100.0;
        let tax = regex_or(lower, r"tax\s*([\d\.]+)", 21.0) / 100.0;
        let total = equity + debt;
        let wacc = (equity / total) * cost_equity + (debt / total) * cost_debt * (1.0 - tax);
        format!("# WACC (Weighted Average Cost of Capital)\n\n## Inputs\n- Equity: ${:.0} ({:.0}%)\n- Debt: ${:.0} ({:.0}%)\n- Cost of equity: {:.2}%\n- Cost of debt: {:.2}%\n- Tax rate: {:.0}%\n\n## Formula\nWACC = (E/V × Re) + (D/V × Rd × (1 − T))\nWACC = ({:.0}/{:.0} × {:.4}) + ({:.0}/{:.0} × {:.4} × {:.2})\n\n## Result\nWACC = {:.2}%\n\n## Interpretation\nWACC is the average rate of return required by all of the company's security holders.\nUse it as the discount rate for NPV calculations.", equity, equity/total*100.0, debt, debt/total*100.0, cost_equity*100.0, cost_debt*100.0, tax*100.0, equity, total, cost_equity, debt, total, cost_debt, 1.0-tax, wacc*100.0)
    }
    fn compound_interest(&self, lower: &str) -> String {
        let principal = regex_or(lower, r"([\d\.]+)", 10000.0);
        let rate = regex_or(lower, r"(\d+\.?\d*)%", 5.0) / 100.0;
        let years = regex_or(lower, r"over\s*(\d+)", 10.0);
        let n = if lower.contains("monthly") { 12.0 } else if lower.contains("quarterly") { 4.0 } else if lower.contains("daily") { 365.0 } else { 1.0 };
        let amount = principal * (1.0 + rate/n).powf(n * years);
        let interest = amount - principal;
        format!("# Compound Interest\n\n## Inputs\n- Principal: ${:.0}\n- Annual rate: {:.2}%\n- Period: {} years\n- Compounding: {} times/year\n\n## Formula\nA = P(1 + r/n)^(nt)\nA = {:.0}(1 + {:.4}/{})^({}×{})\n\n## Result\nFinal amount: ${:.2}\nInterest earned: ${:.2}\n\n## Note\nThe power of compounding: interest earns interest.", principal, rate*100.0, years as i64, n as i64, principal, rate, n as i64, n as i64, years as i64, amount, interest)
    }
    fn convert_currency(&self, lower: &str) -> String {
        // Parse amount and source currency together (amount followed by currency code)
        let re = regex::Regex::new(r"(\d+(?:\.\d+)?)\s*(usd|eur|jpy|gbp|chf|cad|aud|cny|inr|try)").ok();
        let (amount, from_curr) = if let Some(ref r) = re {
            if let Some(cap) = r.captures(lower) {
                let amt = cap.get(1).and_then(|m| m.as_str().parse::<f64>().ok()).unwrap_or(100.0);
                let curr = cap.get(2).map(|m| m.as_str().to_uppercase()).unwrap_or_else(|| "USD".to_string());
                (amt, curr)
            } else {
                (100.0, "USD".to_string())
            }
        } else {
            (100.0, "USD".to_string())
        };
        
        // Detect target currency — look for "to X", "in X", "-> X" patterns
        let to_curr = if lower.contains("to jpy") || lower.contains("in jpy") || lower.contains("-> jpy") { "JPY" }
            else if lower.contains("to eur") || lower.contains("in eur") || lower.contains("-> eur") { "EUR" }
            else if lower.contains("to gbp") || lower.contains("in gbp") || lower.contains("-> gbp") { "GBP" }
            else if lower.contains("to chf") || lower.contains("in chf") || lower.contains("-> chf") { "CHF" }
            else if lower.contains("to cad") || lower.contains("in cad") || lower.contains("-> cad") { "CAD" }
            else if lower.contains("to aud") || lower.contains("in aud") || lower.contains("-> aud") { "AUD" }
            else if lower.contains("to cny") || lower.contains("in cny") || lower.contains("-> cny") { "CNY" }
            else if lower.contains("to inr") || lower.contains("in inr") || lower.contains("-> inr") { "INR" }
            else if lower.contains("to try") || lower.contains("in try") || lower.contains("-> try") { "TRY" }
            else if lower.contains("to usd") || lower.contains("in usd") || lower.contains("-> usd") { "USD" }
            else { "EUR" }; // default
        
        // Rate matrix: units of currency per 1 USD
        let rate = |c: &str| -> f64 {
            match c {
                "USD" => 1.0,
                "EUR" => 0.92,
                "JPY" => 155.0,
                "GBP" => 0.79,
                "CHF" => 0.88,
                "CAD" => 1.36,
                "AUD" => 1.53,
                "CNY" => 7.24,
                "INR" => 83.0,
                "TRY" => 32.0,
                _ => 1.0,
            }
        };
        
        let from_rate = rate(&from_curr);
        let to_rate = rate(&to_curr);
        let usd_amount = amount / from_rate;
        let result = usd_amount * to_rate;
        
        format!("# Currency Conversion\n\n## Inputs\n- Amount: {:.2} {}\n- Convert to: {}\n\n## Rate Matrix (per 1 USD)\n| Currency | Rate |\n|----------|------|\n| USD | 1.0000 |\n| EUR | 0.9200 |\n| JPY | 155.00 |\n| GBP | 0.7900 |\n| CHF | 0.8800 |\n| CAD | 1.3600 |\n| AUD | 1.5300 |\n| CNY | 7.2400 |\n| INR | 83.00 |\n| TRY | 32.00 |\n\n## Calculation\n{:.2} {} → USD → {}\n{:.2} / {:.4} × {:.4} = {:.2}\n\n## Result\n{:.2} {} = {:.2} {}\n\n## Note\nRates are approximate reference values. For live rates, enable Network mode.\n\n**Source**: Approximate interbank reference rates.", 
            amount, from_curr, to_curr, amount, from_curr, to_curr, amount, from_rate, to_rate, result, amount, from_curr, result, to_curr)
    }
    fn trend(&self, s: &SlotBindings, _question: &str) -> String {
        let subj = s.subject.as_deref().unwrap_or("metric");
        let geo = s.geography.as_deref().unwrap_or("US");

        // Determine which historical series to show
        let (label, unit, data, source): (String, &str, Vec<(String, f64)>, &str) =
            if subj.contains("cpi") || subj.contains("inflation") {
                (
                    format!("CPI ({}) — Annual %", geo),
                    "%",
                    vec![
                        ("2019".into(), 2.3),
                        ("2020".into(), 1.4),
                        ("2021".into(), 4.7),
                        ("2022".into(), 8.0),
                        ("2023".into(), 4.1),
                        ("2024".into(), 3.2),
                    ],
                    "BLS / Eurostat / national statistics",
                )
            } else if subj.contains("policy rate") || subj.contains("rate") {
                let rate_data: Vec<(String, f64)> = if geo == "eu" || geo == "ecb" {
                    vec![
                        ("2019".into(), 0.0),
                        ("2020".into(), 0.0),
                        ("2021".into(), 0.0),
                        ("2022".into(), 1.25),
                        ("2023".into(), 3.0),
                        ("2024".into(), 3.15),
                    ]
                } else if geo == "uk" || geo == "boe" {
                    vec![
                        ("2019".into(), 0.75),
                        ("2020".into(), 0.1),
                        ("2021".into(), 0.25),
                        ("2022".into(), 2.25),
                        ("2023".into(), 4.5),
                        ("2024".into(), 4.5),
                    ]
                } else if geo == "japan" || geo == "boj" {
                    vec![
                        ("2019".into(), -0.1),
                        ("2020".into(), -0.1),
                        ("2021".into(), -0.1),
                        ("2022".into(), -0.1),
                        ("2023".into(), 0.0),
                        ("2024".into(), 0.5),
                    ]
                } else {
                    // US / Fed default
                    vec![
                        ("2019".into(), 2.4),
                        ("2020".into(), 0.25),
                        ("2021".into(), 0.25),
                        ("2022".into(), 4.5),
                        ("2023".into(), 4.5),
                        ("2024".into(), 4.375),
                    ]
                };
                (
                    format!("Policy Rate ({})", geo),
                    "%",
                    rate_data,
                    "Federal Reserve / ECB / BoE / BoJ",
                )
            } else if subj.contains("yield") {
                (
                    format!("10Y Bond Yield ({})", geo),
                    "%",
                    vec![
                        ("2019".into(), 2.1),
                        ("2020".into(), 0.9),
                        ("2021".into(), 1.5),
                        ("2022".into(), 3.0),
                        ("2023".into(), 4.2),
                        ("2024".into(), 4.3),
                    ],
                    "Bloomberg / Treasury.gov",
                )
            } else {
                (
                    format!("{} ({})", subj, geo),
                    "",
                    vec![
                        ("2019".into(), 100.0),
                        ("2020".into(), 95.0),
                        ("2021".into(), 120.0),
                        ("2022".into(), 110.0),
                        ("2023".into(), 135.0),
                        ("2024".into(), 145.0),
                    ],
                    "Finny local cache",
                )
            };

        // Generate ASCII chart
        let chart = render_ascii_chart(&data, label.as_str(), unit);

        let min_val = data.iter().map(|(_, v)| *v).fold(f64::INFINITY, f64::min);
        let max_val = data.iter().map(|(_, v)| *v).fold(f64::NEG_INFINITY, f64::max);
        let first_val = data.first().map(|(_, v)| *v).unwrap_or(0.0);
        let last_val = data.last().map(|(_, v)| *v).unwrap_or(0.0);
        let change = last_val - first_val;
        let direction = if change > 0.0 { "↑" } else if change < 0.0 { "↓" } else { "→" };

        format!("# Trend: {}\n\n## Historical Data\n\n{}\n\n## Summary\n- Period: {}–{}\n- Range: {:.2}{} to {:.2}{}\n- Change: {:+.2}{} {}\n\n## Source Profile\n- {}\n- Retrieved at: {} UTC\n\n## Reliability note\nFinny hard-codes approximate historical figures. Enable Network or consult the primary source for live data.",
            label, chart, data.first().map(|(y, _)| y.as_str()).unwrap_or("?"),
            data.last().map(|(y, _)| y.as_str()).unwrap_or("?"),
            min_val, unit, max_val, unit, change, unit, direction,
            source, chrono::Utc::now().format("%Y-%m-%d %H:%M"))
    }

    fn forecast(&self, s: &SlotBindings, question: &str) -> String {
        let subj = s.subject.as_deref().unwrap_or("metric");
        let geo = s.geography.as_deref().unwrap_or("US");
        let lower = question.to_lowercase();

        // Determine number of years to forecast
        let forecast_years = if lower.contains("next quarter") || lower.contains("next 3 months") {
            1
        } else if lower.contains("next 2 years") || lower.contains("two years") {
            2
        } else if lower.contains("next 5 years") || lower.contains("five years") {
            5
        } else {
            3 // default: 3 years
        };

        // Get historical data (reuse trend data selection)
        let (label, unit, data, source): (String, &str, Vec<(String, f64)>, &str) =
            if subj.contains("cpi") || subj.contains("inflation") {
                (
                    format!("CPI ({}) — Forecast", geo),
                    "%",
                    vec![
                        ("2019".into(), 2.3),
                        ("2020".into(), 1.4),
                        ("2021".into(), 4.7),
                        ("2022".into(), 8.0),
                        ("2023".into(), 4.1),
                        ("2024".into(), 3.2),
                    ],
                    "BLS / Eurostat / national statistics",
                )
            } else if subj.contains("policy rate") || subj.contains("rate") {
                let rate_data: Vec<(String, f64)> = if geo == "eu" || geo == "ecb" {
                    vec![
                        ("2019".into(), 0.0),
                        ("2020".into(), 0.0),
                        ("2021".into(), 0.0),
                        ("2022".into(), 1.25),
                        ("2023".into(), 3.0),
                        ("2024".into(), 3.15),
                    ]
                } else if geo == "uk" || geo == "boe" {
                    vec![
                        ("2019".into(), 0.75),
                        ("2020".into(), 0.1),
                        ("2021".into(), 0.25),
                        ("2022".into(), 2.25),
                        ("2023".into(), 4.5),
                        ("2024".into(), 4.5),
                    ]
                } else if geo == "japan" || geo == "boj" {
                    vec![
                        ("2019".into(), -0.1),
                        ("2020".into(), -0.1),
                        ("2021".into(), -0.1),
                        ("2022".into(), -0.1),
                        ("2023".into(), 0.0),
                        ("2024".into(), 0.5),
                    ]
                } else {
                    vec![
                        ("2019".into(), 2.4),
                        ("2020".into(), 0.25),
                        ("2021".into(), 0.25),
                        ("2022".into(), 4.5),
                        ("2023".into(), 4.5),
                        ("2024".into(), 4.375),
                    ]
                };
                (
                    format!("Policy Rate ({}) — Forecast", geo),
                    "%",
                    rate_data,
                    "Federal Reserve / ECB / BoE / BoJ",
                )
            } else if subj.contains("yield") {
                (
                    format!("10Y Bond Yield ({}) — Forecast", geo),
                    "%",
                    vec![
                        ("2019".into(), 2.1),
                        ("2020".into(), 0.9),
                        ("2021".into(), 1.5),
                        ("2022".into(), 3.0),
                        ("2023".into(), 4.2),
                        ("2024".into(), 4.3),
                    ],
                    "Bloomberg / Treasury.gov",
                )
            } else if subj.contains("gold") {
                (
                    "Gold (XAU/USD) — Forecast".to_string(),
                    "$/oz",
                    vec![
                        ("2019".into(), 1392.0),
                        ("2020".into(), 1770.0),
                        ("2021".into(), 1829.0),
                        ("2022".into(), 1800.0),
                        ("2023".into(), 1940.0),
                        ("2024".into(), 2386.0),
                    ],
                    "World Gold Council / LBMA",
                )
            } else if subj.contains("oil") {
                (
                    "Crude Oil (WTI) — Forecast".to_string(),
                    "$/bbl",
                    vec![
                        ("2019".into(), 57.0),
                        ("2020".into(), 39.0),
                        ("2021".into(), 68.0),
                        ("2022".into(), 80.0),
                        ("2023".into(), 72.0),
                        ("2024".into(), 76.0),
                    ],
                    "EIA / CME",
                )
            } else if subj.contains("bitcoin") || subj.contains("btc") {
                (
                    "Bitcoin (BTC/USD) — Forecast".to_string(),
                    "$",
                    vec![
                        ("2019".into(), 7193.0),
                        ("2020".into(), 29000.0),
                        ("2021".into(), 46300.0),
                        ("2022".into(), 16500.0),
                        ("2023".into(), 42000.0),
                        ("2024".into(), 93000.0),
                    ],
                    "CoinGecko / Coinbase",
                )
            } else {
                (
                    format!("{} ({}) — Forecast", subj, geo),
                    "",
                    vec![
                        ("2019".into(), 100.0),
                        ("2020".into(), 95.0),
                        ("2021".into(), 120.0),
                        ("2022".into(), 110.0),
                        ("2023".into(), 135.0),
                        ("2024".into(), 145.0),
                    ],
                    "Finny local cache",
                )
            };

        // Linear regression on historical data
        let n = data.len() as f64;
        let x_vals: Vec<f64> = (0..data.len()).map(|i| i as f64).collect();
        let y_vals: Vec<f64> = data.iter().map(|(_, v)| *v).collect();

        let x_mean = x_vals.iter().sum::<f64>() / n;
        let y_mean = y_vals.iter().sum::<f64>() / n;

        let mut ss_xy = 0.0;
        let mut ss_xx = 0.0;
        let mut ss_yy = 0.0;
        for i in 0..data.len() {
            let dx = x_vals[i] - x_mean;
            let dy = y_vals[i] - y_mean;
            ss_xy += dx * dy;
            ss_xx += dx * dx;
            ss_yy += dy * dy;
        }

        let slope = if ss_xx > 0.0 { ss_xy / ss_xx } else { 0.0 };
        let intercept = y_mean - slope * x_mean;

        // R-squared
        let r_squared = if ss_yy > 0.0 {
            let ss_res: f64 = (0..data.len())
                .map(|i| {
                    let predicted = intercept + slope * x_vals[i];
                    (y_vals[i] - predicted).powi(2)
                })
                .sum();
            1.0 - ss_res / ss_yy
        } else {
            1.0
        };

        // Generate forecast data points
        let last_year = data
            .last()
            .map(|(y, _)| y.parse::<i32>().unwrap_or(2024))
            .unwrap_or(2024);

        let mut forecast_data: Vec<(String, f64)> = Vec::new();
        for i in 1..=forecast_years {
            let x = data.len() as f64 - 1.0 + i as f64;
            let predicted = intercept + slope * x;
            let year = last_year + i as i32;
            forecast_data.push((format!("{}f", year), predicted));
        }

        // Confidence band: ±1 standard error
        let std_error = if n > 2.0 {
            let ss_res: f64 = (0..data.len())
                .map(|i| {
                    let predicted = intercept + slope * x_vals[i];
                    (y_vals[i] - predicted).powi(2)
                })
                .sum();
            (ss_res / (n - 2.0)).sqrt()
        } else {
            0.0
        };

        // Build combined chart data (historical + forecast)
        let mut chart_data: Vec<(String, f64)> = data.clone();
        chart_data.extend(forecast_data.clone());
        let chart = render_ascii_chart(&chart_data, label.as_str(), unit);

        // Forecast summary
        let last_actual = data.last().map(|(_, v)| *v).unwrap_or(0.0);
        let last_forecast = forecast_data.last().map(|(_, v)| *v).unwrap_or(0.0);
        let forecast_change = last_forecast - last_actual;
        let direction = if forecast_change > 0.0 {
            "↑"
        } else if forecast_change < 0.0 {
            "↓"
        } else {
            "→"
        };

        let confidence = if r_squared > 0.8 {
            "HIGH"
        } else if r_squared > 0.5 {
            "MODERATE"
        } else {
            "LOW"
        };

        let mut forecast_table = String::new();
        forecast_table.push_str("| Year | Projected | Low band | High band |\n");
        forecast_table.push_str("|------|-----------|----------|-----------|\n");
        for (year, val) in &forecast_data {
            forecast_table.push_str(&format!(
                "| {} | {:.2}{} | {:.2}{} | {:.2}{} |\n",
                year,
                val,
                unit,
                val - std_error,
                unit,
                val + std_error,
                unit
            ));
        }

        format!("# Forecast: {}\n\n## Historical + Projected\n\n{}\n\n## Forecast Table\n\n{}\n\n## Model Summary\n- Method: Simple linear regression\n- R² = {:.3} ({} confidence)\n- Slope: {}{}{} per year\n- Forecast horizon: {} years\n- Std error: {:.2}{}\n\n## Interpretation\n- {} forecast of {}{}{} from {:.2}{} to {:.2}{}\n- Wide confidence bands indicate uncertainty in linear extrapolation\n\n## Source Profile\n- {}\n- Retrieved at: {} UTC\n\n## Reliability note\nForecasts are mechanical linear extrapolations of approximate historical data. They do NOT account for policy shifts, black swans, or regime changes. Enable Network or consult primary sources for consensus forecasts.",
            label, chart, forecast_table, r_squared, confidence,
            if slope >= 0.0 { "+" } else { "" }, slope, unit,
            forecast_years, std_error, unit,
            direction,
            if forecast_change >= 0.0 { "+" } else { "" }, forecast_change, unit,
            last_actual, unit,
            last_forecast, unit,
            source,
            chrono::Utc::now().format("%Y-%m-%d %H:%M")
        )
    }

    fn asset_price(&self, s: &SlotBindings, _question: &str) -> String {
        let subj = s.subject.as_deref().unwrap_or("asset");

        let (label, unit, data, source): (String, &str, Vec<(String, f64)>, &str) =
            if subj.contains("gold") {
                (
                    "Gold (XAU/USD)".to_string(),
                    "$/oz",
                    vec![
                        ("2019".into(), 1392.0),
                        ("2020".into(), 1770.0),
                        ("2021".into(), 1829.0),
                        ("2022".into(), 1800.0),
                        ("2023".into(), 1940.0),
                        ("2024".into(), 2386.0),
                    ],
                    "World Gold Council / LBMA",
                )
            } else if subj.contains("oil") {
                (
                    "Crude Oil (WTI)".to_string(),
                    "$/bbl",
                    vec![
                        ("2019".into(), 57.0),
                        ("2020".into(), 39.0),
                        ("2021".into(), 68.0),
                        ("2022".into(), 80.0),
                        ("2023".into(), 72.0),
                        ("2024".into(), 76.0),
                    ],
                    "EIA / CME",
                )
            } else if subj.contains("bitcoin") || subj.contains("btc") {
                (
                    "Bitcoin (BTC/USD)".to_string(),
                    "$",
                    vec![
                        ("2019".into(), 7193.0),
                        ("2020".into(), 29000.0),
                        ("2021".into(), 46300.0),
                        ("2022".into(), 16500.0),
                        ("2023".into(), 42000.0),
                        ("2024".into(), 93000.0),
                    ],
                    "CoinGecko / Coinbase",
                )
            } else if subj.contains("silver") {
                (
                    "Silver (XAG/USD)".to_string(),
                    "$/oz",
                    vec![
                        ("2019".into(), 16.2),
                        ("2020".into(), 20.5),
                        ("2021".into(), 23.3),
                        ("2022".into(), 23.9),
                        ("2023".into(), 23.8),
                        ("2024".into(), 30.0),
                    ],
                    "Silver Institute / LBMA",
                )
            } else if subj.contains("gas") {
                (
                    "Natural Gas (Henry Hub)".to_string(),
                    "$/MMBtu",
                    vec![
                        ("2019".into(), 2.5),
                        ("2020".into(), 2.0),
                        ("2021".into(), 3.9),
                        ("2022".into(), 6.4),
                        ("2023".into(), 2.5),
                        ("2024".into(), 3.2),
                    ],
                    "EIA / CME",
                )
            } else {
                (
                    format!("{} (price)", subj),
                    "$",
                    vec![
                        ("2019".into(), 100.0),
                        ("2020".into(), 95.0),
                        ("2021".into(), 120.0),
                        ("2022".into(), 110.0),
                        ("2023".into(), 135.0),
                        ("2024".into(), 145.0),
                    ],
                    "Finny local cache",
                )
            };

        let chart = render_ascii_chart(&data, label.as_str(), unit);

        let min_val = data.iter().map(|(_, v)| *v).fold(f64::INFINITY, f64::min);
        let max_val = data.iter().map(|(_, v)| *v).fold(f64::NEG_INFINITY, f64::max);
        let first_val = data.first().map(|(_, v)| *v).unwrap_or(0.0);
        let last_val = data.last().map(|(_, v)| *v).unwrap_or(0.0);
        let change = last_val - first_val;
        let pct_change = if first_val != 0.0 { (change / first_val) * 100.0 } else { 0.0 };
        let direction = if change > 0.0 { "↑" } else if change < 0.0 { "↓" } else { "→" };

        format!("# Asset Price: {}\n\n## Historical Price Chart\n\n{}\n\n## Summary\n- Period: {}–{}\n- Range: {:.2}{} to {:.2}{}\n- Change: {:+.2}{} ({:.1}%)\n- Trend: {}\n\n## Source Profile\n- {}\n- Retrieved at: {} UTC\n\n## Reliability note\nFinny hard-codes approximate historical prices. Enable Network or consult the primary source for live market data.",
            label, chart, data.first().map(|(y, _)| y.as_str()).unwrap_or("?"),
            data.last().map(|(y, _)| y.as_str()).unwrap_or("?"),
            min_val, unit, max_val, unit, change, unit, pct_change, direction,
            source, chrono::Utc::now().format("%Y-%m-%d %H:%M"))
    }

    fn scenario(&self, _s: &SlotBindings, q: &str) -> String {
        if q.to_lowercase().contains("yen") || q.to_lowercase().contains("carry") {
            "# Carry-Trade Scenario (bounded)\n\n## Parameters\n- FX move: yen +8% vs USD\n- Borrowing (JPY): ≈ 0.5%\n- US yield: ≈ 4.5% → gross carry ≈ 4.0%\n\n## Output band\n| Scenario | Unwind % | Impact |\n|----------|----------|--------|\n| Low | 30% | 2–4% correction |\n| Base | 50% | 5–8% correction |\n| High | Systemic | 10–15% correction |\n\n## Reproducible assumptions\n- Carry not fully hedged\n- No policy intervention in 2 weeks\n- Vol-target rebalancing amplifies\n\n## Source methodology\nScenario analysis per BIS/IMF working papers on yen carry unwind dynamics.".to_string()
        } else { "My registry only models yen-carry scenarios directly. Ask 'how could yen appreciation affect carry trades?'".to_string() }
    }
    fn unsupported(&self, q: &str) -> String {
        format!("Finn refuses to hallucinate this answer: \"{}\"\n\nSupported queries:\n- latest METRIC for COUNTRY\n- compare METRIC across COUNTRY_LIST\n- calculate X given Y\n- explain likely effects of EVENT on ASSET\n- define FINANCE_TERM", q.trim())
    }
    fn define_term(&self, q: &str) -> String {
        let lower = q.to_lowercase();
        if lower.contains("duration") {
            "# Definition: Bond Duration\n\n**Duration** measures the sensitivity of a bond's price to changes in yield.\n\n- Macaulay duration: weighted-average time to receive cash flows (years)\n- Modified duration: % price change for a 1% yield change\n- Δ%price ≈ −D × Δyield\n\n**Example**: D=7 means ~7% price drop for a 1% (100bp) yield rise.\n\n**Source**: Fabozzi, *Fixed Income Analysis*".to_string()
        } else if lower.contains("carry trade") {
            "# Definition: Carry Trade\n\n**Carry trade**: borrow in a low-yield currency, invest in a higher-yield one, pocket the spread.\n\n- **Profit** = yield differential − FX depreciation\n- **Risk**: adverse FX move wipes the carry\n- **Classic**: borrow JPY (~0.5%), buy USD assets (~4.5%)\n\n**Source**: BIS Quarterly Review".to_string()
        } else if lower.contains("cpi") || lower.contains("consumer price index") {
            "# Definition: CPI (Consumer Price Index)\n\n**CPI** measures the average change over time in prices paid by urban consumers for a market basket.\n\n- Headline: all items\n- Core: excludes food and energy\n- Reported monthly by national statistical agencies\n- **Key metric**: YoY % change (inflation rate)\n\n**Source**: Bureau of Labor Statistics (US)".to_string()
        } else if lower.contains("yield") || lower.contains("bond yield") {
            "# Definition: Bond Yield\n\n**Yield** is the return an investor earns on a bond.\n\n- **Coupon rate**: fixed % of face value paid annually\n- **Current yield**: coupon / current price\n- **YTM**: total return if held to maturity (reinvested coupons)\n- **Inverse relationship**: yield up → price down\n\n**Source**: SEC / Treasury.gov".to_string()
        } else if lower.contains("fx") || lower.contains("foreign exchange") || lower.contains("currency") {
            "# Definition: FX (Foreign Exchange)\n\n**FX** is the market for trading national currencies.\n\n- **Spot**: immediate delivery (T+2)\n- **Forward**: agreed price, future delivery\n- **Bid-ask**: spread between buy and sell prices\n- **Majors**: USD/EUR/JPY/GBP/CHF/CAD/AUD\n\n**Source**: BIS Triennial Survey".to_string()
        } else if lower.contains("gdp") {
            "# Definition: GDP (Gross Domestic Product)\n\n**GDP** is the total value of goods and services produced in a country over a period.\n\n- **Nominal**: at current prices\n- **Real**: adjusted for inflation\n- **YoY growth**: most-watched headline number\n- Reported quarterly (advance, preliminary, final)\n\n**Source**: BEA (US)".to_string()
        } else if lower.contains("cagr") {
            "# Definition: CAGR (Compound Annual Growth Rate)\n\n**CAGR** smooths an investment's growth into an annualized rate.\n\n- Formula: CAGR = (end / start)^(1/years) − 1\n- Ignores volatility — assumes steady growth\n- Good for comparing funds over the same period\n\n**Example**: $1000 → $2000 over 5 years = 14.87% CAGR\n\n**Source**: CFA Institute".to_string()
        } else if lower.contains("inflation") {
            "# Definition: Inflation\n\n**Inflation** is the rate at which the general level of prices rises, eroding purchasing power.\n\n- **Headline**: all items\n- **Core**: excludes volatile food & energy\n- **Target**: most central banks target 2%\n- Measured by CPI, PCE, HICP (EU)\n\n**Source**: Federal Reserve / ECB".to_string()
        } else if lower.contains("recession") {
            "# Definition: Recession\n\n**Recession** is a significant, prolonged economic decline.\n\n- **Rule of thumb**: two consecutive quarters of negative GDP\n- **US official**: declared by NBER (not just GDP)\n- Features: rising unemployment, falling output, lower inflation\n- **Depression**: severe recession lasting 3+ years\n\n**Source**: NBER, IMF".to_string()
        } else if lower.contains("mortgage") || lower.contains("loan") {
            "# Definition: Mortgage\n\n**Mortgage** is a loan used to purchase real estate, where the property itself serves as collateral.\n\n- **Fixed-rate**: same interest rate for the full term\n- **ARM**: rate adjusts periodically (e.g., 5/1 ARM)\n- **Amortization**: early payments are mostly interest, later mostly principal\n- **Typical terms**: 15 or 30 years\n\n**Source**: CFPB, Freddie Mac".to_string()
        } else if lower.contains("present value") || lower.contains("discount") {
            "# Definition: Present Value (PV)\n\n**Present value** is the current worth of a future sum of money, given a specified rate of return.\n\n- Formula: PV = FV / (1 + r)^n\n- Foundation of discounted cash flow (DCF) analysis\n- $100 today is worth more than $100 in 5 years\n\n**Source**: Brealey, Myers, *Principles of Corporate Finance*".to_string()
        } else if lower.contains("p/e") || lower.contains("price-earnings") {
            "# Definition: P/E Ratio\n\n**P/E ratio** compares a company's stock price to its earnings per share.\n\n- Formula: P/E = Price / EPS\n- High P/E: growth expectations or overvaluation\n- Low P/E: value or distress\n- Always compare within the same industry\n\n**Source**: Damodaran, *Investment Valuation*".to_string()
        } else if lower.contains("wacc") {
            "# Definition: WACC\n\n**WACC** (Weighted Average Cost of Capital) is the average rate a company pays to finance its assets.\n\n- Blends cost of equity and after-tax cost of debt\n- Used as the discount rate in DCF valuations\n- Lower WACC = cheaper financing = higher valuation\n\n**Source**: Brealey, Myers, *Principles of Corporate Finance*".to_string()
        } else if lower.contains("break-even") || lower.contains("break even") {
            "# Definition: Break-Even Analysis\n\n**Break-even** is the point where total revenue equals total costs — no profit, no loss.\n\n- Formula: Fixed costs / (Price − Variable cost per unit)\n- Below this point: loss. Above: profit.\n- Key for pricing and production decisions\n\n**Source**: Horngren, *Cost Accounting*".to_string()
        } else {
            format!("# Definition not found\n\nFinny doesn't have a definition for: \"{}\"\n\nTry asking about: duration, carry trade, CPI, yield, FX, GDP, CAGR, inflation, recession, mortgage, present value, P/E ratio, WACC, break-even.", q.trim())
        }
    }
}

fn regex_or(text: &str, pat: &str, default: f64) -> f64 {
    regex::Regex::new(pat).ok()
        .and_then(|r| r.captures(text))
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse::<f64>().ok())
        .unwrap_or(default)
}
fn ans(heading: &str, body: &str, src: &str) -> String {
    format!(
        "# Answer — {}\n\n{}\n\n## Observed fact\nApproximate value per latest authoritative release. Verify before transacting.\n\n## Source Profile\n- {}\n- Retrieved at: {} UTC\n\n## Reliability note\nFinny hard-codes approximate consensus figures for narrow query classes. Enable Network or consult the primary source for live central-bank decisions.",
        heading, body, src, chrono::Utc::now().format("%Y-%m-%d %H:%M")
    )
}
impl Default for QueryPlanner { fn default() -> Self { Self::new() } }

// Helper functions for Black-Scholes and other calculations
// Note: sqrt and ln are provided as helper functions for the Black-Scholes formula.
// In normal usage, prefer the built-in f64::sqrt() and f64::ln() methods.
#[allow(dead_code)]
fn sqrt(x: f64) -> f64 { x.sqrt() }
#[allow(dead_code)]
fn ln(x: f64) -> f64 { x.ln() }

fn normal_cdf(x: f64) -> f64 {
    const SQRT_2PI: f64 = 2.5066282746310002;
    let mut sum = 0.0;
    let mut term = x;
    for i in 1..=100 {
        sum += term;
        let n = (2 * i + 1) as f64;
        term = -term * x * x / n;
    }
    0.5 + sum / SQRT_2PI
}

/// Render an ASCII line chart from time-series data.
fn render_ascii_chart(data: &[(String, f64)], title: &str, unit: &str) -> String {
    let mut out = String::new();
    const CHART_WIDTH: usize = 50;
    const CHART_HEIGHT: usize = 12;

    if data.is_empty() {
        return "(no data)".to_string();
    }

    let min_val = data.iter().map(|(_, v)| *v).fold(f64::INFINITY, f64::min);
    let max_val = data
        .iter()
        .map(|(_, v)| *v)
        .fold(f64::NEG_INFINITY, f64::max);
    let range = (max_val - min_val).max(1e-10);
    let pad = (max_val.abs().max(1.0).log10().ceil() as usize) + 4;

    // Title + top border
    out.push_str("```\n");
    out.push_str(&format!("{}\n", title));
    out.push_str(&format!(
        "{:>w$} ┤\n",
        format!("{:.1}{}", max_val, unit),
        w = pad
    ));

    // Chart body — draw from top row to bottom row
    for row in (0..CHART_HEIGHT).rev() {
        let row_bottom = min_val + (range * row as f64) / CHART_HEIGHT as f64;
        let row_top = min_val + (range * (row + 1) as f64) / CHART_HEIGHT as f64;

        let label = if row == 0 || row == CHART_HEIGHT / 2 {
            format!("{:>w$.1}{}", row_bottom, unit, w = pad - 3)
        } else {
            " ".repeat(pad)
        };
        out.push_str(&format!("{} │", label));

        let mut line = vec![' '; CHART_WIDTH];
        for (i, (_, val)) in data.iter().enumerate() {
            let col = if data.len() > 1 {
                (i * (CHART_WIDTH - 1)) / (data.len() - 1)
            } else {
                0
            };
            if *val >= row_bottom && *val < row_top {
                line[col] = '●';
            } else if *val >= row_top && row < CHART_HEIGHT - 1 {
                line[col] = '│';
            }
        }
        out.push_str(&line.into_iter().collect::<String>());
        out.push('\n');
    }

    // X-axis line
    out.push_str(&format!(
        "{:>w$} └{:─<cw$}\n",
        format!("{:.1}{}", min_val, unit),
        "",
        w = pad,
        cw = CHART_WIDTH
    ));
    // X-axis year labels — space them across the chart width
        out.push_str(&format!("{:>pad$}   ", ""));
        let mut label_line = vec![' '; CHART_WIDTH];
        for (i, (year, _)) in data.iter().enumerate() {
            let col = if data.len() > 1 {
                (i * (CHART_WIDTH - 1)) / (data.len() - 1)
            } else {
                0
            };
            for (j, ch) in year.chars().enumerate() {
                let pos = col + j;
                if pos < CHART_WIDTH {
                    label_line[pos] = ch;
                }
            }
        }
        out.push_str(&label_line.into_iter().collect::<String>());
        out.push('\n');
        out.push_str("```\n");
        out
    }
