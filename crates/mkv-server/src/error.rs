use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use mkv_types::ApiError;

#[derive(Debug)]
pub struct AppError {
    status: StatusCode,
    body: ApiError,
}

impl AppError {
    pub fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            body: ApiError::new(code, message),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

impl From<mkv_core::CoreError> for AppError {
    fn from(e: mkv_core::CoreError) -> Self {
        AppError::internal(e.to_string())
    }
}

impl From<mkv_volume::VolumeError> for AppError {
    fn from(e: mkv_volume::VolumeError) -> Self {
        use mkv_volume::VolumeError;
        match &e {
            VolumeError::UnknownRoot(_)
            | VolumeError::PathEscape
            | VolumeError::AbsolutePath
            | VolumeError::InvalidComponent
            | VolumeError::NotAFile => AppError::bad_request(e.to_string()),
            VolumeError::Io { .. } => AppError::not_found(e.to_string()),
            VolumeError::InvalidConfig(_) => AppError::internal(e.to_string()),
        }
    }
}
