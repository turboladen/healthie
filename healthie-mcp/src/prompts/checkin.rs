//! The `checkin` prompt body — the scripted opener for the accountable
//! health-coach conversation. Cadence-agnostic: a checkin covers "since the
//! last checkin", whatever that gap is.

/// Render the checkin opener. `focus` is the user's optional "what's on my
/// mind" — woven in so the conversation starts where they are.
#[must_use]
pub fn render(focus: Option<&str>) -> String {
    let focus_block = match focus {
        Some(f) if !f.trim().is_empty() => {
            format!(
                "\n\nBefore anything else, one thing is on my mind today: {f}\nStart there, then \
                 fold the rest of the checkin around it."
            )
        }
        _ => String::new(),
    };
    format!(
        "Run my health checkin — it covers everything since the last one, however long that gap \
         is.{focus_block}\n\nWork through this conversationally, one thing at a time (short \
         questions, wait for my answers):\n\n1. ORIENT — call `get_briefing` first. Note the gap \
         since the last checkin; if the briefing flags a long gap, acknowledge it without \
         judgment.\n2. ACCOUNTABILITY — open with what I committed to last time: for each item in \
         the previous plan, ask what actually happened and record it with `record_plan_outcome` \
         (done / skipped / partial, with a note). Skipped items deserve a why, not a lecture.\n3. \
         CHECKIN — call `start_checkin` (it resumes an open one if we got cut off). Ask about \
         sleep, energy, pain, mood, and anything the briefing raised. Persist every meaningful \
         exchange AS IT HAPPENS with `record_checkin_response` — never batch at the end.\n4. \
         CAPTURE — anything notable that isn't a Q&A: `log_observation` / `log_symptom` (origin \
         `self` when I reported it). New recurring problems may deserve `open_concern`; overdue \
         protocol reviews (flagged in the briefing) should get a verdict via \
         `record_protocol_outcome` — check `get_protocol_history` before proposing anything we've \
         already tried.\n5. PLAN — propose next steps and negotiate them with me. Once I confirm, \
         `commit_plan`: workout items get a `scheduled_for` date, discrete actions are `action` \
         items, standing direction goes in `guidance`/`nutrition`. Then push time-bound items to \
         my calendar and actions to my task system if those tools are connected in this \
         conversation.\n6. CLOSE — `complete_checkin` with a summary written for the NEXT \
         checkin's step 2: what I committed to, state of play, open threads."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_scripts_the_full_loop_in_order() {
        let body = render(None);
        let order = [
            "get_briefing",
            "record_plan_outcome",
            "start_checkin",
            "record_checkin_response",
            "log_observation",
            "get_protocol_history",
            "commit_plan",
            "complete_checkin",
        ];
        crate::prompts::assert_scripts_in_order(&body, &order);
    }

    #[test]
    fn focus_is_woven_in_when_present() {
        let body = render(Some("my knee has been clicking"));
        assert!(body.contains("my knee has been clicking"));
        assert!(!render(None).contains("on my mind today"));
        assert!(
            !render(Some("   ")).contains("on my mind today"),
            "whitespace focus ignored"
        );
    }
}
