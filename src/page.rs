use std::{path::PathBuf, string::FromUtf8Error, sync::Arc};

use axum::{
  async_trait,
  extract::{rejection::PathRejection, FromRequest, Path, RequestParts},
  http::StatusCode,
  response::{Html, IntoResponse, Redirect, Response},
  Extension,
  Json,
};
use extract_frontmatter::{config::Splitter, Extractor};

use crate::{config::Config, context::Context, pandoc::Format, user::User, State};

#[derive(Debug, thiserror::Error)]
pub enum Error {
  #[error(transparent)]
  TeraError(#[from] tera::Error),
  #[error(transparent)]
  MakeRelativeError(#[from] std::path::StripPrefixError),
  #[error(transparent)]
  Io(#[from] tokio::io::Error),
  #[error(transparent)]
  FrontMatterError(#[from] toml::de::Error),
  #[error(transparent)]
  Git(#[from] crate::git::Error),
  #[error(transparent)]
  Pandoc(#[from] crate::pandoc::Error),
  #[error(transparent)]
  User(#[from] crate::user::Error),
  #[error(transparent)]
  Utf8(#[from] FromUtf8Error),
  #[error(transparent)]
  Path(#[from] PagePathError),
}

impl IntoResponse for Error {
  fn into_response(self) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()).into_response()
  }
}

pub struct Page {
  pub path: PathBuf,
  pub filepath: PathBuf,
  pub format: Option<Format>,
  pub user: Option<User>,
}

#[derive(serde::Serialize)]
pub struct PageContext {
  pub base: Context,
  pub path: PathBuf,
}

impl PageContext {
  pub fn context(&self) -> Result<tera::Context, Error> {
    let base = tera::Context::from_serialize(&self.base)?;
    let mut ctx = tera::Context::from_serialize(self)?;
    ctx.remove("base");
    ctx.extend(base);

    Ok(ctx)
  }
}

#[derive(serde::Deserialize, Debug)]
pub struct FrontMatter {
  pub title: Option<String>,
  pub categories: Option<Vec<String>>,
}

impl Page {
  pub fn relative_path(&self, config: &Config) -> Result<PathBuf, Error> {
    let path = self.filepath.strip_prefix(&config.pages_directory)?;

    Ok(path.to_path_buf())
  }

  pub fn url_path(&self) -> String {
    let path = self.path.with_extension("");

    format!("/{}", path.display())
  }

  pub async fn raw(&self) -> Result<String, Error> {
    let file = tokio::fs::read_to_string(&self.filepath).await?;

    Ok(file)
  }

  pub async fn context(&self) -> Result<(PageContext, String), Error> {
    let file = self.raw().await?;

    Ok(self.context_with(&file)?)
  }

  pub fn context_with(&self, file: &str) -> Result<(PageContext, String), Error> {
    let (data, front_matter) = Extractor::new(Splitter::DelimiterLine("---")).extract(file);
    let data = data.to_string();
    let front_matter: FrontMatter = toml::from_str(&front_matter)?;

    Ok((
      PageContext {
        base: Context {
          title: front_matter
            .title
            .unwrap_or_else(|| self.path.to_string_lossy().to_string()),
          user: self.user.clone(),
        },
        path: self.path.clone(),
      },
      data,
    ))
  }

  pub async fn create(
    &self,
    contents: String,
    user: &User,
    state: Arc<State>,
  ) -> Result<(), Error> {
    // Make sure the page can render without errors
    self
      .renderer_with(&contents, state.clone())
      .await?
      .render()
      .await?;

    tokio::fs::write(&self.filepath, contents).await?;

    state.git.add_file(&self.relative_path(&state.config)?)?;
    state
      .git
      .commit(&format!("[create] {}", self.path.display()), user)?;
    state.git.push()?;

    Ok(())
  }

  pub async fn update(
    &self,
    contents: String,
    user: &User,
    state: Arc<State>,
  ) -> Result<(), Error> {
    // Make sure the page can render without errors
    self
      .renderer_with(&contents, state.clone())
      .await?
      .render()
      .await?;

    let raw = self.raw().await?;

    tokio::fs::write(&self.filepath, contents).await?;

    let git = || -> Result<(), Error> {
      state.git.add_file(&self.relative_path(&state.config)?)?;
      state
        .git
        .commit(&format!("[update] {}", self.path.display()), user)?;
      state.git.push()?;

      Ok(())
    };

    // If any of the `git` commands fail, revert the file on-disk to what it was before.
    match git() {
      Ok(_) => Ok(()),
      Err(err) => {
        tokio::fs::write(&self.filepath, raw).await?;

        Err(err)
      },
    }
  }

  pub async fn renderer(&self, state: Arc<State>) -> Result<PageRender, Error> {
    let file = self.raw().await?;

    self.renderer_with(&file, state).await
  }

  pub async fn renderer_with(&self, file: &str, state: Arc<State>) -> Result<PageRender, Error> {
    let (front_matter, data) = self.context_with(file)?;
    let context = front_matter.context()?;

    let html = tokio::task::spawn_blocking({
      let state = Arc::clone(&state);
      let format = self.format.clone();
      move || crate::pandoc::to_html(data, format, state)
    })
    .await
    .unwrap()?;

    Ok(PageRender {
      context,
      html,
      state,
    })
  }

  pub async fn view_handler(&self, state: Arc<State>) -> Result<Html<String>, Error> {
    let mime = mime_guess::from_path(&self.path).first_or_text_plain();

    log::info!("{:?}: {:?}", self.path, mime.essence_str());

    if mime.type_() != "text" && !state.config.allowed_mime_types.contains(mime.essence_str()) {
      panic!()
    }

    {
      let mut tera = state.tera.lock().unwrap();
      tera.full_reload()?;
    }

    let renderer = self.renderer(state).await?;
    let html = renderer.render().await?;

    Ok(Html(html))
  }

  pub async fn edit_handler(&self, state: Arc<State>) -> Result<Html<String>, Error> {
    {
      let mut tera = state.tera.lock().unwrap();
      tera.full_reload()?;
    }

    let file = self.raw().await?;

    let (front_matter, _) = self.context_with(&file)?;
    let mut context = front_matter.context()?;

    context.insert("supported_formats", &crate::pandoc::VALID_FORMATS_WITH_NAME);

    tokio::task::spawn_blocking(move || {
      let tera = state.tera.lock().unwrap();

      let rendered = tera.render("edit.html", &context)?;

      Ok(Html(rendered))
    })
    .await
    .unwrap()
  }
}

pub async fn history_handler(page: Page, Extension(state): Extension<Arc<State>>) -> Response {
  state
    .git
    .clone()
    .history_listing_handler(&page, state)
    .await
    .into_response()
}

pub mod edit_handler {
  use super::*;

  pub async fn get(page: Page, Extension(state): Extension<Arc<State>>) -> Response {
    page.edit_handler(state).await.into_response()
  }

  pub async fn post(
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
}

pub mod new_handler {
  use super::*;

  pub async fn get(
    Path(path): Path<String>,
    user: Option<User>,
    Extension(state): Extension<Arc<State>>,
  ) -> Result<Response, Error> {
    let path = path.strip_prefix("/").unwrap();

    match find_file(&path, &state.config) {
      Ok(path) => return Ok(Redirect::to(&format!("/{}", path.display())).into_response()),
      Err(err) if err.kind() == std::io::ErrorKind::NotFound => (),
      Err(err) => return Err(Error::from(err)),
    };

    let mut context = tera::Context::new();
    context.insert("path", path);
    context.insert("user", &user);
    context.insert("supported_formats", &crate::pandoc::VALID_FORMATS_WITH_NAME);

    let html = tokio::task::spawn_blocking(move || {
      let tera = state.tera.lock().unwrap();

      let rendered = tera.render("new.html", &context)?;

      Ok::<_, crate::page::Error>(rendered)
    })
    .await
    .unwrap()
    .map(|html| Html(html));

    Ok(html.into_response())
  }

  #[derive(serde::Deserialize)]
  pub struct NewPage {
    body: String,
    format: Format,
  }

  pub async fn post(
    Path(url_path): Path<String>,
    Json(new_page): Json<NewPage>,
    user: User,
    Extension(state): Extension<Arc<State>>,
  ) -> Result<Response, Error> {
    let path = url_path.strip_prefix("/").unwrap();
    let path = PathBuf::from(path);

    let filepath =
      dbg!(state.config.pages_directory.join(&path)).with_extension(new_page.format.extension());

    let page = Page {
      path,
      filepath,
      format: Some(new_page.format),
      user: Some(user.clone()),
    };

    page.create(new_page.body, &user, state).await?;

    Ok(Redirect::to(&page.url_path()).into_response())
  }
}

pub async fn raw_handler(page: Page) -> Response {
  page.raw().await.into_response()
}

pub struct PageRender {
  html: String,
  context: tera::Context,
  state: Arc<State>,
}

impl PageRender {
  pub fn context_mut(&mut self) -> &mut tera::Context {
    &mut self.context
  }

  pub async fn render(mut self) -> Result<String, Error> {
    self.context.insert("html", &self.html);

    tokio::task::spawn_blocking(move || {
      let tera = self.state.tera.lock().unwrap();

      let rendered = tera.render("page.html", &self.context)?;

      Ok(rendered)
    })
    .await
    .unwrap()
  }
}

#[derive(Debug, thiserror::Error)]
pub enum PagePathError {
  #[error(transparent)]
  PathRejection(#[from] PathRejection),
  #[error(transparent)]
  Io(#[from] std::io::Error),
}

impl IntoResponse for PagePathError {
  fn into_response(self) -> Response {
    let code = match self {
      Self::PathRejection(_) => StatusCode::NOT_FOUND,
      Self::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };

    (code, self.to_string()).into_response()
  }
}

#[async_trait]
impl<B> FromRequest<B> for Page
where
  B: Send,
{
  type Rejection = PagePathError;

  async fn from_request(req: &mut RequestParts<B>) -> Result<Self, Self::Rejection> {
    let Extension(state) = Extension::<Arc<State>>::from_request(req)
      .await
      .expect("`State` extension missing");

    let path = axum::extract::Path::<String>::from_request(req).await?;
    let path = path.0;

    let path = path.strip_prefix("/").unwrap();
    let path = PathBuf::from(path);

    let filepath = find_file(&path, &state.config)?;

    let format = filepath
      .extension()
      .map(|e| e.to_str())
      .flatten()
      .map(|ext| Format::from_extension(ext))
      .flatten();

    // We're good to unwrap here because if there's an error, it'll just return `None`.
    let user = Option::<User>::from_request(req).await.unwrap();

    let page = Page {
      path,
      filepath,
      format,
      user,
    };

    Ok(page)
  }
}

pub fn find_file(
  path: impl AsRef<std::path::Path>,
  config: &Config,
) -> Result<PathBuf, std::io::Error> {
  let mut path = config.pages_directory.join(&path);

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
