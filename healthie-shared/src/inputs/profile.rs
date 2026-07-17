/// Partial update; outer Option = "was the field sent", inner = "set vs clear".
#[derive(Debug, Default)]
pub struct UpdateProfile {
    pub date_of_birth: Option<Option<String>>,
    pub sex: Option<Option<String>>,
    pub height_cm: Option<Option<i32>>,
    pub notes: Option<Option<String>>,
}
