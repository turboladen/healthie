//! The `baseline_intake` prompt — scripts ONE SITTING of the baseline, not a
//! marathon. The baseline is a state of completeness (coverage derived from
//! the claims registry), deepened across many conversations.

/// Render the intake-sitting opener. `area` optionally steers the sitting to
/// a specific system area.
#[must_use]
pub fn render(area: Option<&str>) -> String {
    let steer = match area {
        Some(a) if !a.trim().is_empty() => {
            format!("\n\nFor this sitting, focus on: {a}.")
        }
        _ => String::new(),
    };
    format!(
        "Run a baseline-intake sitting — we are building the durable record of my health history, \
         one area at a time.{steer}\n\nGround rules: record what I CLAIM with honest confidence \
         (verified / recalled / unknown / not-done) — never launder my memory into fact. \
         `unknown` is a task for me to resolve later, never something to nag about. Capture my \
         verbatim words as source_quote so an off-hand remark never quietly becomes canon.\n\nThe \
         sitting:\n1. ORIENT — call `run_baseline_intake` for the coverage map and `get_briefing` \
         for current context. Unless I steered you above, pick 1-2 areas with the biggest gaps \
         (zero-claim categories first). The full systems checklist: sleep (including \
         breathing/apnea signals), cardiovascular + family history, metabolic/labs, \
         musculoskeletal + injuries, mental health, screenings, medications/supplements, \
         allergies, lifestyle (diet, alcohol, tobacco, activity), surgeries/hospitalizations.\n2. \
         DIG — work the chosen area conversationally, one question at a time. Where health data \
         or prior claims exist, START from them (`get_claims`, the briefing) and probe anomalies \
         — do not re-ask what is already on file.\n3. READ BACK — before recording, restate each \
         claim in plain words with its confidence and let me correct the calibration. Then \
         `record_intake_answers` with source_quote on everything you can.\n4. RESOLVE — if I can \
         check something during the conversation (records, dates), upgrade it with \
         `update_claim`; otherwise leave it `unknown`.\n5. CLOSE — call `run_baseline_intake` \
         again and tell me the coverage delta, what is still untouched, and which area you \
         suggest next sitting."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_scripts_the_sitting_in_order() {
        let body = render(None);
        let order = [
            "run_baseline_intake",
            "get_briefing",
            "get_claims",
            "record_intake_answers",
            "update_claim",
            "run_baseline_intake",
        ];
        crate::prompts::assert_scripts_in_order(&body, &order);
        for marker in [
            "verified",
            "recalled",
            "unknown",
            "not-done",
            "source_quote",
            "READ BACK",
        ] {
            assert!(body.contains(marker), "body must mention {marker}");
        }
    }

    /// Ties the hand-written confidence prose to the `ClaimConfidence` enum —
    /// a vocabulary change updates the schemars schema automatically but
    /// would otherwise leave this coaching prose stale and untested.
    #[test]
    fn body_covers_every_confidence_value() {
        use healthie_shared::entities::claim::ClaimConfidence;
        use sea_orm::strum::IntoEnumIterator;
        let body = render(None);
        for confidence in ClaimConfidence::iter() {
            let wire = serde_json::to_value(confidence).expect("serialize variant");
            let wire = wire.as_str().expect("wire value is a string");
            assert!(
                body.contains(wire),
                "prompt body must mention confidence value {wire}"
            );
        }
    }

    #[test]
    fn area_steering_is_woven_in_when_present() {
        assert!(render(Some("sleep and breathing")).contains("sleep and breathing"));
        assert!(!render(None).contains("focus on:"));
        assert!(
            !render(Some("  ")).contains("focus on:"),
            "whitespace area ignored"
        );
    }
}
