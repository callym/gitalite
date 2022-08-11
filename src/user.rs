use std::{
  collections::HashMap,
  fmt::Debug,
  path::{Path, PathBuf},
  sync::Arc,
};

use async_session::Session;
use axum::{extract::Extension, response::Html};
use cocoon::Cocoon;
use oauth2::url::Url;
use serde::{Deserialize, Serialize};

use crate::{
  config::Config,
  role::Role,
  template::{PrettyPrint, Template},
  State,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
  #[error(transparent)]
  Io(#[from] tokio::io::Error),
  #[error(transparent)]
  Ron(#[from] ron::Error),
  #[error("Cocoon error: {0:#?}")]
  Cocoon(cocoon::Error),
}

impl From<cocoon::Error> for Error {
  fn from(err: cocoon::Error) -> Self {
    Error::Cocoon(err)
  }
}

#[derive(Serialize, Deserialize, Clone, Debug, Hash, PartialEq, Eq)]
pub struct UserKey(String);

impl UserKey {
  pub fn email(&self) -> &str {
    &self.0
  }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct UserValue(Url);

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct User {
  pub name: String,
  pub email: String,
  pub url: Url,
  pub approved: bool,
  pub roles: Vec<Role>,
}

impl User {
  pub fn key(&self) -> UserKey {
    self.email.clone().into()
  }
}

impl From<String> for UserKey {
  fn from(email: String) -> UserKey {
    UserKey(email)
  }
}

#[derive(Debug, thiserror::Error)]
#[error("Session doesn't contain `data` key.")]
pub struct UserKeyError;

impl UserKey {
  pub fn from_session(session: &Session) -> Result<Self, UserKeyError> {
    let data = session.get("data").ok_or(UserKeyError)?;

    Ok(data)
  }

  pub fn to_session(&self) -> Session {
    let mut session = Session::new();

    session
      .insert("data", self)
      .expect("Failed to create session!");

    session
  }
}

#[derive(Serialize, Deserialize)]
pub struct UserDb {
  path: PathBuf,
  password: Vec<u8>,
  map: HashMap<UserKey, User>,
}

impl UserDb {
  pub async fn new(config: impl AsRef<Config>) -> Result<Self, Error> {
    let config = config.as_ref();

    let password = tokio::fs::read(&config.users.password).await?;

    if config.users.database.exists() {
      Self::from_path(&config.users.database, &password)
    } else {
      log::info!("Creating new user database");

      let mut db = Self {
        path: config.users.database.clone(),
        password,
        map: HashMap::new(),
      };

      let user = User {
        name: config.users.initial.name.clone(),
        email: config.users.initial.email.clone(),
        url: config.users.initial.url.clone(),
        approved: true,
        roles: vec![Role::Administrator],
      };

      db.set(user)?;

      Ok(db)
    }
  }

  pub fn from_path(path: impl AsRef<Path>, password: &[u8]) -> Result<Self, Error> {
    log::info!("Loading user database from {}", path.as_ref().display());

    let mut file = std::fs::File::open(path.as_ref())?;
    let cocoon = Cocoon::new(password).parse(&mut file)?;
    let map = ron::de::from_bytes(&cocoon)?;

    for (k, v) in &map {
      log::info!("{:?}, {:?}", k, v);
    }

    Ok(Self {
      map,
      path: path.as_ref().to_path_buf(),
      password: password.to_vec(),
    })
  }

  pub fn save(&self) -> Result<(), Error> {
    log::info!("Saving user database");

    let mut file = std::fs::File::create(&self.path)?;
    let value = ron::to_string(&self.map)?;

    Cocoon::new(&self.password).dump(value.as_bytes().to_vec(), &mut file)?;

    Ok(())
  }

  pub fn get(&self, key: &UserKey) -> Option<&User> {
    self.map.get(key)
  }

  pub fn set(&mut self, user: User) -> Result<(), Error> {
    self.map.insert(UserKey(user.email.clone()), user.into());
    self.save()
  }
}

pub async fn profile_handler(
  axum::extract::Path(user_key): axum::extract::Path<UserKey>,
  user: Option<User>,
  Extension(state): Extension<Arc<State>>,
) -> Result<Html<String>, crate::page::Error> {
  let profile = {
    let users = state.users.lock().unwrap();
    users.get(&user_key).unwrap().clone()
  };

  let recent_commits = tokio::task::spawn_blocking({
    let profile = profile.clone();
    move || state.git.user_history(&profile.key(), Some(10), &state)
  })
  .await
  .unwrap()?;

  let content = maud::html! {
    @if let Some(user) = &user {
      pre { (PrettyPrint(user)) }

      h1 { (profile.name) }

      ul {
        li {
          a href=(profile.url) { (profile.url) }
        }
        li {
          a href={"mailto:" (profile.email)} { (profile.email) }
        }
      }

      ol #commits {
        @for commit in recent_commits.iter().take(10) {
          li {
            .date { (commit.date) }
            .message { (commit.message) }
            ul .files {
              @for file in &commit.files {
                li {
                  a href={"/" (file.to_string_lossy()) "?revision=" (commit.hash)} { (file.to_string_lossy()) }
                }
              }
            }
          }
        }
      }
    }
  };

  let html = Template::new()
    .title(profile.url)
    .content(content)
    .render(user);

  Ok(html)
}
