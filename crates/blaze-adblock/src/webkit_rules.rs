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

    if rules.len() > WEBKIT_MAX_RULES {
        tracing::warn!(
            total = rules.len(),
            cap = WEBKIT_MAX_RULES,
            "truncating content-blocker rules to WebKit cap"
        );
        rules.truncate(WEBKIT_MAX_RULES - 1);
        // Keep first-party documents unblocked even after truncation.
        rules.push(adblock::content_blocking::ignore_previous_fp_documents());
    }

    Ok((serde_json::to_string(&rules)?, unconvertible.len()))
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
    fn empty_input_yields_empty_array() {
        let (json, skipped) = compile_webkit_json([]).expect("compiles");
        assert_eq!(json, "[]");
        assert_eq!(skipped, 0);
    }
}
