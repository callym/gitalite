use crate::user::User;

#[derive(serde::Serialize)]
pub struct Context {
  pub title: String,
  pub user: Option<User>,
}
