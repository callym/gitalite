#![feature(adt_const_params, error_reporter)]

use std::{
  path::PathBuf,
  sync::{Arc, Mutex},
};

use axum::{
  extract::{FromRequest, Path, Query, RequestParts},
  http::{header, Request},
  response::{Html, IntoResponse, Redirect, Response},
  routing::{get, post},
  Extension,
  Json,
  Router,
};
use git::Git;
use page::{Page, PagePathError};
use tera::{Context, Tera};
use user::UserDb;

use crate::{
  config::{Args, Config},
  pandoc::{Format, QueryFormat},
  user::User,
};

mod auth;
mod config;
mod context;
mod git;
mod page;
mod pandoc;
mod role;
mod user;

#[derive(Clone)]
pub struct State {
  config: Arc<Config>,
  git: Arc<Git>,
  tera: Arc<Mutex<Tera>>,
  users: Arc<Mutex<UserDb>>,
}

#[tokio::main]
async fn main() -> Result<(), eyre::Report> {
  color_eyre::install()?;
  pretty_env_logger::try_init()?;

  let args = Args::parse();

  let config = tokio::fs::read_to_string(args.config).await?;
  let mut config: Config = ron::from_str(&config)?;

  // We make the directory, so we can canonicalize it!
  tokio::fs::create_dir_all(&config.pages_directory).await?;

  config.canonicalize()?;

  let config = Arc::new(config);

  let git = git::Git::new(config.clone())?;
  let git = Arc::new(git);

  // Use globbing
  let tera = Tera::new(
    config
      .templates_directory
      .join("**/*.html")
      .to_str()
      .unwrap(),
  )?;
  let tera = Arc::new(Mutex::new(tera));

  let users = UserDb::new(config.clone()).await?;
  let users = Arc::new(Mutex::new(users));

  let state = State {
    config,
    git,
    tera,
    users,
  };
  let state = Arc::new(state);

  pandoc::test_output()?;

  // build our application with a route
  let app = Router::new()
    .route(
      "/meta/login",
      get(|state| auth::login_handler(state)).post(
        move |body, jar, store, state| auth::authenticate_handler(body, jar, store, state)
      ),
    )
    .route(
      "/meta/login-callback",
      get(|query, jar, store, state| auth::callback_handler(query, jar, store, state)),
    )
    .route(
      "/meta/profile/:user",
      get(|jar, store, state| user::profile_handler(jar, store, state)),
    )
    .route(
      "/meta/new/*path",
      get(|path, user, state| get_new(path, user, state))
        .post(|path, body, user, state| new(path, body, user, state)),
    )
    .route(
      "/meta/history/*path",
      get(|page, state| history(page, state)),
    )
    .route(
      "/meta/edit/*path",
      get(|page, state| edit(page, state))
        .post(|body, page, user, state| save(page, body, user, state)),
    )
    .route(
      "/meta/raw/*path",
      get(|page| raw(page)),
    )
    .route(
      "/meta/render",
      post(|body, format, state| render(body, format, state)),
    )
    .fallback(get(|request| route(request)));

  let app = auth::setup(app, state.clone()).await?;
  let app = role::setup(app, state.clone()).await?;
  let app = app.layer(Extension(state.clone()));

  log::info!("listening on {}", state.config.listen_on);
  axum::Server::bind(&state.config.listen_on)
    .serve(app.into_make_service())
    .await?;

  Ok(())
}

async fn static_handler(path: &std::path::Path, _: &State) -> Result<impl IntoResponse, crate::page::Error> {
  let mime = mime_guess::from_path(path).first_or_text_plain();

  let file = tokio::fs::read(path).await?;

  Ok((
    [(header::CONTENT_TYPE, mime.essence_str().to_string())],
    file,
  ))
}

#[derive(serde::Deserialize)]
struct RouteQuery {
  revision: Option<String>,
}

async fn route<T: Send>(request: Request<T>) -> Response {
  let path = request.uri().path();
  let path = path.strip_prefix("/").unwrap();
  let path = match urlencoding::decode(path) {
    Ok(path) => path.to_string(),
    Err(err) => return Err::<(), _>(crate::page::Error::Utf8(err)).into_response(),
  };
  let path = PathBuf::from(path);

  let query = request.uri().query().unwrap_or("");
  let query = serde_qs::from_str::<RouteQuery>(query).unwrap();

  let mut parts = RequestParts::new(request);

  let Extension(state) = Extension::<Arc<State>>::from_request(&mut parts)
    .await
    .expect("`State` extension missing");

  let static_path = state.config.static_directory.join(&path);
  if static_path.is_file() {
    return static_handler(&static_path, &state).await.into_response();
  }

  let page = match Page::from_request(&mut parts).await {
    Ok(page) => page,
    Err(PagePathError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {
      return Redirect::to(&format!("/meta/new/{}", path.display())).into_response();
    },
    Err(err) => return err.into_response(),
  };


  if let Some(revision) = query.revision {
    return state
      .git
      .clone()
      .history_handler(&page, revision, state)
      .await
      .into_response();
  }

  return page.view_handler(state.clone()).await.into_response();
}

async fn history(page: Page, Extension(state): Extension<Arc<State>>) -> Response {
  state
    .git
    .clone()
    .history_listing_handler(&page, state)
    .await
    .into_response()
}

async fn edit(page: Page, Extension(state): Extension<Arc<State>>) -> Response {
  page.edit_handler(state).await.into_response()
}

async fn save(
  page: Page,
  body: String,
  user: User,
  Extension(state): Extension<Arc<State>>,
) -> Response {
  match page.update(body, &user, state).await {
    Ok(_) => Redirect::to(&page.url_path()).into_response(),
    Err(err) => err.into_response(),
  }
}

async fn get_new(
  Path(path): Path<String>,
  user: Option<User>,
  Extension(state): Extension<Arc<State>>,
) -> Response {
  let path = path.strip_prefix("/").unwrap();

  match page::find_file(&path, &state.config) {
    Ok(path) => return Redirect::to(&format!("/{}", path.display())).into_response(),
    Err(err) if err.kind() == std::io::ErrorKind::NotFound => (),
    Err(err) => return crate::page::Error::from(err).into_response(),
  };

  let mut context = Context::new();
  context.insert("path", path);
  context.insert("user", &user);
  context.insert("supported_formats", &crate::pandoc::VALID_FORMATS_WITH_NAME);

  tokio::task::spawn_blocking(move || {
    let tera = state.tera.lock().unwrap();

    let rendered = tera.render("new.html", &context)?;

    Ok::<_, crate::page::Error>(rendered)
  })
  .await
  .unwrap()
  .map(|html| Html(html))
  .into_response()
}

#[derive(serde::Deserialize)]
struct NewPage {
  body: String,
  format: Format,
}

async fn new(
  Path(url_path): Path<String>,
  Json(new_page): Json<NewPage>,
  user: User,
  Extension(state): Extension<Arc<State>>,
) -> Response {
  let path = url_path.strip_prefix("/").unwrap();
  let path = PathBuf::from(path);

  let filepath = dbg!(state.config.pages_directory.join(&path))
    .with_extension(new_page.format.extension());

  let page = Page {
    path,
    filepath,
    format: Some(new_page.format),
    user: Some(user.clone()),
  };

  match page.create(new_page.body, &user, state).await {
    Ok(_) => Redirect::to(&url_path).into_response(),
    Err(err) => err.into_response(),
  }
}

async fn raw(page: Page) -> Response {
  page.raw().await.into_response()
}

async fn render(
  body: String,
  format: Option<Query<QueryFormat>>,
  Extension(state): Extension<Arc<State>>,
) -> Response {
  let format = format.map(|query| query.0);

  tokio::task::spawn_blocking(move || {
    let rendered = crate::pandoc::to_html(body, format.map(|f| f.into()), state)?;

    Ok::<_, crate::page::Error>(Html(rendered))
  })
  .await
  .unwrap()
  .into_response()
}
