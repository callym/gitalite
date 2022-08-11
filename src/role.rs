use axum::{
  async_trait,
  extract::{FromRequest, RequestParts},
  http::StatusCode,
  response::IntoResponse,
};
use serde::{Deserialize, Serialize};

use crate::{auth::UserExtractError, user::User};

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
