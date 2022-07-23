use std::sync::Arc;

use axum::{
  async_trait,
  extract::{FromRequest, RequestParts},
  http::StatusCode,
  response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use tera::{helpers::tests::number_args_allowed, Value};

use crate::{auth::UserExtractError, user::User, State};

#[derive(Serialize, Deserialize, PartialEq, Eq, Copy, Clone, Debug)]
pub enum Role {
  Administrator,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
  #[error(transparent)]
  UserExtract(#[from] UserExtractError),
  #[error("Unauthorised: User is not '{0:?}'")]
  Unauthorised(Role),
}

impl IntoResponse for Error {
  fn into_response(self) -> axum::response::Response {
    let code = match self {
      Self::Unauthorised(_) => StatusCode::UNAUTHORIZED,
      _ => StatusCode::INTERNAL_SERVER_ERROR,
    };

    (code, self.to_string()).into_response()
  }
}

pub struct Is<const ROLE: Role>(User);

#[async_trait]
impl<const ROLE: Role, B> FromRequest<B> for Is<ROLE>
where
  B: Send,
{
  type Rejection = Error;

  async fn from_request(req: &mut RequestParts<B>) -> Result<Self, Self::Rejection> {
    let user = User::from_request(req).await?;

    if user.roles.contains(&ROLE) {
      return Ok(Self(user));
    }

    Err(Error::Unauthorised(ROLE))
  }
}

pub async fn setup(app: axum::Router, state: Arc<State>) -> Result<axum::Router, Error> {
  fn is(value: Option<&Value>, params: &[Value]) -> tera::Result<bool> {
    number_args_allowed("Role::is", 1, params.len())?;

    let value = match value {
      Some(value) => value,
      None => return Ok(false),
    };

    if value.is_null() {
      return Ok(false);
    }

    let user: User = serde_json::from_value(value.clone())?;
    let role: Role = serde_json::from_value(params[0].clone())?;

    Ok(user.roles.contains(&role))
  }

  let mut tera = state.tera.lock().unwrap();
  tera.register_tester("role", is);

  Ok(app)
}
