// dialogue.rs — Conversational response synthesis (natural-language generation)
// for Finny. This is template-based NLG with light variation — NOT a language
// model. It does two jobs:
//
//   1. `chitchat_reply` — warm, varied answers to social messages (greetings,
//      thanks, help, identity, …) that the finance engine doesn't handle.
//
//   2. `wrap` — turn the deterministic engine's rigid multi-section report into
//      something that reads like a normal chat reply: a friendly lead-in, the
//      substantive content (formulas/tables/charts kept intact), the repetitive
//      "Observed fact / Source Profile / Reliability note" boilerplate removed,
//      and a single concise source + reliability footer appended. Provenance is
//      preserved (the source name survives in the footer).

use crate::intent::{ChitchatKind, IntentClass, SlotBindings};

// ---------------------------------------------------------------------------
// Small deterministic variation helper
// ---------------------------------------------------------------------------

/// Stable per-input index into a pool of phrasings (so the same question gets a
/// consistent reply, but different questions vary). Cheap FNV-1a hash.
fn pick<'a>(input: &str, pool: &'a [&'a str]) -> &'a str {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in input.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    pool[(h as usize) % pool.len()]
}

// ---------------------------------------------------------------------------
// Chitchat replies
// ---------------------------------------------------------------------------

pub fn chitchat_reply(kind: ChitchatKind, input: &str) -> String {
    match kind {
        ChitchatKind::Greeting => {
            let open = pick(
                input,
                &[
                    "Hey there! 👋 I'm Finny, your local finance assistant.",
                    "Hi! Finny here — ready when you are.",
                    "Hello! Good to see you. I'm Finny.",
                ],
            );
            format!(
                "{}\n\nAsk me things like:\n- `latest US CPI` or `inflation in Turkey`\n- `convert 100 USD to EUR`\n- `compare policy rate across US, UK and Japan`\n- `what is duration` or `calculate mortgage 300k at 6.5%`\n\nWhat would you like to know?",
                open
            )
        }
        ChitchatKind::Farewell => pick(
            input,
            &[
                "Goodbye! Come back whenever you want to crunch some numbers. 👋",
                "See you later! I'll be right here.",
                "Take care! Happy to help anytime.",
            ],
        )
        .to_string(),
        ChitchatKind::Thanks => pick(
            input,
            &[
                "You're very welcome! 😊 Anything else you'd like me to look up?",
                "Happy to help! Ask me anything else.",
                "Anytime! That's what I'm here for.",
            ],
        )
        .to_string(),
        ChitchatKind::HowAreYou => pick(
            input,
            &[
                "I'm running smoothly — thanks for asking! 😄 More importantly, how can I help with your finances today?",
                "Doing great, all systems nominal! What can I look up for you?",
                "I'm good! Ready to dig into rates, inflation, FX — whatever you need.",
            ],
        )
        .to_string(),
        ChitchatKind::Capabilities => {
            "Here's what I can do for you — all locally, no cloud, no LLM:\n\n\
             **Look up** economic data\n- `latest US CPI`, `inflation in Turkey`, `GDP growth in China`\n\n\
             **Compare** across countries or assets\n- `compare policy rate across US, UK, Japan` · `compare gold and oil`\n\n\
             **Calculate** (18 built-in calculators)\n- `convert 100 USD to EUR` · `calculate mortgage 300k at 6.5%` · `CAGR from 1000 to 2000 over 5 years` · `bond duration D=7 and 25bp`\n\n\
             **Explain** concepts\n- `what is duration` · `define WACC` · `explain inflation`\n\n\
             **Trends & forecasts**\n- `US CPI over the last 5 years` · `forecast inflation next year`\n\n\
             Turn on **Live network** (sidebar) and I'll also pull real-time figures from the World Bank, BLS and central-bank pages. Just type a question!"
                .to_string()
        }
        ChitchatKind::Identity => {
            "I'm **Finny** — a local-first personal finance assistant that runs entirely on your computer.\n\n\
             - **Native desktop app** (Rust), not a website.\n- **No LLM, no cloud, no telemetry** — your chats stay in a local database.\n- I use fast, classic machine learning to understand your questions, plus deterministic math for the answers.\n\n\
             Ask me about inflation, interest rates, GDP, FX, bonds, and more — or type `what can you do` for examples."
                .to_string()
        }
        ChitchatKind::Affirm => pick(
            input,
            &["Great! What shall we look at next?", "Awesome — go ahead, ask me anything.", "Perfect. I'm ready when you are."],
        )
        .to_string(),
        ChitchatKind::Negate => pick(
            input,
            &["No problem! Let me know if you change your mind.", "Sure thing. I'm here if you need anything.", "Okay — just say the word when you're ready."],
        )
        .to_string(),
        ChitchatKind::Smalltalk => pick(
            input,
            &[
                "😄 I'm better at numbers than jokes, but I'm happy to chat! Want to look up some financial data?",
                "Haha — you're kind! Shall we dive into some rates or inflation figures?",
                "I appreciate that! My favourite topic is finance, naturally — ask me anything about it.",
            ],
        )
        .to_string(),
    }
}

// ---------------------------------------------------------------------------
// Wrapping deterministic answers conversationally
// ---------------------------------------------------------------------------

/// Section headings that are repetitive boilerplate — stripped and replaced by a
/// single concise footer. (Useful sections like "## Inputs", "## Result",
/// "## Note", "## Interpretation", "## Formula" are kept.)
const BOILERPLATE_HEADINGS: &[&str] = &["## Observed fact", "## Source Profile", "## Reliability note"];

/// Remove the boilerplate sections, returning the cleaned text and the source
/// name (first bullet under "## Source Profile"), if any.
fn strip_boilerplate(answer: &str) -> (String, Option<String>) {
    let mut out: Vec<&str> = Vec::new();
    let mut source: Option<String> = None;
    let mut skipping = false;
    let mut in_source_block = false;

    for line in answer.lines() {
        let trimmed = line.trim();
        let is_heading = trimmed.starts_with("## ") || (trimmed.starts_with("# ") && !trimmed.starts_with("## "));

        if is_heading {
            if BOILERPLATE_HEADINGS.iter().any(|h| trimmed == *h) {
                skipping = true;
                in_source_block = trimmed == "## Source Profile";
                continue;
            }
            // A new, non-boilerplate heading ends the skip.
            skipping = false;
            in_source_block = false;
        }

        if skipping {
            if in_source_block && source.is_none() && trimmed.starts_with("- ") {
                let s = trimmed.trim_start_matches("- ").trim();
                // Skip the "Retrieved at: …" line — we only want the source name.
                if !s.to_lowercase().starts_with("retrieved at") {
                    source = Some(s.to_string());
                }
            }
            continue;
        }
        out.push(line);
    }

    // Trim trailing blank lines left behind by the removal.
    while out.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
        out.pop();
    }
    (out.join("\n"), source)
}

/// True when the engine produced the broken "not in cache" placeholder answer
/// (e.g. `# Answer — subject ((region))`). We replace that with a friendly
/// clarification instead of showing literal placeholders.
fn is_dead_end(answer: &str) -> bool {
    answer.contains("Not in local cache")
        || answer.contains("((region))")
        || answer.contains("# Answer — subject")
}

fn lead_in(slots: &SlotBindings, input: &str) -> String {
    let subj = slots.subject.as_deref().unwrap_or("");
    let geo = slots.geography.as_deref().unwrap_or("");
    match &slots.intent {
        IntentClass::DirectLookup => {
            if !subj.is_empty() && !geo.is_empty() {
                pick(input, &["Sure — here's what I have:", "Here you go:", "Got it. Here's the latest I can show you:"])
                    .to_string()
            } else {
                "Here's what I found:".to_string()
            }
        }
        IntentClass::Comparison(_) => "Let's line them up side by side:".to_string(),
        IntentClass::Calculation => pick(
            input,
            &["Here you go — I worked it out step by step:", "Done! Here's the full breakdown:", "Sure — here's the calculation:"],
        )
        .to_string(),
        IntentClass::Definition => pick(
            input,
            &["Good question. Here's a clear explanation:", "Happy to explain — here's the definition:", "Sure, here's what that means:"],
        )
        .to_string(),
        IntentClass::CausalScenario => "Great scenario to think through. Here's a bounded, reasoned take:".to_string(),
        IntentClass::Trend => "Here's the trend I can plot for you:".to_string(),
        IntentClass::Forecast => {
            "Here's a mechanical extrapolation from historical data — treat it as a rough guide, not a prediction:".to_string()
        }
        IntentClass::AssetPrice => "Here's the recent price picture:".to_string(),
        IntentClass::UnsupportedRequest => String::new(),
        IntentClass::Chitchat(_) => String::new(),
    }
}

/// Build the final conversational reply from the deterministic engine answer.
pub fn wrap(slots: &SlotBindings, answer: &str, input: &str) -> String {
    // Dead-end placeholder answer → friendly clarification with suggestions.
    if is_dead_end(answer) {
        return clarification(slots);
    }

    let (cleaned, source) = strip_boilerplate(answer);
    let lead = lead_in(slots, input);

    // If live data was actually fetched, say so; otherwise point to Live network.
    let has_live = cleaned.contains("## Live Data") || cleaned.contains("## Live Excerpts");
    let footer = if has_live {
        "\n\n_Live figures fetched just now from the sources listed above — provenance saved._".to_string()
    } else {
        match &source {
            Some(s) => format!(
                "\n\n_Source: {} · figures are approximate — enable **Live network** for real-time data._",
                s
            ),
            None => "\n\n_Figures are approximate reference values — enable **Live network** for live data._".to_string(),
        }
    };

    let mut out = String::new();
    if !lead.is_empty() {
        out.push_str(&lead);
        out.push_str("\n\n");
    }
    out.push_str(cleaned.trim());
    out.push_str(&footer);
    out
}

/// Friendly response when the engine couldn't pin down the query.
fn clarification(slots: &SlotBindings) -> String {
    let mut s = String::from(
        "I'm not quite sure what to look up there — could you rephrase? A few things that work well:\n\n\
         - `latest US CPI` or `inflation in Turkey`\n\
         - `compare policy rate across US, UK and Japan`\n\
         - `convert 100 USD to EUR`\n\
         - `what is duration` · `calculate mortgage 300k at 6.5%`\n\n\
         Tip: naming a **metric** (CPI, rates, GDP…) and a **country** helps me a lot.",
    );
    if slots.geography.is_some() || slots.subject.is_some() {
        s.push_str("\n\n(If you turn on **Live network** in the sidebar, I can also fetch the latest figures directly.)");
    }
    s
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn slots(intent: IntentClass) -> SlotBindings {
        SlotBindings {
            intent,
            subject: Some("cpi".into()),
            metric: None,
            geography: Some("us".into()),
            date_window: None,
        }
    }

    #[test]
    fn strips_boilerplate_but_keeps_content() {
        let raw = "# Answer — CPI (US)\n\nLatest ≈ 3.0%.\n\n## Observed fact\nblah\n\n## Source Profile\n- BLS\n- Retrieved at: 2025 UTC\n\n## Reliability note\nFinny hard-codes approximate figures.";
        let (cleaned, source) = strip_boilerplate(raw);
        assert!(cleaned.contains("CPI (US)"));
        assert!(cleaned.contains("3.0%"));
        assert!(!cleaned.contains("Observed fact"));
        assert!(!cleaned.contains("Reliability note"));
        assert!(!cleaned.contains("Retrieved at"));
        assert_eq!(source.as_deref(), Some("BLS"));
    }

    #[test]
    fn wrap_adds_lead_in_and_footer() {
        let raw = "# Answer — CPI (US)\n\nLatest ≈ 3.0%.\n\n## Source Profile\n- BLS\n- Retrieved at: x UTC\n\n## Reliability note\nFinny hard-codes approximate figures.";
        let out = wrap(&slots(IntentClass::DirectLookup), raw, "latest us cpi");
        assert!(out.contains("3.0%"));
        assert!(out.to_lowercase().contains("source: bls"));
        assert!(!out.contains("Reliability note"));
    }

    #[test]
    fn dead_end_becomes_clarification() {
        let raw = "# Answer — subject ((region))\n\nNot in local cache. Try with network enabled.";
        let out = wrap(&slots(IntentClass::DirectLookup), raw, "asdf");
        assert!(out.contains("rephrase"));
        assert!(!out.contains("((region))"));
    }

    #[test]
    fn chitchat_greeting_is_friendly() {
        let out = chitchat_reply(ChitchatKind::Greeting, "hi");
        assert!(out.to_lowercase().contains("finny"));
        assert!(out.contains("latest US CPI"));
    }
}
