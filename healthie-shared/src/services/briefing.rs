use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use sea_orm::ConnectionTrait;
use serde::Serialize;

use crate::{
    entities::{checkin_response, claim, goal, observation, profile, protocol},
    error::DomainResult,
    services::{
        checkin, claim as claim_svc, concern, goal as goal_svc, observation as observation_svc,
        plan, profile as profile_svc, protocol as protocol_svc,
    },
};

#[derive(Debug, Serialize)]
pub struct LastCheckin {
    pub summary: Option<String>,
    pub completed_at: DateTime<Utc>,
    pub responses: Vec<checkin_response::Model>,
}

#[derive(Debug, Serialize)]
pub struct ProtocolBrief {
    pub protocol: protocol::Model,
    pub overdue_review: bool,
}

#[derive(Debug, Serialize)]
pub struct Briefing {
    pub generated_on: NaiveDate,
    pub profile: Option<profile::Model>,
    pub days_since_last_checkin: Option<i64>,
    pub cadence_note: Option<String>,
    pub last_checkin: Option<LastCheckin>,
    pub previous_plan: Option<plan::PlanWithItems>,
    pub active_concerns: Vec<concern::ConcernWithTags>,
    pub active_goals: Vec<goal::Model>,
    pub active_protocols: Vec<ProtocolBrief>,
    pub observations_pending_review: Vec<observation::Model>,
    pub recent_observations: Vec<observation::Model>,
    /// Claims with `unknown` confidence — tasks to resolve, surfaced as
    /// available context, never a nag (ADR-0004).
    pub claims_needing_resolution: Vec<claim::Model>,
}

/// Assembles the full daily briefing as of `today`.
///
/// # Errors
/// `DomainError::Db` on database failure in any of the underlying queries.
pub async fn assemble(db: &impl ConnectionTrait, today: NaiveDate) -> DomainResult<Briefing> {
    let last = checkin::latest_completed(db).await?;
    let (last_checkin, days_since, since_window) = if let Some((ck, responses)) = last {
        let completed_at = ck.completed_at.unwrap_or(ck.started_at);
        let days = (today - completed_at.date_naive()).num_days();
        (
            Some(LastCheckin {
                summary: ck.summary,
                completed_at,
                responses,
            }),
            Some(days),
            completed_at,
        )
    } else {
        let two_weeks_ago = (today - chrono::Duration::days(14))
            .and_time(NaiveTime::MIN)
            .and_utc();
        (None, None, two_weeks_ago)
    };

    let cadence_note = days_since.filter(|d| *d > 10).map(|d| {
        format!("Last checkin was {d} days ago — widen your questions to cover the whole gap.")
    });

    let active_protocols = protocol_svc::list_active(db)
        .await?
        .into_iter()
        .map(|p| {
            let overdue_review = p.review_by.is_some_and(|r| r < today);
            ProtocolBrief {
                protocol: p,
                overdue_review,
            }
        })
        .collect();

    Ok(Briefing {
        generated_on: today,
        profile: profile_svc::get(db).await?,
        days_since_last_checkin: days_since,
        cadence_note,
        last_checkin,
        previous_plan: plan::latest(db).await?,
        active_concerns: concern::list_active(db).await?,
        active_goals: goal_svc::list_active(db).await?,
        active_protocols,
        observations_pending_review: observation_svc::pending_review(db).await?,
        recent_observations: observation_svc::recent(db, since_window).await?,
        claims_needing_resolution: claim_svc::unresolved(db).await?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        entities::{
            concern_tag::ConcernTag,
            observation::{ObservationKind, ObservationOrigin},
            plan_item::PlanItemKind,
            protocol::ProtocolKind,
        },
        inputs::{
            concern::NewConcern,
            observation::NewObservation,
            plan::{NewPlan, NewPlanItem},
            protocol::NewProtocol,
        },
        services::{checkin, concern, observation, plan, protocol},
        test_support::{date, test_db},
    };

    #[tokio::test]
    async fn first_ever_briefing_is_empty_but_valid() {
        let db = test_db().await;
        let b = assemble(&db, date("2026-07-16")).await.unwrap();
        assert!(b.days_since_last_checkin.is_none());
        assert!(b.previous_plan.is_none());
        assert!(b.active_concerns.is_empty());
        // must serialize — it crosses the MCP boundary in M1b
        serde_json::to_string(&b).unwrap();
    }

    #[tokio::test]
    async fn briefing_assembles_full_picture() {
        let db = test_db().await;
        let c = concern::open(
            &db,
            NewConcern {
                name: "Bad back".into(),
                narrative: None,
                tags: vec![ConcernTag::Musculoskeletal],
                opened_on: None,
            },
        )
        .await
        .unwrap();
        protocol::start(
            &db,
            NewProtocol {
                concern_id: Some(c.concern.id),
                goal_id: None,
                name: "Magnesium".into(),
                kind: ProtocolKind::Supplement,
                purpose: None,
                schedule: None,
                started_on: None,
                review_by: Some(date("2026-07-01")), // overdue vs 2026-07-16
            },
        )
        .await
        .unwrap();
        observation::log(
            &db,
            NewObservation {
                origin: ObservationOrigin::Ai,
                kind: ObservationKind::Note,
                body: "HR trending up".into(),
                severity: None,
                concern_id: None,
                occurred_at: None,
            },
        )
        .await
        .unwrap();
        let ck = checkin::start(&db).await.unwrap();
        checkin::record_response(&db, ck.id, "Week?", "Fine.", None)
            .await
            .unwrap();
        checkin::complete(&db, ck.id, "Fine week.").await.unwrap();
        plan::commit(
            &db,
            NewPlan {
                checkin_id: Some(ck.id),
                starts_on: None,
                horizon_days: None,
                guidance: None,
                nutrition: None,
                items: vec![NewPlanItem {
                    kind: PlanItemKind::Workout,
                    title: "PT bird-dogs".into(),
                    detail: None,
                    scheduled_for: None,
                }],
            },
        )
        .await
        .unwrap();

        let b = assemble(&db, date("2026-07-16")).await.unwrap();
        assert_eq!(b.active_concerns.len(), 1);
        assert_eq!(b.active_concerns[0].tags, vec![ConcernTag::Musculoskeletal]);
        assert!(b.active_protocols[0].overdue_review);
        assert_eq!(b.observations_pending_review.len(), 1);
        assert!(b.previous_plan.is_some());
        assert!(b.last_checkin.is_some());
        assert!(b.days_since_last_checkin.is_some());
    }

    #[tokio::test]
    async fn briefing_lists_unknown_claims_for_resolution() {
        use crate::{
            entities::claim::{ClaimCategory, ClaimConfidence},
            inputs::claim::NewClaim,
            services::claim,
        };

        let db = test_db().await;
        claim::record_batch(
            &db,
            vec![
                NewClaim {
                    category: ClaimCategory::Screening,
                    statement: "Last tetanus shot: no idea".to_owned(),
                    confidence: ClaimConfidence::Unknown,
                    subject: None,
                    topic: Some("tetanus".to_owned()),
                    occurred_on: None,
                    source_quote: None,
                    concern_id: None,
                },
                NewClaim {
                    category: ClaimCategory::Surgery,
                    statement: "Back surgery 2014, L4-L5 microdiscectomy".to_owned(),
                    confidence: ClaimConfidence::Verified,
                    subject: None,
                    topic: Some("back-surgery".to_owned()),
                    occurred_on: None,
                    source_quote: None,
                    concern_id: None,
                },
            ],
        )
        .await
        .expect("seed claims");

        let briefing = assemble(&db, date("2026-07-18")).await.expect("assemble");
        assert_eq!(briefing.claims_needing_resolution.len(), 1);
        assert_eq!(
            briefing.claims_needing_resolution[0].confidence,
            ClaimConfidence::Unknown
        );
    }

    #[tokio::test]
    async fn long_gap_sets_cadence_note() {
        let db = test_db().await;
        let ck = checkin::start(&db).await.unwrap();
        checkin::record_response(&db, ck.id, "Week?", "ok", None)
            .await
            .unwrap();
        checkin::complete(&db, ck.id, "ok").await.unwrap();
        // completed today; a briefing dated 30 days out sees a 30-day gap
        let future = (chrono::Utc::now() + chrono::Duration::days(30)).date_naive();
        let b = assemble(&db, future).await.unwrap();
        assert!(b.cadence_note.is_some());
    }
}
