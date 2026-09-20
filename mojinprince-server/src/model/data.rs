use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SignalEventDto {
    pub id: String,
    pub at: DateTime<Utc>,
    pub kind: String,
    pub code: String,
    pub title: String,
    pub body: String,
    pub price: f64,
    pub source: String,
    pub evidence: String,
    pub why: String,
    #[schema(value_type = Object)]
    pub meta: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SignalFeedbackRequest {
    /// accept / ignore / partial / opened / sold
    pub action: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PositionDto {
    pub code: String,
    pub cost: f64,
    pub shares: f64,
    pub stop_loss: f64,
    pub take_profit: f64,
    pub position_pct: f64,
    pub note: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PositionUpdate {
    #[serde(default)]
    pub cost: f64,
    #[serde(default)]
    pub shares: f64,
    #[serde(default)]
    pub stop_loss: f64,
    #[serde(default)]
    pub take_profit: f64,
    #[serde(default)]
    pub position_pct: f64,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct WatchlistItem {
    pub code: String,
    pub name: String,
    pub market: String,
    pub pinned: bool,
    #[serde(rename = "group")]
    pub group_name: String,
    pub added_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WatchlistUpdate {
    pub code: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub market: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default, rename = "group")]
    pub group_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SettingsDocument {
    #[schema(value_type = Object)]
    pub values: serde_json::Value,
    pub updated_at: DateTime<Utc>,
}
