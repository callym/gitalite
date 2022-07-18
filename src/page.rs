use std::{
  path::PathBuf,
  sync::{Arc, Mutex},
};

use axum::response::{Html, IntoResponse, Redirect};
use extract_frontmatter::{config::Splitter, Extractor};
use tera::Tera;

use crate::{auth::User, config::Config, context::Context, error::Error, pandoc::Format, State};

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

  pub async fn view_handler(&self, state: Arc<State>) -> Result<impl IntoResponse, Error> {
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

  pub async fn edit_handler(&self, state: Arc<State>) -> Result<impl IntoResponse, Error> {
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

  pub async fn update_handler(&self, content: String, state: Arc<State>) -> Result<(), Error> {
    unimplemented!()
  }
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
