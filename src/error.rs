use axum::{
  extract::RawQuery,
  response::{Html, IntoResponse, Redirect, Response},
};
use serde::{Deserialize, Serialize};

use crate::{
  template::{PrettyPrint, Template},
  user::User,
};

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

pub async fn handler(RawQuery(query): RawQuery, user: Option<User>) -> Html<String> {
  let error = if let Some(query) = query {
    match serde_qs::from_str(&query) {
      Ok(ErrorPageWrapper { error }) => error,
      _ => ErrorPage::Unknown,
    }
  } else {
    ErrorPage::Unknown
  };

  let content = maud::html! {
    @match &error {
      ErrorPage::ReservedPage { url } => {
        "You can't make the page at " (url) " because it's reserved for future internal use, sorry!"
      },
      ErrorPage::Unknown => { "An unknown error occured, sorry!" },
    }

    pre { (PrettyPrint(error)) }
  };

  Template::new().title("Error").content(content).render(user)
}
