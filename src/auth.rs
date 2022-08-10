use std::{str::FromStr, string::FromUtf8Error, sync::Arc};

use async_session::{Session, SessionStore};
use async_sqlx_session::PostgresSessionStore;
use axum::{
  async_trait,
  extract::{Extension, FromRequest, Query, RequestParts, TypedHeader},
  headers::Cookie,
  http::StatusCode,
  response::{Html, IntoResponse, Redirect},
  Form,
};
use axum_extra::extract::cookie::{Cookie as CookieExt, CookieJar};
use indieweb::standards::indieauth::{self, Client, Scopes};
use oauth2::{url::Url, ClientId, RedirectUrl};
use serde::{Deserialize, Serialize};

use crate::{
  config::Config,
  user::{User, UserKey},
  State,
};

#[derive(thiserror::Error, Debug)]
pub enum Error {
  #[error(transparent)]
  TeraError(#[from] tera::Error),
  #[error(transparent)]
  Session(#[from] async_session::Error),
  #[error(transparent)]
  Utf8(#[from] FromUtf8Error),
  #[error("No authorization endpoint found")]
  MissingAuthEndpoint,
  #[error("No token endpoint found")]
  MissingTokenEndpoint,
  #[error(transparent)]
  IndieWebError(#[from] indieweb::Error),
  #[error(transparent)]
  SerdeJson(#[from] serde_json::Error),
  #[error("Missing field {0} from profile")]
  MissingField(&'static str),
  #[error(transparent)]
  User(#[from] crate::user::Error),
}

impl IntoResponse for Error {
  fn into_response(self) -> axum::response::Response {
    let code = match self {
      Self::Utf8(_) => StatusCode::BAD_REQUEST,
      Self::MissingAuthEndpoint => StatusCode::BAD_REQUEST,
      Self::MissingTokenEndpoint => StatusCode::BAD_REQUEST,
      Self::MissingField(_) => StatusCode::BAD_REQUEST,
      _ => StatusCode::INTERNAL_SERVER_ERROR,
    };

    (code, self.to_string()).into_response()
  }
}

#[derive(Debug, serde::Deserialize)]
pub struct Params {
  code: String,
  state: String,
}

pub async fn login_handler(
  Extension(state): Extension<Arc<State>>,
) -> Result<impl IntoResponse, crate::page::Error> {
  {
    let mut tera = state.tera.lock().unwrap();
    tera.full_reload()?;
  }

  let render = tokio::task::spawn_blocking(move || {
    let context = tera::Context::new();

    let tera = state.tera.lock().unwrap();

    let rendered = tera.render("login.html", &context)?;

    Ok::<_, crate::page::Error>(rendered)
  })
  .await
  .unwrap()?;

  Ok(Html(render))
}

#[derive(Debug, serde::Deserialize)]
pub struct AuthenticateParams {
  url: Url,
}

pub async fn authenticate_handler(
  Form(params): Form<AuthenticateParams>,
  mut jar: CookieJar,
  store: Extension<PostgresSessionStore>,
  Extension(state): Extension<Arc<State>>,
) -> Result<impl IntoResponse, Error> {
  // Get session from the cookie
  let session = match jar.get(SESSION_COOKIE_NAME).cloned() {
    Some(cookie) => {
      let cookie_value = urlencoding::decode(cookie.value())?.to_string();
      store
        .load_session(cookie_value)
        .await?
        .map(|session| (cookie, session))
    },
    None => None,
  };

  // Remove any active sessions, if there are any
  if let Some((cookie, session)) = session {
    store.destroy_session(session).await?;
    jar = jar.remove(cookie);
  }

  let (redirect, session) = authenticate(&params.url, &state.config).await?;
  let cookie = store.store_session(session).await.unwrap().unwrap();

  let cookie = CookieExt::build(SESSION_COOKIE_NAME, cookie)
    .path("/")
    .finish();

  jar = jar.add(cookie);

  return Ok((jar, Redirect::to(redirect.as_str())));
}

pub async fn callback_handler(
  Query(params): Query<Params>,
  mut jar: CookieJar,
  store: Extension<PostgresSessionStore>,
  Extension(state): Extension<Arc<State>>,
) -> Result<impl IntoResponse, Error> {
  // Get session from the cookie
  let session = match jar.get(SESSION_COOKIE_NAME).cloned() {
    Some(cookie) => {
      let cookie_value = urlencoding::decode(cookie.value())?.to_string();
      store
        .load_session(cookie_value)
        .await?
        .map(|session| (cookie, session))
    },
    None => None,
  };

  let (cookie, session) = match session {
    Some((cookie, session)) => {
      // Session isn't valid - remove cookie and error out
      if session.is_destroyed() || session.is_expired() {
        if session.is_destroyed() {
          log::info!("Session is destroyed.");
        } else {
          log::info!("Session is invalid.");
        }

        store.destroy_session(session).await?;
        jar = jar.remove(cookie);

        return Ok((jar, Redirect::to("/meta/login")));
      }

      // If session doesn't have a `login` key, then we shouldn't be in the
      // authentication callback, so remove cookie and error out
      if session.get_raw("login").is_none() {
        log::info!("Session doesn't have a `login` key.");

        store.destroy_session(session).await?;
        jar = jar.remove(cookie);

        return Ok((jar, Redirect::to("/meta/login")));
      }

      (cookie, session)
    },
    // There's no session - if there's a cookie, remove it,
    // then error out
    None => {
      log::info!("No session found.");

      let session_cookie = jar.get(SESSION_COOKIE_NAME).cloned();

      if let Some(cookie) = session_cookie {
        jar = jar.remove(cookie);
      }

      return Ok((jar, Redirect::to("/")));
    },
  };

  let user = authenticate_callback(&session, params.code, params.state, &state).await?;

  // Here we've authenticated successfully, so we can remove the `login` cookie...
  store.destroy_session(session).await.unwrap();
  jar = jar.remove(cookie);

  let session = user.key().to_session();
  // ...and add the user-session cookie!
  let cookie = store.store_session(session).await.unwrap().unwrap();

  let cookie = CookieExt::build(SESSION_COOKIE_NAME, cookie)
    .path("/")
    .finish();

  jar = jar.add(cookie);

  return Ok((jar, Redirect::to("/")));
}

const SESSION_COOKIE_NAME: &str = "gitalite_session";

#[derive(Serialize, Deserialize)]
pub struct Login {
  authorization_endpoint: Url,
  token_endpoint: Url,
  verifier: String,
  challenge: String,
  url: Url,
  csrf_token: String,
}

pub async fn setup(app: axum::Router, state: Arc<State>) -> Result<axum::Router, Error> {
  let store = PostgresSessionStore::new(&state.config.postgresql)
    .await
    .unwrap();
  store.migrate().await.unwrap();

  Ok(app.layer(Extension(store)))
}

pub async fn authenticate(url: &Url, config: impl AsRef<Config>) -> Result<(Url, Session), Error> {
  let config = config.as_ref();
  let http_client = indieweb::http::ureq::Client::default();

  let discovered = indieauth::discover(&http_client, url).await.unwrap();

  let client_id = &config.client_id;
  let client_id = ClientId::new(client_id.into());

  let redirect_uri = format!("{}/meta/login-callback", config.client_id);
  let redirect_uri = RedirectUrl::new(redirect_uri.into()).unwrap();

  let scope = Scopes::from_str("profile email").unwrap();

  let authorization_endpoint = discovered
    .authorization_endpoints
    .map(|end| end.first().cloned())
    .flatten()
    .ok_or(Error::MissingAuthEndpoint)?;
  let authorization_endpoint = indieauth::AuthUrl::from_url(authorization_endpoint);

  let token_endpoint = discovered
    .token_endpoints
    .map(|end| end.first().cloned())
    .flatten()
    .ok_or(Error::MissingTokenEndpoint)?;
  let token_endpoint = indieauth::TokenUrl::from_url(token_endpoint);

  let client = indieauth::StockClient::from((
    client_id,
    authorization_endpoint.clone(),
    token_endpoint.clone(),
  ));

  let (verifier, challenge, url, csrf_token) = match client.dispatch(
    &http_client,
    indieauth::Request::BuildAuthorizationUrl {
      scope: Some(scope),
      redirect_uri: Some(redirect_uri),
      me: Some(url.clone()),
    },
  )? {
    indieauth::Response::AuthenticationUrl {
      verifier,
      challenge,
      url,
      csrf_token,
    } => (verifier, challenge, url, csrf_token),
    _ => unreachable!(),
  };

  let mut session = Session::new();

  session.insert(
    "login",
    Login {
      authorization_endpoint: authorization_endpoint.url().clone(),
      token_endpoint: token_endpoint.url().clone(),
      verifier,
      challenge,
      url: url.clone(),
      csrf_token,
    },
  )?;

  {
    use time::ext::NumericalStdDuration;

    session.expire_in(1.std_hours());
  }

  Ok((url, session))
}

pub async fn authenticate_callback(
  session: &Session,
  code: String,
  auth_state: String,
  state: &Arc<State>,
) -> Result<User, Error> {
  let http_client = indieweb::http::ureq::Client::default();

  let login: Login = session.get("login").unwrap();

  let client_id = &state.config.client_id;
  let client_id = ClientId::new(client_id.into());

  let redirect_uri = format!("{}/meta/login-callback", state.config.client_id);
  let redirect_uri = RedirectUrl::new(redirect_uri.into()).unwrap();

  let client = indieauth::StockClient::from((
    client_id,
    indieauth::AuthUrl::from_url(login.authorization_endpoint.clone()),
    indieauth::TokenUrl::from_url(login.token_endpoint.clone()),
  ));

  let profile = match client.dispatch(
    &http_client,
    indieauth::Request::CompleteAuthorization {
      resource: indieauth::DesiredResourceAuthorization::Profile,
      code: indieauth::AuthorizationCode::new(code),
      code_verifier: login.verifier,
      redirect_uri: Some(redirect_uri),
    },
  )? {
    indieauth::Response::Profile(profile) => profile,
    _ => unreachable!(),
  };

  let email = profile.email.ok_or(Error::MissingField("email"))?;
  let name = profile.name.ok_or(Error::MissingField("name"))?;
  let url = profile.url.ok_or(Error::MissingField("url"))?;

  let user = {
    let key = UserKey::from(email.clone());
    let mut users = state.users.lock().unwrap();

    match users.get(&key) {
      Some(user) => {
        let mut new_user = user.clone();

        if new_user.name != name {
          log::info!("Updating name for {}", &email);
          new_user.name = name;
        }

        if new_user.url != url {
          log::info!("Updating url for {}", &email);
          new_user.url = url;
        }

        if new_user.email != email {
          log::info!("Updating email for {}", &email);
          new_user.email = email;
        }

        if new_user != *user {
          users.set(new_user.clone())?;
        }

        new_user
      },
      None => {
        let user = User {
          name,
          email,
          url: url.into(),
          approved: false,
          roles: Vec::new(),
        };

        users.set(user.clone())?;
        user
      },
    }
  };

  Ok(user)
}

#[derive(Debug, thiserror::Error)]
pub enum UserExtractError {
  #[error(transparent)]
  UserKey(#[from] crate::user::UserKeyError),
  #[error("No user cookie found")]
  UserCookie,
  #[error(transparent)]
  Utf8(#[from] FromUtf8Error),
  #[error("Unauthorised")]
  Unauthorised,
}

impl IntoResponse for UserExtractError {
  fn into_response(self) -> axum::response::Response {
    let code = match self {
      Self::Unauthorised => StatusCode::UNAUTHORIZED,
      _ => StatusCode::INTERNAL_SERVER_ERROR,
    };

    (code, self.to_string()).into_response()
  }
}

#[async_trait]
impl<B> FromRequest<B> for User
where
  B: Send,
{
  type Rejection = UserExtractError;

  async fn from_request(req: &mut RequestParts<B>) -> Result<Self, Self::Rejection> {
    let Extension(store) = Extension::<PostgresSessionStore>::from_request(req)
      .await
      .expect("`PostgresSessionStore` extension missing");
    let Extension(state) = Extension::<Arc<State>>::from_request(req)
      .await
      .expect("`State` extension missing");

    let cookie = Option::<TypedHeader<Cookie>>::from_request(req)
      .await
      .unwrap();

    let session_cookie = cookie
      .as_ref()
      .and_then(|cookie| cookie.get(SESSION_COOKIE_NAME))
      .ok_or(UserExtractError::UserCookie)?;
    let session_cookie = urlencoding::decode(session_cookie)?;

    log::info!("{}", session_cookie);

    dbg!(Session::id_from_cookie_value(&session_cookie).unwrap());

    let session = store
      .load_session(session_cookie.to_string())
      .await
      .ok()
      .flatten()
      .ok_or(UserExtractError::Unauthorised)?;

    let users = state.users.lock().unwrap();
    users
      .get(&UserKey::from_session(&session)?)
      .ok_or(UserExtractError::Unauthorised)
      .cloned()
  }
}
