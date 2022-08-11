use std::sync::Arc;

use axum::{
  extract::RawQuery,
  http::StatusCode,
  response::{Html, IntoResponse, Redirect, Response},
  Extension,
};
use serde::{Deserialize, Serialize};

use crate::{user::User, State};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ErrorPage {
  ReservedPage { url: String },
  Unknown,
}

#[derive(Debug, Serialize, Deserialize)]
struct ErrorPageWrapper {
  error: ErrorPage,
}

impl ErrorPage {
  pub fn query_string(self) -> String {
    // Should be good to unwrap here because we control the input!
    serde_qs::to_string(&ErrorPageWrapper { error: self }).unwrap()
  }
}

impl IntoResponse for ErrorPage {
  fn into_response(self) -> Response {
    Redirect::to(&format!("/meta/error?{}", self.query_string())).into_response()
  }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
  #[error(transparent)]
  TeraError(#[from] tera::Error),
}

impl IntoResponse for Error {
  fn into_response(self) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()).into_response()
  }
}

pub async fn handler(
  RawQuery(query): RawQuery,
  user: Option<User>,
  Extension(state): Extension<Arc<State>>,
) -> Result<Html<String>, Error> {
  {
    let mut tera = state.tera.lock().unwrap();
    tera.full_reload().unwrap();
  }

  let mut context = tera::Context::new();
  context.insert("user", &user);

  let error = if let Some(query) = query {
    match serde_qs::from_str(&query) {
      Ok(ErrorPageWrapper { error }) => error,
      _ => ErrorPage::Unknown,
    }
  } else {
    ErrorPage::Unknown
  };

  context.insert("error", &error);

  let html = tokio::task::spawn_blocking(move || {
    let tera = state.tera.lock().unwrap();

    let rendered = tera.render("error.html", &context)?;

    Ok::<_, Error>(rendered)
  })
  .await
  .unwrap()
  .map(|html| Html(html))?;

  Ok(html)
}
