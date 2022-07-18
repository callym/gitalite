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
  Json,
  Router,
};
use git::Git;
use page::Page;
use tera::{Context, Tera};

use crate::{
  auth::User,
  config::{Args, Config},
  error::Error,
  pandoc::{Format, QueryFormat},
};

mod auth;
mod config;
mod context;
mod error;
mod git;
mod page;
mod pandoc;

#[derive(Clone)]
pub struct State {
  config: Arc<Config>,
  git: Arc<Git>,
  tera: Arc<Mutex<Tera>>,
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

  let state = State { config, git, tera };
  let state = Arc::new(state);

  pandoc::test_output()?;

  // build our application with a route
  let app = Router::new()
    .route(
      "/meta/login",
      get({
        let state = Arc::clone(&state);
        move || auth::login_handler(state)
      })
      .post({
        let state = Arc::clone(&state);
        move |body, jar, store| auth::authenticate_handler(body, jar, store, state)
      }),
    )
    .route(
      "/meta/login-callback",
      get({
        let state = Arc::clone(&state);
        move |query, jar, store| auth::callback_handler(query, jar, store, state)
      }),
    )
    .route(
      "/meta/new/*path",
      get({
        let state = Arc::clone(&state);
        move |path, user| get_new(path, user, state)
      })
      .post({
        let state = Arc::clone(&state);
        move |path, body, user| new(path, body, user, state)
      }),
    )
    .route(
      "/meta/history/*path",
      get({
        let state = Arc::clone(&state);
        move |path, user| history(path, user, state)
      }),
    )
    .route(
      "/meta/edit/*path",
      get({
        let state = Arc::clone(&state);
        move |path, user| edit(path, user, state)
      })
      .post({
        let state = Arc::clone(&state);
        move |body, path, user| save(path, body, user, state)
      }),
    )
    .route(
      "/meta/raw/*path",
      get({
        let state = Arc::clone(&state);
        move |path, user| raw(path, user, state)
      }),
    )
    .route(
      "/meta/render",
      post({
        let state = Arc::clone(&state);
        move |body, format| render(body, format, state)
      }),
    )
    .fallback(get({
      let state = Arc::clone(&state);
      move |request| route(request, state)
    }));

  let app = User::setup(app, &state.config).await?;

  log::info!("listening on {}", state.config.listen_on);
  axum::Server::bind(&state.config.listen_on)
    .serve(app.into_make_service())
    .await?;

  Ok(())
}

async fn static_handler(path: &std::path::Path, _: &State) -> Result<impl IntoResponse, Error> {
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

async fn route<T: Send>(request: Request<T>, state: Arc<State>) -> Response {
  let path = request.uri().path();
  let path = path.strip_prefix("/").unwrap();
  let path = match dbg!(urlencoding::decode(path)) {
    Ok(path) => path.to_string(),
    Err(err) => return Err::<(), _>(Error::Utf8(err)).into_response(),
  };
  let path = dbg!(PathBuf::from(path));

  let query = request.uri().query().unwrap_or("");
  let query = serde_qs::from_str::<RouteQuery>(query).unwrap();

  let mut parts = RequestParts::new(request);
  let user = Option::<User>::from_request(&mut parts).await.unwrap();

  let static_path = state.config.static_directory.join(&path);
  if static_path.is_file() {
    return static_handler(&static_path, &state).await.into_response();
  }

  let page_path = state.config.pages_directory.join(&path);
  let filepath = match find_file(page_path) {
    Ok(path) => path,
    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
      return Redirect::to(&format!("/meta/new/{}", path.display())).into_response();
    },
    Err(err) => return Error::from(err).into_response(),
  };

  let format = filepath
    .extension()
    .map(|e| e.to_str())
    .flatten()
    .map(|ext| Format::from_extension(ext))
    .flatten();

  let page = Page {
    path,
    filepath,
    format,
    user,
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

async fn history(Path(path): Path<String>, user: Option<User>, state: Arc<State>) -> Response {
  let path = path.strip_prefix("/").unwrap();
  let path = PathBuf::from(path);

  let dir = std::env::current_dir().unwrap();
  let page_path = dbg!(dir.join(&state.config.pages_directory).join(&path));

  let filepath = match find_file(page_path) {
    Ok(path) => path,
    Err(err) => return Error::from(err).into_response(),
  };

  let format = filepath
    .extension()
    .map(|e| e.to_str())
    .flatten()
    .map(|ext| Format::from_extension(ext))
    .flatten();

  let page = Page {
    path,
    filepath,
    format,
    user,
  };

  state
    .git
    .clone()
    .history_listing_handler(&page, state)
    .await
    .into_response()
}

async fn edit(Path(path): Path<String>, user: Option<User>, state: Arc<State>) -> Response {
  let path = path.strip_prefix("/").unwrap();
  let path = PathBuf::from(path);

  let dir = std::env::current_dir().unwrap();
  let page_path = dbg!(dir.join(&state.config.pages_directory).join(&path));

  let filepath = match find_file(page_path) {
    Ok(path) => path,
    Err(err) => return Error::from(err).into_response(),
  };

  let format = filepath
    .extension()
    .map(|e| e.to_str())
    .flatten()
    .map(|ext| Format::from_extension(ext))
    .flatten();

  let page = Page {
    path,
    filepath,
    format,
    user,
  };

  page.edit_handler(state).await.into_response()
}

async fn save(
  Path(url_path): Path<String>,
  body: String,
  user: User,
  state: Arc<State>,
) -> Response {
  let path = url_path.strip_prefix("/").unwrap();
  let path = PathBuf::from(path);

  let dir = std::env::current_dir().unwrap();
  let page_path = dbg!(dir.join(&state.config.pages_directory).join(&path));

  let filepath = match find_file(page_path) {
    Ok(path) => path,
    Err(err) => return Error::from(err).into_response(),
  };

  let format = filepath
    .extension()
    .map(|e| e.to_str())
    .flatten()
    .map(|ext| Format::from_extension(ext))
    .flatten();

  let page = Page {
    path,
    filepath,
    format,
    user: Some(user.clone()),
  };

  match page.update(body, &user, state).await {
    Ok(_) => Redirect::to(&url_path).into_response(),
    Err(err) => Error::from(err).into_response(),
  }
}

async fn get_new(Path(path): Path<String>, user: Option<User>, state: Arc<State>) -> Response {
  let path = path.strip_prefix("/").unwrap();

  let dir = std::env::current_dir().unwrap();
  let page_path = dbg!(dir.join(&state.config.pages_directory).join(&path));

  match find_file(page_path) {
    Ok(path) => return Redirect::to(&format!("/{}", path.display())).into_response(),
    Err(err) if err.kind() == std::io::ErrorKind::NotFound => (),
    Err(err) => return Error::from(err).into_response(),
  };

  let mut context = Context::new();
  context.insert("path", path);
  context.insert("user", &user);
  context.insert("supported_formats", &crate::pandoc::VALID_FORMATS_WITH_NAME);

  tokio::task::spawn_blocking(move || {
    let tera = state.tera.lock().unwrap();

    let rendered = tera.render("new.html", &context)?;

    Ok::<_, Error>(rendered)
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
  state: Arc<State>,
) -> Response {
  let path = url_path.strip_prefix("/").unwrap();
  let path = PathBuf::from(path);

  let dir = std::env::current_dir().unwrap();
  let filepath = dbg!(dir.join(&state.config.pages_directory).join(&path))
    .with_extension(new_page.format.extension());

  let page = Page {
    path,
    filepath,
    format: Some(new_page.format),
    user: Some(user.clone()),
  };

  match page.create(new_page.body, &user, state).await {
    Ok(_) => Redirect::to(&url_path).into_response(),
    Err(err) => Error::from(err).into_response(),
  }
}

async fn raw(Path(path): Path<String>, user: Option<User>, state: Arc<State>) -> Response {
  let path = path.strip_prefix("/").unwrap();
  let path = PathBuf::from(path);

  let dir = std::env::current_dir().unwrap();
  let page_path = dbg!(dir.join(&state.config.pages_directory).join(&path));

  let filepath = match find_file(page_path) {
    Ok(path) => path,
    Err(err) => return Error::from(err).into_response(),
  };

  let format = filepath
    .extension()
    .map(|e| e.to_str())
    .flatten()
    .map(|ext| Format::from_extension(ext))
    .flatten();

  let page = Page {
    path,
    filepath,
    format,
    user,
  };

  page.raw().await.into_response()
}

async fn render(body: String, format: Option<Query<QueryFormat>>, state: Arc<State>) -> Response {
  let format = format.map(|query| query.0);

  tokio::task::spawn_blocking(move || {
    let rendered = crate::pandoc::to_html(body, format.map(|f| f.into()), state)?;

    Ok::<_, Error>(Html(rendered))
  })
  .await
  .unwrap()
  .into_response()
}

fn find_file(mut path: PathBuf) -> Result<PathBuf, std::io::Error> {
  if path.is_dir() {
    return Err(std::io::Error::new(
      std::io::ErrorKind::NotFound,
      format!("{:?} is a directory", &path),
    ));
  }

  let name_to_match = path
    .file_stem()
    .ok_or(std::io::Error::new(
      std::io::ErrorKind::NotFound,
      format!("{:?} has no filename", &path),
    ))?
    .to_os_string();

  path.pop();

  for file in std::fs::read_dir(&path)? {
    let file = file?;
    let path = file.path();

    let name = match path.file_stem() {
      Some(name) => name,
      None => continue,
    };

    if name_to_match == name {
      return Ok(file.path());
    }
  }

  return Err(std::io::Error::new(
    std::io::ErrorKind::NotFound,
    format!("{:?} not found", &path),
  ));
}
