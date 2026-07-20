//! Input shapes for the claims registry (ADR-0004). `NewClaim` carries the
//! immutable `source_quote`; `UpdateClaim` deliberately does NOT — provenance
//! cannot be revised. `ClaimFilter` scopes reads by category, confidence, and
//! subject.

use chrono::NaiveDate;

use crate::entities::claim::{ClaimCategory, ClaimConfidence};

#[derive(Debug)]
pub struct NewClaim {
    pub category: ClaimCategory,
    pub statement: String,
    pub confidence: ClaimConfidence,
    /// Absent when the claim is about Steve; else the relative ("father").
    pub subject: Option<String>,
    /// Normalizable key for later rules to query, e.g. "colonoscopy".
    pub topic: Option<String>,
    pub occurred_on: Option<NaiveDate>,
    /// Verbatim words the claim was distilled from — immutable provenance.
    pub source_quote: Option<String>,
    pub concern_id: Option<i32>,
}

/// Partial update; outer `Option` = "field sent" (the `UpdateProfile` pattern),
/// inner = "set vs clear". No `source_quote` field — it is immutable by design.
#[derive(Debug, Default)]
pub struct UpdateClaim {
    pub category: Option<ClaimCategory>,
    pub statement: Option<String>,
    pub confidence: Option<ClaimConfidence>,
    pub subject: Option<Option<String>>,
    pub topic: Option<Option<String>>,
    pub occurred_on: Option<Option<NaiveDate>>,
    pub concern_id: Option<Option<i32>>,
}

#[derive(Debug, Default)]
pub struct ClaimFilter {
    pub category: Option<ClaimCategory>,
    pub confidence: Option<ClaimConfidence>,
    /// `None` = all subjects; `Some(None)` = self-only (`subject` IS NULL);
    /// `Some(Some(s))` = that relative.
    pub subject: Option<Option<String>>,
}

/// The literal "self" (trimmed, ASCII case-insensitive) is a RESERVED word on
/// the subject contract: claims about Steve store `subject` = NULL (ADR-0004
/// §2). This is the single definition both the MCP boundary and the domain
/// guard consume — the two layers can never disagree on what counts as
/// "self".
#[must_use]
pub fn is_self_sentinel(subject: &str) -> bool {
    subject.trim().eq_ignore_ascii_case("self")
}
