use chrono::{DateTime, NaiveDate, Utc};

/// Current UTC instant, the canonical DB timestamp value.
#[must_use]
pub fn now() -> DateTime<Utc> {
    Utc::now()
}

/// Today's UTC date.
#[must_use]
pub fn today() -> NaiveDate {
    Utc::now().date_naive()
}
