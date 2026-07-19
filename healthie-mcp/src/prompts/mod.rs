//! MCP prompts — user-initiated templates MCP clients surface as slash-command
//! entries. Hosting the canonical checkin script here means every Claude
//! surface gets the same version as it improves. Layout per fewd: one file per
//! prompt for the body + tests; this module holds the thin `#[prompt]` wiring.
//! The generated `prompt_router()` is `pub(crate)` so the `#[prompt_handler]`
//! on `handler.rs`'s `ServerHandler` impl can reach it across modules.

pub mod baseline_intake;
pub mod checkin;

use rmcp::{
    ErrorData as McpError,
    handler::server::wrapper::Parameters,
    model::{GetPromptResult, PromptMessage, Role},
    prompt, prompt_router,
};

use crate::{
    handler::HealthieMcp,
    schemas::{BaselineIntakePromptArgs, CheckinPromptArgs},
};

#[prompt_router(vis = "pub(crate)")]
impl HealthieMcp {
    /// The scripted checkin opener; see [`checkin::render`] for the body.
    #[prompt(
        name = "checkin",
        description = "Run a health checkin: review what you committed to last time, talk through \
                       how things have actually been, capture observations, and commit the next \
                       plan. Cadence-agnostic — it covers everything since the last checkin. \
                       Optionally say what's on your mind to start there."
    )]
    // `&self` is required by `#[prompt_router]` for registration; the body is a
    // pure render over the args and never touches instance state.
    #[allow(clippy::unused_self)]
    async fn checkin(
        &self,
        Parameters(args): Parameters<CheckinPromptArgs>,
    ) -> Result<GetPromptResult, McpError> {
        let body = checkin::render(args.focus.as_deref());
        Ok(
            GetPromptResult::new(vec![PromptMessage::new_text(Role::User, body)])
                .with_description("Accountable health-coach checkin"),
        )
    }

    /// One sitting of the baseline intake; see [`baseline_intake::render`].
    #[prompt(
        name = "baseline_intake",
        description = "Run one sitting of the baseline health-history intake: pick the biggest \
                       coverage gaps (or steer with `area`), dig conversationally, and record \
                       claims with honest confidence and verbatim quotes. Resumable by design — \
                       the registry itself is the progress state."
    )]
    // `&self` is required by `#[prompt_router]` for registration; the body is a
    // pure render over the args and never touches instance state.
    #[allow(clippy::unused_self)]
    async fn baseline_intake(
        &self,
        Parameters(args): Parameters<BaselineIntakePromptArgs>,
    ) -> Result<GetPromptResult, McpError> {
        let body = baseline_intake::render(args.area.as_deref());
        Ok(
            GetPromptResult::new(vec![PromptMessage::new_text(Role::User, body)])
                .with_description("Baseline intake sitting"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Catches the prompt silently dropping out of prompts/list or `focus`
    /// flipping to required.
    #[test]
    fn checkin_is_registered_with_focus_optional() {
        let prompts = HealthieMcp::prompt_router().list_all();
        let prompt = prompts
            .iter()
            .find(|p| p.name == "checkin")
            .expect("checkin must be registered");
        let args = prompt
            .arguments
            .as_ref()
            .expect("checkin exposes arguments");
        let focus = args.iter().find(|a| a.name == "focus").expect("focus arg");
        assert_ne!(focus.required, Some(true), "focus must stay optional");
    }

    /// Catches the prompt silently dropping out of prompts/list or `area`
    /// flipping to required.
    #[test]
    fn baseline_intake_is_registered_with_area_optional() {
        let prompts = HealthieMcp::prompt_router().list_all();
        let prompt = prompts
            .iter()
            .find(|p| p.name == "baseline_intake")
            .expect("baseline_intake must be registered");
        let args = prompt
            .arguments
            .as_ref()
            .expect("baseline_intake exposes arguments");
        let area = args.iter().find(|a| a.name == "area").expect("area arg");
        assert_ne!(area.required, Some(true), "area must stay optional");
    }
}
