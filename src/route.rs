use std::{path::PathBuf, sync::Arc};

use axum::{
  extract::{FromRequest, RequestParts},
  http::{header, Request},
  response::{IntoResponse, Redirect, Response},
  Extension,
};

use crate::{
  page::{Page, PagePathError},
  State,
};

#[derive(serde::Deserialize)]
struct RouteQuery {
  revision: Option<String>,
}

pub async fn route<T: Send>(request: Request<T>) -> Result<Response, crate::page::Error> {
  let path = request.uri().path();
  let path = path.strip_prefix("/").unwrap();
  let path = urlencoding::decode(path)?;
  let path = PathBuf::from(path.to_string());

  let query = request.uri().query().unwrap_or("");
  let query = serde_qs::from_str::<RouteQuery>(query).unwrap();

  let mut parts = RequestParts::new(request);

  let Extension(state) = Extension::<Arc<State>>::from_request(&mut parts)
    .await
    .expect("`State` extension missing");

  let static_path = state.config.static_directory.join(&path);
  if static_path.is_file() {
    return static_handler(&static_path).await;
  }

  let page = match Page::from_request(&mut parts).await {
    Ok(page) => page,
    Err(PagePathError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {
      return Ok(Redirect::to(&format!("/meta/new/{}", path.display())).into_response());
    },
    Err(err) => return Err(crate::page::Error::Path(err)),
  };

  if let Some(revision) = query.revision {
    let html = state
      .git
      .clone()
      .history_handler(&page, revision, state)
      .await?;

    return Ok(html.into_response());
  }

  let html = page.view_handler(state.clone()).await?;

  Ok(html.into_response())
}

async fn static_handler(path: &std::path::Path) -> Result<Response, crate::page::Error> {
  let mime = mime_guess::from_path(path).first_or_text_plain();

  let file = tokio::fs::read(path).await?;

  let response = (
    [(header::CONTENT_TYPE, mime.essence_str().to_string())],
    file,
  )
    .into_response();

  Ok(response)
}
