use std::{collections::HashSet, path::PathBuf, string::FromUtf8Error, sync::Arc};

use axum::{
  async_trait,
  extract::{rejection::PathRejection, FromRequest, Path, RequestParts},
  http::StatusCode,
  response::{Html, IntoResponse, Redirect, Response},
  Extension,
  Json,
};
use extract_frontmatter::{config::Splitter, Extractor};
use walkdir::WalkDir;

use crate::{
  config::Config,
  error::ErrorPage,
  front_matter::FrontMatter,
  pandoc::Format,
  user::User,
  State,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
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
  #[error("This page is reserved")]
  ReservedPage { url: String },
}

impl IntoResponse for Error {
  fn into_response(self) -> Response {
    match self {
      Self::ReservedPage { url } => ErrorPage::ReservedPage { url }.into_response(),
      _ => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()).into_response(),
    }
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
  pub path: String,
  pub revision: Option<String>,
  pub title: String,
  pub user: Option<User>,
}

impl Page {
  pub fn all(config: &Config) -> impl Iterator<Item = Self> {
    WalkDir::new(&config.pages_directory)
      .into_iter()
      .filter_map(|e| {
        let e = e.ok()?;

        if !e.file_type().is_file() {
          return None;
        }

        let path = e.path().with_extension("");
        let filepath = e.path().to_path_buf();

        let format = e
          .path()
          .extension()
          .map(|ext| Format::from_extension(&ext.to_string_lossy()))
          .flatten();

        match format {
          Some(format) => Some(Self {
            path,
            filepath,
            format: Some(format),
            user: None,
          }),
          None => None,
        }
      })
  }

  pub async fn categories(config: &Config) -> Result<HashSet<String>, Error> {
    let mut categories = HashSet::new();

    for page in Self::all(config) {
      let file = page.raw().await?;
      let (front_matter, _) = page.front_matter(&file)?;

      if let Some(cat) = front_matter.categories {
        categories.extend(cat);
      }
    }

    Ok(categories)
  }

  pub fn check_if_reserved(path: &str) -> Result<(), Error> {
    if path.starts_with("/meta") {
      return Err(Error::ReservedPage {
        url: path.to_string(),
      });
    }

    Ok(())
  }

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

  pub fn front_matter(&self, file: &str) -> Result<(FrontMatter, String), Error> {
    if file.starts_with(FrontMatter::DELIMITER) {
      let (front_matter, data) =
        Extractor::new(Splitter::EnclosingLines(FrontMatter::DELIMITER)).extract(file);
      let data = data.to_string();
      let front_matter = toml::from_str(&front_matter)?;

      Ok((front_matter, data))
    } else {
      Ok((FrontMatter::default(), file.to_string()))
    }
  }

  pub fn context_with(&self, file: &str) -> Result<(PageContext, String), Error> {
    let (front_matter, data) = self.front_matter(file)?;

    Ok((
      PageContext {
        title: front_matter
          .title
          .unwrap_or_else(|| self.path.to_string_lossy().to_string()),
        user: self.user.clone(),
        path: self.path.to_string_lossy().to_string(),
        revision: None,
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
    let (context, data) = self.context_with(file)?;

    let html = tokio::task::spawn_blocking({
      let state = Arc::clone(&state);
      let format = self.format.clone();
      move || crate::pandoc::to_html(data, format, state)
    })
    .await
    .unwrap()?;

    Ok(PageRender { context, html })
  }

  pub async fn view_handler(self, state: Arc<State>) -> Result<Html<String>, Error> {
    let mime = mime_guess::from_path(&self.path).first_or_text_plain();

    log::info!("{:?}: {:?}", self.path, mime.essence_str());

    if mime.type_() != "text" && !state.config.allowed_mime_types.contains(mime.essence_str()) {
      panic!()
    }

    let renderer = self.renderer(state).await?;
    let html = renderer.render().await?;

    Ok(html)
  }

  pub async fn edit_handler(self) -> Result<Html<String>, Error> {
    let file = self.raw().await?;

    let (front_matter, _) = self.context_with(&file)?;

    let tabs = PageTab::Edit.render(front_matter.path);

    let content = maud::html! {
      @if self.user.is_some() {
        #toolbar {
          div {
            select #format {
              option value="auto" selected { "Auto" }
              @for format in crate::pandoc::VALID_FORMATS_WITH_NAME {
                option value=(format.0) { (format.1) }
              }
            }
          }

          div {
            .toggle {
              input #preview-toggle type="checkbox" autocomplete="off";
            }
            label for="preview-toggle" { "Preview" }
          }

          div {
            button #save { "Save" }
          }
        }

        #editor {}
        #preview {}
      } @else {
        "You must be logged in to create new pages!"
      }
    };

    let script = r#"
      import { setup_editor } from '/bundle.js';
      setup_editor();
    "#;

    let template = crate::template::Template::new()
      .tabs(tabs)
      .title(maud::html! { (front_matter.title) " - Edit"})
      .content(content)
      .script(script)
      .render(self.user);

    Ok(template)
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

  pub async fn get(page: Page, _: User) -> Response {
    page.edit_handler().await.into_response()
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

  #[derive(serde::Deserialize)]
  pub struct NewPage {
    body: String,
    format: Format,
  }

  pub async fn get(
    Path(path): Path<String>,
    user: Option<User>,
    Extension(state): Extension<Arc<State>>,
  ) -> Result<Response, Error> {
    Page::check_if_reserved(&path)?;

    let path = path.strip_prefix("/").unwrap();

    match find_file(&path, &state.config) {
      Ok(path) => {
        let path = if path.starts_with(&state.config.pages_directory) {
          path
            .strip_prefix(&state.config.pages_directory)
            .unwrap()
            .to_path_buf()
        } else {
          path
        };

        return Ok(Redirect::to(&format!("/{}", path.display())).into_response());
      },
      Err(err) if err.kind() == std::io::ErrorKind::NotFound => (),
      Err(err) => return Err(Error::from(err)),
    };

    let content = maud::html! {
      .warning { "The page at " (path) " doesn't exist." }
      @if user.is_some() {
        #toolbar {
          div {
            select #format {
              @for format in crate::pandoc::VALID_FORMATS_WITH_NAME {
                option value=(format.0) { (format.1) }
              }
            }
          }

          div {
            .toggle {
              input #preview-toggle type="checkbox" autocomplete="off";
            }
            label for="preview-toggle" { "Preview" }
          }

          div {
            button #save { "Save" }
          }
        }

        #editor {}
        #preview {}
      } @else {
        "You must be logged in to create new pages!"
      }
    };

    let script = r#"
      import { newpage_editor } from '/bundle.js';
      newpage_editor();
    "#;

    let template = crate::template::Template::new()
      .title("Create new page")
      .content(content)
      .script(script)
      .render(user);

    Ok(template.into_response())
  }

  pub async fn post(
    Path(url_path): Path<String>,
    Json(new_page): Json<NewPage>,
    user: User,
    Extension(state): Extension<Arc<State>>,
  ) -> Result<Response, Error> {
    Page::check_if_reserved(&url_path)?;

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

pub async fn categories_handler(
  user: Option<User>,
  Extension(state): Extension<Arc<State>>,
) -> Result<Html<String>, Error> {
  let categories = Page::categories(&state.config).await?;

  let content = maud::html! {
    ul #categories {
      @for category in &categories {
        li { a href={"/meta/category/" (category)} { (category) } }
      }
    }
  };

  let template = crate::template::Template::new()
    .title("Categories")
    .content(content)
    .render(user);

  Ok(template)
}

pub struct PageRender {
  html: String,
  context: PageContext,
}

impl PageRender {
  pub fn context_mut(&mut self) -> &mut PageContext {
    &mut self.context
  }

  pub async fn render(self) -> Result<Html<String>, Error> {
    let tabs = PageTab::View.render(self.context.path);

    let content = maud::html! {
      @if let Some(revision) = self.context.revision {
        .warning { (revision) }
      }
      (maud::PreEscaped(self.html))
    };

    let template = crate::template::Template::new()
      .tabs(tabs)
      .title(self.context.title)
      .content(content)
      .render(self.context.user);

    Ok(template)
  }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PageTab {
  View,
  Edit,
  History,
}

impl PageTab {
  pub fn render(self, path: impl AsRef<str>) -> maud::Markup {
    maud::html! {
      a .active[self == PageTab::View] href={"/" (path)} { "view" }
      a .active[self == PageTab::Edit] href={"/meta/edit/" (path)} { "edit" }
      a .active[self == PageTab::History] href={"/meta/history/" (path)} { "history" }
    }
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

const PATH_PREFIXES_TO_STRIP: [&'static str; 5] = [
  "/meta/new/",
  "/meta/history/",
  "/meta/edit/",
  "/meta/raw/",
  "/",
];

#[async_trait]
impl<B> FromRequest<B> for Page
where
  B: Send,
{
  type Rejection = PagePathError;

  async fn from_request(req: &mut RequestParts<B>) -> Result<Self, Self::Rejection> {
    let state = req
      .extensions()
      .get::<Arc<State>>()
      .expect("`State` extension missing")
      .clone();

    let path = req.uri().path();

    let path = PATH_PREFIXES_TO_STRIP
      .into_iter()
      .find_map(|pre| path.strip_prefix(pre))
      .unwrap_or(path);

    dbg!(&path);

    let path = PathBuf::from(path);

    let filepath = dbg!(find_file(&path, &state.config))?;

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

  dbg!(&path);

  path.pop();

  dbg!(&name_to_match);
  dbg!(&path);

  for file in std::fs::read_dir(&path)? {
    let file = file?;
    let path = file.path();

    let name = match path.file_stem() {
      Some(name) => name,
      None => continue,
    };

    dbg!(&name);

    if name_to_match == name {
      return Ok(file.path());
    }
  }

  return Err(std::io::Error::new(
    std::io::ErrorKind::NotFound,
    format!("{:?} not found", &path),
  ));
}
