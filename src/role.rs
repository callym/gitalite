use std::sync::Arc;

use axum::{
  async_trait,
  extract::{FromRequest, RequestParts},
};
use serde::{Deserialize, Serialize};
use tera::{helpers::tests::number_args_allowed, Value};

use crate::{error::Error, user::User, State};

#[derive(Serialize, Deserialize, PartialEq, Eq, Copy, Clone, Debug)]
pub enum Role {
  Administrator,
}

pub struct Is<const ROLE: Role>;

#[async_trait]
impl<const ROLE: Role, B> FromRequest<B> for Is<ROLE>
where
  B: Send,
{
  type Rejection = Error;

  async fn from_request(req: &mut RequestParts<B>) -> Result<Self, Self::Rejection> {
    let user = User::from_request(req).await?;

    if user.roles.contains(&ROLE) {
      return Ok(Self);
    }

    unimplemented!()
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
