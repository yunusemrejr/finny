// nlu.rs — Local, fast, NON-LLM natural-language understanding for Finny.
//
// This is "old-school ML + modern algorithmic math", not a language model:
//
//   1. A Multinomial Naive-Bayes intent/chitchat classifier trained at startup
//      on an embedded corpus of example phrases (incl. deliberately misspelled /
//      loosely-worded ones). Features are word unigrams + word bigrams + character
//      trigrams, so "inflaiton" still shares signal with "inflation". Training a
//      few hundred short examples takes milliseconds; inference is microseconds.
//
//   2. Dice-coefficient (character-bigram) fuzzy matching for typo-tolerant slot
//      extraction, so "bitcon price" / "turky inflation" still resolve.
//
// The well-tested rule-based classifier in `intent.rs` stays the authority for
// precise, keyword-heavy finance queries. This module adds what the rules cannot
// do: detect chitchat (greetings/thanks/help/…) and rescue loosely-worded or
// misspelled queries the rules would drop. `resolve()` fuses both into the
// existing `SlotBindings` type the rest of the engine already consumes.

use crate::intent::{ChitchatKind, IntentClass, IntentClassifier, SlotBindings};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Embedded training corpus
// ---------------------------------------------------------------------------
// Labels: the chitchat kinds, plus the finance intents (used only as a fallback
// signal when the rule classifier is unsure). Each row is one training example.
// Misspelled / colloquial variants are included on purpose so the model learns
// to cope with "written bad" English.

const CORPUS: &[(&str, &str)] = &[
    // ---- greeting ----
    ("hi", "greeting"), ("hello", "greeting"), ("hey", "greeting"),
    ("hi finny", "greeting"), ("hello finny", "greeting"), ("hey there", "greeting"),
    ("hey finny", "greeting"), ("good morning", "greeting"), ("good afternoon", "greeting"),
    ("good evening", "greeting"), ("morning", "greeting"), ("yo", "greeting"),
    ("hii", "greeting"), ("helo", "greeting"), ("heyy", "greeting"), ("heya", "greeting"),
    ("greetings", "greeting"), ("sup", "greeting"), ("hi there", "greeting"),
    ("howdy", "greeting"), ("hi finny how are you today", "greeting"),
    // ---- farewell ----
    ("bye", "farewell"), ("goodbye", "farewell"), ("see you", "farewell"),
    ("see ya", "farewell"), ("later", "farewell"), ("good night", "farewell"),
    ("gn", "farewell"), ("cya", "farewell"), ("farewell", "farewell"),
    ("take care", "farewell"), ("bye bye", "farewell"), ("gtg", "farewell"),
    ("got to go", "farewell"), ("talk to you later", "farewell"), ("im leaving", "farewell"),
    // ---- thanks ----
    ("thanks", "thanks"), ("thank you", "thanks"), ("thx", "thanks"), ("ty", "thanks"),
    ("cheers", "thanks"), ("appreciate it", "thanks"), ("thanks finny", "thanks"),
    ("thank u", "thanks"), ("thanx", "thanks"), ("much appreciated", "thanks"),
    ("thanks a lot", "thanks"), ("tyvm", "thanks"), ("thank you so much", "thanks"),
    // ---- howareyou ----
    ("how are you", "howareyou"), ("hows it going", "howareyou"), ("how r u", "howareyou"),
    ("how are you doing", "howareyou"), ("are you ok", "howareyou"), ("hru", "howareyou"),
    ("how you doing", "howareyou"), ("you good", "howareyou"), ("how do you do", "howareyou"),
    ("are you well", "howareyou"), ("hows everything", "howareyou"),
    // ---- capabilities (what can you do / help) ----
    ("what can you do", "capabilities"), ("what do you do", "capabilities"),
    ("what can u do", "capabilities"), ("what are your features", "capabilities"),
    ("how do i use this", "capabilities"), ("what can you help with", "capabilities"),
    ("capabilities", "capabilities"), ("what do you know", "capabilities"),
    ("help", "capabilities"), ("help me", "capabilities"), ("commands", "capabilities"),
    ("what can i ask", "capabilities"), ("how does this work", "capabilities"),
    ("what can i ask you", "capabilities"), ("show me what you can do", "capabilities"),
    ("what questions can i ask", "capabilities"), ("usage", "capabilities"),
    // ---- identity (who/what are you) ----
    ("who are you", "identity"), ("what are you", "identity"),
    ("whats your name", "identity"), ("who r u", "identity"), ("are you a robot", "identity"),
    ("are you ai", "identity"), ("are you real", "identity"), ("your name", "identity"),
    ("who made you", "identity"), ("what is finny", "identity"), ("who is finny", "identity"),
    ("are you a human", "identity"), ("are you an llm", "identity"),
    // ---- affirm ----
    ("yes", "affirm"), ("yeah", "affirm"), ("yep", "affirm"), ("sure", "affirm"),
    ("ok", "affirm"), ("okay", "affirm"), ("sounds good", "affirm"), ("great", "affirm"),
    ("cool", "affirm"), ("nice", "affirm"), ("awesome", "affirm"), ("perfect", "affirm"),
    ("yup", "affirm"), ("alright", "affirm"), ("absolutely", "affirm"),
    // ---- negate ----
    ("no", "negate"), ("nope", "negate"), ("nah", "negate"), ("not really", "negate"),
    ("nevermind", "negate"), ("cancel", "negate"), ("stop", "negate"), ("no thanks", "negate"),
    ("never mind", "negate"), ("dont", "negate"),
    // ---- smalltalk ----
    ("i am bored", "smalltalk"), ("tell me a joke", "smalltalk"), ("youre funny", "smalltalk"),
    ("lol", "smalltalk"), ("haha", "smalltalk"), ("i like you", "smalltalk"),
    ("you are cool", "smalltalk"), ("good bot", "smalltalk"), ("bad bot", "smalltalk"),
    ("im sad", "smalltalk"), ("im happy", "smalltalk"), ("are you bored", "smalltalk"),
    // ---- finance: direct lookup ----
    ("latest us cpi", "lookup"), ("whats the inflation rate in america", "lookup"),
    ("current fed funds rate", "lookup"), ("show me gdp for germany", "lookup"),
    ("unemployment in the uk", "lookup"), ("cpi turkey", "lookup"),
    ("whats interest rate in japan", "lookup"), ("policy rate ecb", "lookup"),
    ("how much is inflation in brazil", "lookup"), ("inflation in india", "lookup"),
    ("gdp growth china", "lookup"), ("what is the rate in the us", "lookup"),
    ("latest consumer price index usa", "lookup"),
    // ---- finance: comparison ----
    ("compare inflation us and eu", "comparison"), ("us vs uk rates", "comparison"),
    ("gold versus oil", "comparison"), ("compare gdp of china and india", "comparison"),
    ("which has higher inflation germany or france", "comparison"),
    ("compare policy rate across countries", "comparison"),
    // ---- finance: calculation ----
    ("calculate mortgage 300k 6 percent", "calculation"), ("convert 100 usd to eur", "calculation"),
    ("compute cagr from 1000 to 2000 over 5 years", "calculation"),
    ("bond duration d=7 25bp", "calculation"), ("how much is 1000 dollars in euros", "calculation"),
    ("sharpe ratio return 10 risk free 3 deviation 15", "calculation"),
    ("monthly payment 250k loan 5 percent 30 years", "calculation"),
    ("convert 50000 jpy to usd", "calculation"), ("what is 5 percent of 200", "calculation"),
    // ---- finance: causal ----
    ("what if rates go up", "causal"), ("how would inflation affect bonds", "causal"),
    ("effect of rate hike on stocks", "causal"), ("what happens if the dollar weakens", "causal"),
    ("impact of a recession on housing", "causal"),
    ("how could yen appreciation affect carry trades", "causal"),
    // ---- finance: definition ----
    ("what is inflation", "definition"), ("define cpi", "definition"),
    ("what does gdp mean", "definition"), ("explain duration", "definition"),
    ("meaning of yield", "definition"), ("what is a bond", "definition"),
    ("tell me about wacc", "definition"), ("what is the sharpe ratio", "definition"),
    ("define present value", "definition"),
    // ---- finance: asset price ----
    ("gold price", "assetprice"), ("how much is bitcoin", "assetprice"),
    ("oil price today", "assetprice"), ("silver spot price", "assetprice"),
    ("btc price", "assetprice"), ("crude oil cost", "assetprice"), ("gold spot", "assetprice"),
    // ---- finance: trend ----
    ("us cpi history", "trend"), ("inflation over time", "trend"),
    ("gold price trend", "trend"), ("policy rate over the last 5 years", "trend"),
    ("chart of gdp growth", "trend"), ("historical interest rates", "trend"),
    // ---- finance: forecast ----
    ("forecast us cpi", "forecast"), ("predict inflation next year", "forecast"),
    ("where is the fed funds rate going", "forecast"), ("outlook for gold", "forecast"),
    ("will rates rise next year", "forecast"), ("project gdp growth", "forecast"),
    // ---- finance: unsupported (refuse to hallucinate) ----
    ("exact volume of stop loss orders", "unsupported"),
    ("show me the hidden order book", "unsupported"),
    ("whats in my private portfolio", "unsupported"),
    ("dark pool volume for aapl", "unsupported"),
];

// ---------------------------------------------------------------------------
// Tokenization / feature extraction
// ---------------------------------------------------------------------------

/// Lowercase and keep only ascii alphanumerics + spaces.
fn normalize(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { ' ' })
        .collect()
}

/// Word tokens from normalized text.
fn words(norm: &str) -> Vec<&str> {
    norm.split_whitespace().collect()
}

/// Character trigrams of a single token (sub-word signal → typo robustness).
fn char_trigrams(token: &str) -> Vec<String> {
    let padded = format!("{}{}{}", '#', token, '#');
    let chars: Vec<char> = padded.chars().collect();
    let mut out = Vec::new();
    if chars.len() >= 3 {
        for w in chars.windows(3) {
            out.push(w.iter().collect());
        }
    }
    out
}

/// Full feature multiset for a phrase: word unigrams, word bigrams, and char
/// trigrams of each word. Returns a term-frequency map.
fn features(phrase: &str) -> HashMap<String, u32> {
    let norm = normalize(phrase);
    let w = words(&norm);
    let mut feats: HashMap<String, u32> = HashMap::new();
    for (i, tok) in w.iter().enumerate() {
        *feats.entry(format!("w={}", tok)).or_insert(0) += 1;
        if i + 1 < w.len() {
            *feats.entry(format!("b={} {}", tok, w[i + 1])).or_insert(0) += 1;
        }
        for tg in char_trigrams(tok) {
            *feats.entry(format!("t={}", tg)).or_insert(0) += 1;
        }
    }
    feats
}

// ---------------------------------------------------------------------------
// Multinomial Naive Bayes
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct NaiveBayes {
    labels: Vec<String>,
    /// log P(class)
    log_prior: HashMap<String, f64>,
    /// log P(feature | class), Laplace-smoothed
    log_likelihood: HashMap<(String, String), f64>,
    /// total feature tokens seen per class (for smoothing unseen features)
    class_tokens: HashMap<String, u64>,
    vocab: usize,
}

impl NaiveBayes {
    fn train(corpus: &[(&str, &str)]) -> Self {
        let mut class_feat_counts: HashMap<String, HashMap<String, u64>> = HashMap::new();
        let mut class_tokens: HashMap<String, u64> = HashMap::new();
        let mut class_docs: HashMap<String, u64> = HashMap::new();
        let mut vocab: std::collections::HashSet<String> = std::collections::HashSet::new();

        for (phrase, label) in corpus {
            *class_docs.entry(label.to_string()).or_insert(0) += 1;
            let counts = class_feat_counts.entry(label.to_string()).or_default();
            for (feat, n) in features(phrase) {
                *counts.entry(feat.clone()).or_insert(0) += n as u64;
                *class_tokens.entry(label.to_string()).or_insert(0) += n as u64;
                vocab.insert(feat);
            }
        }

        let total_docs: u64 = class_docs.values().sum();
        let vocab = vocab.len().max(1);
        let mut log_prior = HashMap::new();
        let mut log_likelihood: HashMap<(String, String), f64> = HashMap::new();

        let labels: Vec<String> = class_docs.keys().cloned().collect();
        for label in &labels {
            let docs = class_docs[label];
            log_prior.insert(label.clone(), ((docs as f64) / (total_docs as f64)).ln());
            let denom = *class_tokens.get(label).unwrap_or(&0) as f64 + vocab as f64;
            if let Some(counts) = class_feat_counts.get(label) {
                for (feat, n) in counts {
                    let p = (*n as f64 + 1.0) / denom; // Laplace smoothing
                    log_likelihood.insert((label.clone(), feat.clone()), p.ln());
                }
            }
        }

        Self { labels, log_prior, log_likelihood, class_tokens, vocab }
    }

    /// Smoothed log P(feature | class) for a possibly-unseen feature.
    fn feat_log_prob(&self, label: &str, feat: &str) -> f64 {
        if let Some(&lp) = self.log_likelihood.get(&(label.to_string(), feat.to_string())) {
            lp
        } else {
            let denom = *self.class_tokens.get(label).unwrap_or(&0) as f64 + self.vocab as f64;
            (1.0 / denom).ln()
        }
    }

    /// Posterior probability per label (softmax over log-scores), sorted desc.
    fn predict(&self, phrase: &str) -> Vec<(String, f64)> {
        let feats = features(phrase);
        let mut scores: Vec<(String, f64)> = self
            .labels
            .iter()
            .map(|label| {
                let mut s = *self.log_prior.get(label).unwrap_or(&f64::NEG_INFINITY);
                for (feat, n) in &feats {
                    s += self.feat_log_prob(label, feat) * (*n as f64);
                }
                (label.clone(), s)
            })
            .collect();

        // log-sum-exp softmax → probabilities
        let max = scores.iter().map(|(_, s)| *s).fold(f64::NEG_INFINITY, f64::max);
        let sum_exp: f64 = scores.iter().map(|(_, s)| (s - max).exp()).sum();
        let log_z = max + sum_exp.ln();
        for (_, s) in scores.iter_mut() {
            *s = (*s - log_z).exp();
        }
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scores
    }
}

// ---------------------------------------------------------------------------
// Fuzzy string similarity (Dice coefficient on character bigrams)
// ---------------------------------------------------------------------------

fn char_bigrams(s: &str) -> std::collections::HashSet<String> {
    let norm = normalize(s);
    let compact: String = norm.split_whitespace().collect();
    let chars: Vec<char> = compact.chars().collect();
    let mut set = std::collections::HashSet::new();
    if chars.len() < 2 {
        if chars.len() == 1 {
            set.insert(chars[0].to_string());
        }
        return set;
    }
    for w in chars.windows(2) {
        set.insert(w.iter().collect());
    }
    set
}

/// Sørensen–Dice coefficient in [0,1]; 1 = identical bigram multisets.
pub fn dice_similarity(a: &str, b: &str) -> f64 {
    let (ba, bb) = (char_bigrams(a), char_bigrams(b));
    if ba.is_empty() && bb.is_empty() {
        return 1.0;
    }
    let inter = ba.intersection(&bb).count() as f64;
    (2.0 * inter) / (ba.len() + bb.len()) as f64
}

/// Classic Levenshtein edit distance.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut curr = vec![0usize; m + 1];
    for i in 1..=n {
        curr[0] = i;
        for j in 1..=m {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[m]
}

/// Normalized Levenshtein similarity in [0,1]; robust for single-character typos
/// in short tokens (where bigram-Dice under-scores).
fn levenshtein_similarity(a: &str, b: &str) -> f64 {
    let (na, nb) = (normalize(a), normalize(b));
    let max = na.chars().count().max(nb.chars().count());
    if max == 0 {
        return 1.0;
    }
    1.0 - (levenshtein(&na, &nb) as f64 / max as f64)
}

/// Combined fuzzy score — best of bigram-Dice and normalized-Levenshtein.
fn fuzzy_score(a: &str, b: &str) -> f64 {
    dice_similarity(a, b).max(levenshtein_similarity(a, b))
}

/// Rescue typo'd / loosely-written queries: if exact alias matching found no
/// subject or geography, try fuzzy similarity between input tokens and the alias
/// tables (e.g. "inflation in turky" → geography = turkey). Purely additive — it
/// only fills gaps and never overrides an exact match.
fn fuzzy_fill(slots: &mut SlotBindings, lower: &str) {
    if slots.subject.is_none() {
        slots.subject = fuzzy_canonical(lower, &crate::intent::SUBJECT_ALIASES);
    }
    if slots.geography.is_none() {
        slots.geography = fuzzy_canonical(lower, &crate::intent::GEO_ALIASES);
    }
}

/// Find the canonical form whose alias best fuzzy-matches a token, above a
/// confidence floor. Short tokens/aliases (<3 chars) are skipped to avoid noise.
fn fuzzy_canonical(lower: &str, table: &[(&str, &[&str])]) -> Option<String> {
    const FLOOR: f64 = 0.75;
    let tokens: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| s.len() >= 3)
        .collect();
    let mut best: Option<(f64, String)> = None;
    for (canonical, aliases) in table {
        for alias in *aliases {
            if alias.len() < 3 {
                continue;
            }
            for tok in &tokens {
                let sim = fuzzy_score(tok, alias);
                let better = best.as_ref().map(|(b, _)| sim > *b).unwrap_or(true);
                if sim >= FLOOR && better {
                    best = Some((sim, canonical.to_string()));
                }
            }
        }
    }
    best.map(|(_, c)| c)
}

// ---------------------------------------------------------------------------
// Public NLU façade
// ---------------------------------------------------------------------------

/// Strong finance signals — if present, the message is NOT pure chitchat even if
/// the classifier leans that way (e.g. "hi, convert 100 usd to eur").
const FINANCE_SIGNALS: &[&str] = &[
    "cpi", "gdp", "inflation", "interest", "rate", "yield", "bond", "mortgage",
    "convert", "calculate", "compute", "compare", "forecast", "predict", "trend",
    "usd", "eur", "gbp", "jpy", "chf", "cad", "aud", "cny", "inr", "try",
    "gold", "oil", "bitcoin", "btc", "silver", "stock", "equity", "dividend",
    "sharpe", "wacc", "cagr", "duration", "roe", "unemployment", "price", "%",
    "define", "definition", "what is", "what's", "policy", "fed", "ecb",
];

#[derive(Clone)]
pub struct Nlu {
    model: NaiveBayes,
    rules: IntentClassifier,
}

/// Result of NLU analysis before fusion with the rule classifier.
pub struct Analysis {
    pub chitchat: Option<ChitchatKind>,
    pub top_label: String,
    pub top_prob: f64,
}

impl Nlu {
    pub fn new() -> Self {
        Self { model: NaiveBayes::train(CORPUS), rules: IntentClassifier::new() }
    }

    /// Run the classifier and decide whether this is pure chitchat.
    pub fn analyze(&self, text: &str) -> Analysis {
        let ranked = self.model.predict(text);
        let (top_label, top_prob) = ranked
            .first()
            .map(|(l, p)| (l.clone(), *p))
            .unwrap_or_else(|| ("lookup".to_string(), 0.0));

        let chitchat = self.detect_chitchat(text, &top_label, top_prob);
        Analysis { chitchat, top_label, top_prob }
    }

    /// Full resolution: chitchat → rule classifier → ML fallback → fuzzy slots.
    /// Returns the same `SlotBindings` the rest of the engine already consumes.
    pub fn resolve(&self, text: &str) -> SlotBindings {
        let analysis = self.analyze(text);

        // 1. Pure chitchat short-circuits everything.
        if let Some(kind) = analysis.chitchat {
            return SlotBindings {
                intent: IntentClass::Chitchat(kind),
                subject: None,
                metric: None,
                geography: None,
                date_window: None,
            };
        }

        // 2. Rule-based classifier (precise for keyword-heavy queries).
        let mut slots = self.rules.classify(text);

        // 3. If the rules only managed the vague fallback (DirectLookup with no
        //    subject AND no geography), let the ML classifier try to rescue it.
        let rules_vague = matches!(slots.intent, IntentClass::DirectLookup)
            && slots.subject.is_none()
            && slots.geography.is_none();
        if rules_vague && analysis.top_prob >= 0.45 {
            if let Some(intent) = ml_label_to_intent(&analysis.top_label) {
                slots.intent = intent;
            }
        }

        // 4. Fuzzy slot recovery for typo'd queries — purely additive (only
        //    fills subject/geography the exact alias match missed).
        fuzzy_fill(&mut slots, &text.to_lowercase());

        slots
    }

    /// Decide if the message is *pure* chitchat (a short social message with no
    /// finance content). Returns the chitchat kind, or None to take the finance path.
    fn detect_chitchat(&self, text: &str, top_label: &str, top_prob: f64) -> Option<ChitchatKind> {
        let kind = label_to_chitchat(top_label)?; // None ⇒ top label is a finance intent
        if top_prob < 0.55 {
            return None; // not confident enough
        }
        let norm = normalize(text);
        let n_words = words(&norm).len();
        // A pure social message is short and carries no finance signal.
        let has_finance = FINANCE_SIGNALS.iter().any(|s| norm.contains(s));
        if n_words <= 7 && !has_finance {
            Some(kind)
        } else {
            None
        }
    }
}

impl Default for Nlu {
    fn default() -> Self {
        Self::new()
    }
}

fn label_to_chitchat(label: &str) -> Option<ChitchatKind> {
    match label {
        "greeting" => Some(ChitchatKind::Greeting),
        "farewell" => Some(ChitchatKind::Farewell),
        "thanks" => Some(ChitchatKind::Thanks),
        "howareyou" => Some(ChitchatKind::HowAreYou),
        "capabilities" => Some(ChitchatKind::Capabilities),
        "identity" => Some(ChitchatKind::Identity),
        "affirm" => Some(ChitchatKind::Affirm),
        "negate" => Some(ChitchatKind::Negate),
        "smalltalk" => Some(ChitchatKind::Smalltalk),
        _ => None, // finance labels
    }
}

/// Map an ML finance label to an `IntentClass` (used only as a fallback signal).
fn ml_label_to_intent(label: &str) -> Option<IntentClass> {
    match label {
        "lookup" => Some(IntentClass::DirectLookup),
        "comparison" => Some(IntentClass::Comparison(Vec::new())),
        "calculation" => Some(IntentClass::Calculation),
        "causal" => Some(IntentClass::CausalScenario),
        "definition" => Some(IntentClass::Definition),
        "assetprice" => Some(IntentClass::AssetPrice),
        "trend" => Some(IntentClass::Trend),
        "forecast" => Some(IntentClass::Forecast),
        "unsupported" => Some(IntentClass::UnsupportedRequest),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn nlu() -> Nlu {
        Nlu::new()
    }

    // ---- chitchat detection (the thing the rules could never do) ----

    #[test]
    fn greetings_are_chitchat() {
        let n = nlu();
        for q in ["hi", "hello", "hey finny", "good morning", "hii", "helo"] {
            let s = n.resolve(q);
            assert!(
                matches!(s.intent, IntentClass::Chitchat(ChitchatKind::Greeting)),
                "{:?} -> {:?}", q, s.intent
            );
        }
    }

    #[test]
    fn thanks_farewell_help_identity() {
        let n = nlu();
        assert!(matches!(
            n.resolve("thanks finny").intent,
            IntentClass::Chitchat(ChitchatKind::Thanks)
        ));
        assert!(matches!(
            n.resolve("goodbye").intent,
            IntentClass::Chitchat(ChitchatKind::Farewell)
        ));
        assert!(matches!(
            n.resolve("what can you do").intent,
            IntentClass::Chitchat(ChitchatKind::Capabilities)
        ));
        assert!(matches!(
            n.resolve("who are you").intent,
            IntentClass::Chitchat(ChitchatKind::Identity)
        ));
    }

    #[test]
    fn greeting_with_finance_is_not_pure_chitchat() {
        let n = nlu();
        // "hi, convert 100 usd to eur" has a finance signal → not pure chitchat.
        let s = n.resolve("hi convert 100 usd to eur");
        assert!(
            !matches!(s.intent, IntentClass::Chitchat(_)),
            "got {:?}", s.intent
        );
    }

    // ---- fuzzy matching ----

    #[test]
    fn fuzzy_metrics_handle_typos() {
        // Dice alone scores well on insertion/deletion typos…
        assert!(dice_similarity("bitcoin", "bitcon") > 0.7);
        // …and low on unrelated terms.
        assert!(dice_similarity("gold", "gdp") < 0.5);
        // The combined production metric (max of Dice + normalized-Levenshtein)
        // also rescues transposition / short-word typos that Dice under-scores.
        assert!(fuzzy_score("inflation", "inflaiton") >= 0.75);
        assert!(fuzzy_score("turkey", "turky") >= 0.75);
        assert!(fuzzy_score("gold", "gdp") < 0.5);
    }

    #[test]
    fn fuzzy_slot_recovery_rescues_typos() {
        let n = nlu();
        // Only the geography is typo'd; exact subject match still works.
        let s = n.resolve("inflation in turky");
        assert_eq!(s.geography.as_deref(), Some("turkey"), "geo {:?}", s.geography);
        // A typo'd asset token is recovered fuzzily.
        let s2 = n.resolve("bitcon price");
        assert_eq!(s2.subject.as_deref(), Some("bitcoin"), "subject {:?}", s2.subject);
    }

    // ---- finance queries still classify (rules + ML agree) ----

    #[test]
    fn finance_queries_still_work() {
        let n = nlu();
        assert!(matches!(n.resolve("latest US CPI").intent, IntentClass::DirectLookup));
        assert!(matches!(n.resolve("convert 100 USD to EUR").intent, IntentClass::Calculation));
        assert!(matches!(
            n.resolve("compare policy rate across US and EU").intent,
            IntentClass::Comparison(_)
        ));
        assert!(matches!(n.resolve("what is inflation").intent, IntentClass::Definition));
    }

    #[test]
    fn model_trains_and_predicts_something_sane() {
        let n = nlu();
        let ranked = n.model.predict("hello there");
        assert_eq!(ranked[0].0, "greeting");
        assert!(ranked[0].1 > 0.5);
    }
}
