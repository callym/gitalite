use std::{fmt::Debug, net::SocketAddr, str::FromStr, sync::Arc};

use async_session::{MemoryStore, Session, SessionStore, SessionStore as _};
use async_sqlx_session::PostgresSessionStore;
use axum::{
  async_trait,
  extract::{Extension, FromRequest, Query, RequestParts, TypedHeader},
  headers::Cookie,
  http::{
    self,
    header::{HeaderMap, HeaderValue},
    StatusCode,
    Uri,
  },
  response::{Html, IntoResponse, Redirect},
  routing::get,
  Form,
  Json,
  Router,
};
use axum_extra::extract::cookie::{Cookie as CookieExt, CookieJar};
use indieweb::standards::indieauth::{self, Client, Profile, Scopes, Token};
use oauth2::{url::Url, ClientId, PkceCodeVerifier, RedirectUrl};
use serde::{Deserialize, Serialize};

use crate::{config::Config, error::Error, State};

pub async fn discover(url: Url) {
  let client = indieweb::http::ureq::Client::default();
  let discovered = dbg!(indieweb::standards::indieauth::discover(&client, &url).await).unwrap();

  let client_id = "https://wiki.callym.com/";
  let redirect_uri = "https://wiki.callym.com/meta/login-callback";
  let scope = "profile+email";

  // let req = AuthenticationRequest::new(
  //   &url.to_string(),
  //   client_id,
  //   &discovered.authorization_endpoints[0].to_string(),
  //   Some(redirect_uri.to_string()),
  //   Some(vec![scope.to_string()]),
  // )
  // .unwrap();

  // let url = dbg!(req.construct_url());
  // dbg!(req.get_pkce_verifier());
}

#[derive(Debug, serde::Deserialize)]
pub struct Params {
  code: String,
  state: String,
}

pub async fn login_handler(state: Arc<State>) -> Result<impl IntoResponse, Error> {
  {
    let mut tera = state.tera.lock().unwrap();
    tera.full_reload()?;
  }

  let render = tokio::task::spawn_blocking(move || {
    let context = tera::Context::new();

    let tera = state.tera.lock().unwrap();

    let rendered = tera.render("login.html", &context)?;

    Ok::<_, Error>(rendered)
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
  state: Arc<State>,
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
    store.destroy_session(session).await;
    jar = jar.remove(cookie);
  }

  let (redirect, session) = User::authenticate(&params.url, &state.config).await?;
  let id = session.id().to_string();
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
  state: Arc<State>,
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

        store.destroy_session(session).await;
        jar = jar.remove(cookie);

        return Ok((jar, Redirect::to("/meta/login")));
      }

      // If session doesn't have a `login` key, then we shouldn't be in the
      // authentication callback, so remove cookie and error out
      if session.get_raw("login").is_none() {
        log::info!("Session doesn't have a `login` key.");

        store.destroy_session(session).await;
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

  let user =
    User::authenticate_callback(&session, params.code, params.state, &state.config).await?;

  // Here we've authenticated successfully, so we can remove the `login` cookie...
  store.destroy_session(session).await.unwrap();
  jar = jar.remove(cookie);

  let session = user.to_session();
  let id = session.id().to_string();
  // ...and add the user-session cookie!
  log::info!("{:#?}", session);
  let cookie = store.store_session(session).await.unwrap().unwrap();

  let cookie = CookieExt::build(SESSION_COOKIE_NAME, cookie)
    .path("/")
    .finish();

  jar = jar.add(cookie);

  return Ok((jar, Redirect::to("/")));
}

const SESSION_COOKIE_NAME: &str = "gitalite_session";

#[derive(Serialize, Deserialize, PartialEq, Eq, Copy, Clone, Debug)]
pub enum Role {
  Administrator,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct User {
  pub name: String,
  pub url: Url,
  pub email: String,
  pub approved: bool,
  pub roles: Vec<Role>,
}

#[derive(Serialize, Deserialize)]
pub struct Login {
  authorization_endpoint: Url,
  token_endpoint: Url,
  verifier: String,
  challenge: String,
  url: Url,
  csrf_token: String,
}

impl User {
  pub async fn setup(app: axum::Router, config: impl AsRef<Config>) -> Result<axum::Router, Error> {
    let store = PostgresSessionStore::new(&config.as_ref().postgresql)
      .await
      .unwrap();
    store.migrate().await.unwrap();

    Ok(app.layer(Extension(store)))
  }

  pub fn from_session(session: &Session) -> Result<Self, Error> {
    dbg!(session);
    let data = session.get("data").ok_or(Error::MissingSession)?;

    Ok(data)
  }

  pub fn to_session(&self) -> Session {
    let mut session = Session::new();

    session.insert("data", self);

    session
  }

  pub async fn authenticate(
    url: &Url,
    config: impl AsRef<Config>,
  ) -> Result<(Url, Session), Error> {
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
    state: String,
    config: impl AsRef<Config>,
  ) -> Result<Self, Error> {
    let config = config.as_ref();
    let http_client = indieweb::http::ureq::Client::default();

    let login: Login = session.get("login").unwrap();

    let client_id = &config.client_id;
    let client_id = ClientId::new(client_id.into());

    let redirect_uri = format!("{}/meta/login-callback", config.client_id);
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

    let name = profile.name.ok_or(Error::MissingField("name"))?;
    let email = profile.email.ok_or(Error::MissingField("email"))?;
    let url = profile.url.ok_or(Error::MissingField("url"))?;

    Ok(Self {
      approved: false,
      roles: Vec::new(),
      name,
      email,
      url,
    })
  }
}

#[async_trait]
impl<B> FromRequest<B> for User
where
  B: Send,
{
  type Rejection = Error;

  async fn from_request(req: &mut RequestParts<B>) -> Result<Self, Self::Rejection> {
    let Extension(store) = Extension::<PostgresSessionStore>::from_request(req)
      .await
      .expect("`PostgresSessionStore` extension missing");

    let cookie = Option::<TypedHeader<Cookie>>::from_request(req)
      .await
      .unwrap();

    let session_cookie = cookie
      .as_ref()
      .and_then(|cookie| cookie.get(SESSION_COOKIE_NAME))
      .ok_or(Error::UserCookie)?;
    let session_cookie = urlencoding::decode(session_cookie)?;

    log::info!("{}", session_cookie);

    dbg!(Session::id_from_cookie_value(&session_cookie).unwrap());

    let session = store
      .load_session(session_cookie.to_string())
      .await
      .unwrap()
      .unwrap();

    Ok(User::from_session(&session)?)
  }
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
