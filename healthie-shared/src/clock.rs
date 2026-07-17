use chrono::{DateTime, NaiveDate, Utc};

/// Current UTC instant, the canonical DB timestamp value.
pub fn now() -> DateTime<Utc> {
    Utc::now()
}

/// Today's UTC date.
pub fn today() -> NaiveDate {
    Utc::now().date_naive()
}
