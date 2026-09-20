//! 统一错误类型 → HTTP 状态码
use actix_web::{http::StatusCode, HttpResponse, ResponseError};
use serde::Serialize;
use thiserror::Error;

/// 把任意 String 包装成 Error，让 thiserror `#[error]` 表达式能正常展开。
#[derive(Debug, Error)]
#[error("{0}")]
pub struct StrWrap(pub(crate) String);

impl From<String> for StrWrap {
    fn from(s: String) -> Self {
        StrWrap(s)
    }
}
impl From<&str> for StrWrap {
    fn from(s: &str) -> Self {
        StrWrap(s.to_string())
    }
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error("not found: {0}")]
    NotFound(StrWrap),

    #[error("bad request: {0}")]
    BadRequest(StrWrap),

    #[error("upstream failed after {tries} tries: {source}")]
    UpstreamExhausted { tries: u32, source: StrWrap },

    #[error("upstream parse error: {0}")]
    UpstreamParse(StrWrap),

    #[error("AI upstream error: {0}")]
    AiUpstream(StrWrap),

    #[error("too many requests: {0}")]
    TooManyRequests(StrWrap),

    #[error("service unavailable: {0}")]
    Unavailable(StrWrap),

    #[error("db error: {0}")]
    Db(#[from] sqlx::Error),

    #[error("internal: {0}")]
    Internal(StrWrap),
}

impl From<String> for AppError {
    fn from(s: String) -> Self {
        AppError::Internal(StrWrap(s))
    }
}
impl From<&str> for AppError {
    fn from(s: &str) -> Self {
        AppError::Internal(StrWrap(s.to_string()))
    }
}
impl From<StrWrap> for AppError {
    fn from(w: StrWrap) -> Self {
        AppError::Internal(w)
    }
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct AppErrorBody {
    /// 错误类型（machine-readable）
    pub error: String,
    /// 人类可读说明
    pub message: String,
}

#[derive(Serialize)]
struct ErrBodyOwned {
    error: String,
    message: String,
}

impl ResponseError for AppError {
    fn status_code(&self) -> StatusCode {
        match self {
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::UpstreamExhausted { .. } => StatusCode::BAD_GATEWAY,
            AppError::UpstreamParse(_) => StatusCode::BAD_GATEWAY,
            AppError::AiUpstream(_) => StatusCode::BAD_GATEWAY,
            AppError::TooManyRequests(_) => StatusCode::TOO_MANY_REQUESTS,
            AppError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            AppError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn error_response(&self) -> HttpResponse {
        let kind = match self {
            AppError::NotFound(_) => "not_found",
            AppError::BadRequest(_) => "bad_request",
            AppError::UpstreamExhausted { .. } => "upstream_exhausted",
            AppError::UpstreamParse(_) => "upstream_parse",
            AppError::AiUpstream(_) => "ai_upstream",
            AppError::TooManyRequests(_) => "too_many_requests",
            AppError::Unavailable(_) => "service_unavailable",
            AppError::Db(_) => "db_error",
            AppError::Internal(_) => "internal",
        };
        HttpResponse::build(self.status_code()).json(ErrBodyOwned {
            error: kind.into(),
            message: self.to_string(),
        })
    }
}

pub type AppResult<T> = Result<T, AppError>;