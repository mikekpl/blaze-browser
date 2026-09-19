//! WKContentRuleList JSON compilation (T022).
//!
//! WebKit enforces network blocking declaratively: filter lists are converted
//! to Apple content-blocker JSON and compiled by WKContentRuleListStore in the
//! shell. WebKit caps a rule list at 150 000 rules — lists are truncated with
//! a warning beyond that (the `classify_request` fallback still covers the
//! remainder via `decidePolicyFor`).

use adblock::lists::{FilterSet, ParseOptions};

/// WebKit's documented per-list rule cap.
const WEBKIT_MAX_RULES: usize = 150_000;

/// Subtitle/caption resources. No ad is ever served as one, but broad list
/// rules (CDN paths, `$third-party` hosts) can catch them, and a blocked
/// caption file fails silently — the viewer just gets no subtitles. These
/// close the rule list as `ignore-previous-rules`, so they always win.
/// (WebKit's rule regex has no alternation: one pattern per rule.)
const SUBTITLE_URL_FILTERS: &[&str] = &[
    r"\.vtt([?#].*)?$",
    r"\.webvtt([?#].*)?$",
    r"\.srt([?#].*)?$",
    r"\.ttml([?#].*)?$",
    r"\.dfxp([?#].*)?$",
    r"/api/timedtext\?",
];

fn subtitle_exceptions() -> impl Iterator<Item = serde_json::Value> {
    SUBTITLE_URL_FILTERS.iter().map(|filter| {
        serde_json::json!({
            "trigger": { "url-filter": filter },
            "action": { "type": "ignore-previous-rules" },
        })
    })
}

#[derive(Debug, thiserror::Error)]
pub enum WebkitRulesError {
    #[error("content-blocker conversion failed")]
    Conversion,
    #[error("JSON encoding failed: {0}")]
    Encode(#[from] serde_json::Error),
}

/// Compile raw filter-list texts into WKContentRuleList JSON.
///
/// Returns the JSON plus the number of source filters that could not be
/// expressed as content-blocker rules (enforced instead by the native
/// matcher via the policy hook).
pub fn compile_webkit_json<'a>(
    lists: impl IntoIterator<Item = &'a str>,
) -> Result<(String, usize), WebkitRulesError> {
    // Content-blocker conversion requires raw filter text retention (debug mode).
    let mut set = FilterSet::new(true);
    for text in lists {
        set.add_filter_list(text.to_string(), ParseOptions::default());
    }
    let (mut rules, unconvertible) = set
        .into_content_blocking()
        .map_err(|()| WebkitRulesError::Conversion)?;

    // Room for the closing exceptions below, which must survive truncation.
    let cap = WEBKIT_MAX_RULES - SUBTITLE_URL_FILTERS.len();
    if rules.len() > cap {
        tracing::warn!(
            total = rules.len(),
            cap,
            "truncating content-blocker rules to WebKit cap"
        );
        rules.truncate(cap - 1);
        // Keep first-party documents unblocked even after truncation.
        rules.push(adblock::content_blocking::ignore_previous_fp_documents());
    }

    let mut json = match serde_json::to_value(&rules)? {
        serde_json::Value::Array(values) => values,
        _ => Vec::new(),
    };
    if !json.is_empty() {
        json.extend(subtitle_exceptions());
    }
    Ok((serde_json::to_string(&json)?, unconvertible.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_simple_list_to_json() {
        let (json, _skipped) =
            compile_webkit_json(["||ads.example.com^\n##.banner-ad\n"]).expect("compiles");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let rules = parsed.as_array().expect("array of rules");
        assert!(!rules.is_empty());
        // Every rule must carry trigger + action per the content-blocker format.
        for rule in rules {
            assert!(rule.get("trigger").is_some());
            assert!(rule.get("action").is_some());
        }
    }

    #[test]
    fn subtitle_files_are_exempt_and_exemptions_come_last() {
        let (json, _) =
            compile_webkit_json(["||cdn.example.com^$third-party\n"]).expect("compiles");
        let rules: Vec<serde_json::Value> = serde_json::from_str(&json).expect("valid JSON");
        let tail = &rules[rules.len() - SUBTITLE_URL_FILTERS.len()..];
        for (rule, filter) in tail.iter().zip(SUBTITLE_URL_FILTERS) {
            assert_eq!(rule["action"]["type"], "ignore-previous-rules");
            assert_eq!(rule["trigger"]["url-filter"], *filter);
        }
        // the blocking rule is still there, ahead of them
        assert!(
            rules[..rules.len() - SUBTITLE_URL_FILTERS.len()]
                .iter()
                .any(|r| r["action"]["type"] == "block")
        );
    }

    #[test]
    fn empty_input_yields_empty_array() {
        let (json, skipped) = compile_webkit_json([]).expect("compiles");
        assert_eq!(json, "[]");
        assert_eq!(skipped, 0);
    }
}
