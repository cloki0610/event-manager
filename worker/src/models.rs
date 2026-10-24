use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: Option<DateTime<Utc>>,
    pub capacity: Option<i64>,
    pub attendee_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateEvent {
    pub title: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: Option<DateTime<Utc>>,
    pub capacity: Option<i64>,
}

/// Partial update. For nullable fields, an absent key leaves the value unchanged
/// while an explicit `null` clears it.
#[derive(Debug, Default, Deserialize)]
pub struct UpdateEvent {
    pub title: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    pub description: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub location: Option<Option<String>>,
    pub starts_at: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "double_option")]
    pub ends_at: Option<Option<DateTime<Utc>>>,
    #[serde(default, deserialize_with = "double_option")]
    pub capacity: Option<Option<i64>>,
}

fn double_option<'de, T, D>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(de).map(Some)
}

#[derive(Debug, Deserialize)]
pub struct ListParams {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    /// When true, only events that have not started yet are returned.
    #[serde(default)]
    pub upcoming: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Guest {
    pub id: String,
    pub name: String,
    /// Only present when the SSO provider shares it.
    pub email: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Returned by the SSO callback; the token cannot be retrieved again.
#[derive(Debug, Serialize, Deserialize)]
pub struct LoginResponse {
    pub guest: Guest,
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

/// Public view of an attendee (email is not exposed).
#[derive(Debug, Serialize, Deserialize)]
pub struct Attendee {
    pub guest_id: String,
    pub name: String,
    pub joined_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Attendance {
    pub event_id: String,
    pub guest_id: String,
    pub joined_at: DateTime<Utc>,
}
