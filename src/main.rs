#![feature(adt_const_params, error_reporter)]

use std::sync::{Arc, Mutex};

use axum::{
  routing::{get, post},
  Extension,
  Router,
};

use crate::{
  config::{Args, Config},
  git::Git,
  user::UserDb,
};

mod auth;
mod config;
mod error;
mod front_matter;
mod git;
mod page;
mod pandoc;
mod role;
mod route;
mod template;
mod user;

#[derive(Clone)]
pub struct State {
  config: Arc<Config>,
  git: Arc<Git>,
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

  let users = UserDb::new(config.clone()).await?;
  let users = Arc::new(Mutex::new(users));

  let state = State { config, git, users };
  let state = Arc::new(state);

  pandoc::test_output()?;

  // build our application with a route
  let app = Router::new()
    .route("/meta/error", get(error::handler))
    .route("/meta/categories", get(page::categories_handler))
    .route(
      "/meta/login",
      get(auth::login_handler).post(auth::authenticate_handler),
    )
    .route("/meta/login-callback", get(auth::callback_handler))
    .route("/meta/profile/:user", get(user::profile_handler))
    .route(
      "/meta/new/*path",
      get(page::new_handler::get).post(page::new_handler::post),
    )
    .route("/meta/history/*path", get(page::history_handler))
    .route(
      "/meta/edit/*path",
      get(page::edit_handler::get).post(page::edit_handler::post),
    )
    .route("/meta/raw/*path", get(page::raw_handler))
    .route("/meta/render", post(pandoc::render_handler))
    .fallback(get(route::route));

  let app = auth::setup(app, state.clone()).await?;
  let app = app.layer(Extension(state.clone()));

  log::info!("listening on {}", state.config.listen_on);
  axum::Server::bind(&state.config.listen_on)
    .serve(app.into_make_service())
    .await?;

  Ok(())
}
