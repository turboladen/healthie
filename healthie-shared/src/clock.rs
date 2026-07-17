/// UTC timestamp string, the canonical DB format.
pub fn now_str() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// UTC date string `YYYY-MM-DD`.
pub fn today_str() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}
