use std::{error::Report, path::PathBuf, string::FromUtf8Error};

use axum::{http::StatusCode, response::IntoResponse, Json};

#[allow(dead_code)]
#[derive(thiserror::Error, Debug)]
pub enum Error {
  #[error("Static file not found: {path}")]
  StaticNotFound { path: PathBuf },
  #[error(transparent)]
  Git(#[from] git2::Error),
  #[error(transparent)]
  Io(#[from] tokio::io::Error),
  #[error(transparent)]
  Utf8(#[from] FromUtf8Error),
  #[error(transparent)]
  PandocError(#[from] pandoc::PandocError),
  #[error("\"{0}\" isn't a valid `pandoc` input format")]
  PandocInvalidInputFormat(String),
  #[error(transparent)]
  TeraError(#[from] tera::Error),
  #[error("Output from Pandoc is wrong\nExpected:\n{expected}\n\n\nActual:\n{actual}")]
  PandocWrongOutput { expected: String, actual: String },
  #[error(transparent)]
  FrontMatterError(#[from] toml::de::Error),
  #[error(transparent)]
  MakeRelativeError(#[from] std::path::StripPrefixError),
  #[error(transparent)]
  Session(#[from] async_session::Error),
  #[error(transparent)]
  IndieWebError(#[from] indieweb::Error),
  #[error(transparent)]
  SerdeJson(#[from] serde_json::Error),
  #[error("No authorization endpoint found")]
  MissingAuthEndpoint,
  #[error("No token endpoint found")]
  MissingTokenEndpoint,
  #[error("No user cookie found")]
  UserCookie,
  #[error("Missing field {0} from profile")]
  MissingField(&'static str),
  #[error("Session not found")]
  MissingSession,
}

impl IntoResponse for Error {
  fn into_response(self) -> axum::response::Response {
    let code = match &self {
      Self::StaticNotFound { .. } => StatusCode::NOT_FOUND,
      Self::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
      Self::Utf8(_) => StatusCode::BAD_REQUEST,
      Self::PandocError(_) => StatusCode::INTERNAL_SERVER_ERROR,
      Self::PandocInvalidInputFormat(_) => StatusCode::BAD_REQUEST,
      Self::FrontMatterError(_) => StatusCode::INTERNAL_SERVER_ERROR,
      Self::TeraError(_) => StatusCode::INTERNAL_SERVER_ERROR,
      Self::PandocWrongOutput { .. } => StatusCode::INTERNAL_SERVER_ERROR,
      Self::Git(_) => StatusCode::INTERNAL_SERVER_ERROR,
      Self::MakeRelativeError(_) => StatusCode::INTERNAL_SERVER_ERROR,
      Self::Session(_) => StatusCode::INTERNAL_SERVER_ERROR,
      Self::IndieWebError(_) => StatusCode::INTERNAL_SERVER_ERROR,
      Self::SerdeJson(_) => StatusCode::INTERNAL_SERVER_ERROR,
      Self::MissingAuthEndpoint => StatusCode::BAD_REQUEST,
      Self::MissingTokenEndpoint => StatusCode::BAD_REQUEST,
      Self::UserCookie => StatusCode::INTERNAL_SERVER_ERROR,
      Self::MissingField(_) => StatusCode::BAD_REQUEST,
      Self::MissingSession => StatusCode::INTERNAL_SERVER_ERROR,
    };

    let data = match &self {
      Self::StaticNotFound { path } => serde_json::json!({ "path": path }),
      Self::Io(io) => serde_json::json!({ "kind": format!("{:?}", io.kind()) }),
      Self::Utf8(_) => serde_json::json!({}),
      Self::PandocError(_) => serde_json::json!({}),
      Self::PandocInvalidInputFormat(_) => serde_json::json!({}),
      Self::TeraError(_) => serde_json::json!({}),
      Self::FrontMatterError(_) => serde_json::json!({}),
      Self::PandocWrongOutput { expected, actual } => {
        serde_json::json!({ "expected": expected, "actual": actual })
      },
      Self::Git(err) => serde_json::json!({ "class": format!("{:?}", err.class()) }),
      Self::MakeRelativeError(_) => serde_json::json!({}),
      Self::Session(_) => serde_json::json!({}),
      Self::IndieWebError(_) => serde_json::json!({}),
      Self::SerdeJson(_) => serde_json::json!({}),
      Self::MissingAuthEndpoint => serde_json::json!({}),
      Self::MissingTokenEndpoint => serde_json::json!({}),
      Self::UserCookie => serde_json::json!({}),
      Self::MissingField(field) => serde_json::json!({ "missing_field": field }),
      Self::MissingSession => serde_json::json!({}),
    };

    let message = format!("{self}");

    let report = Report::new(self).pretty(true).show_backtrace(true);
    let report = format!("{}", report);

    (
      code,
      Json(serde_json::json!({
        "data": data,
        "message": message,
        "report": report,
      })),
    )
      .into_response()
  }
}
