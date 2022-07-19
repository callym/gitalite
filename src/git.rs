use std::{
  path::Path,
  sync::{Arc, Mutex},
};

use axum::response::{Html, IntoResponse};
use git2::{Cred, ErrorClass as Class, ErrorCode as Code, RemoteCallbacks, Repository, Signature};

use crate::{config::Config, error::Error, page::Page, user::User, State};

pub struct Git {
  repository: Arc<Mutex<Repository>>,
  config: Arc<Config>,
}

#[derive(serde::Serialize)]
pub struct Commit {
  author: String,
  hash: String,
  date: String,
  message: String,
}

impl Git {
  pub fn new(config: Arc<Config>) -> Result<Git, Error> {
    match git2::Repository::open(&config.pages_directory) {
      Ok(repository) => {
        let remotes = repository.remotes()?;
        remotes
          .iter()
          .for_each(|r| log::info!("found remote: {:?}", r));

        return Ok(Git {
          repository: Arc::new(Mutex::new(repository)),
          config,
        });
      },
      Err(err)
        if (err.class(), err.code()) == (Class::Os, Code::NotFound)
          || (err.class(), err.code()) == (Class::Repository, Code::NotFound) =>
      {
        ()
      },
      Err(err) => Err(err)?,
    }

    // Prepare callbacks.
    let mut callbacks = RemoteCallbacks::new();

    callbacks.credentials(|_, username_from_url, _| {
      Cred::ssh_key(
        username_from_url.unwrap(),
        config.pages_git.public_key.as_deref(),
        &config.pages_git.private_key,
        None,
      )
    });

    // Prepare builder.
    let repository = git2::build::RepoBuilder::new()
      .fetch_options({
        let mut opts = git2::FetchOptions::new();
        opts.remote_callbacks(callbacks);
        opts
      })
      .clone(&config.pages_git.repository, &config.pages_directory)?;

    Ok(Git {
      repository: Arc::new(Mutex::new(repository)),
      config,
    })
  }

  pub fn add_file(&self, path: &Path) -> Result<(), Error> {
    let repository = self.repository.lock().unwrap();

    let mut index = repository.index()?;

    index.add_path(path)?;
    index.write()?;

    Ok(())
  }

  pub fn commit(&self, subject: &str, user: &User) -> Result<(), Error> {
    let repository = self.repository.lock().unwrap();

    let mut index = repository.index()?;

    // let signature = repository.signature()?; // Use default user.name and user.email
    let user = Signature::now(&user.name, &user.email)?;

    let oid = index.write_tree()?;
    let parent_commit = find_last_commit(&repository)?;
    let tree = repository.find_tree(oid)?;

    repository.commit(
      Some("HEAD"),      // point HEAD to our new commit
      &user,             // author
      &user,             // committer
      subject,           // commit message
      &tree,             // tree
      &[&parent_commit], // parent commit
    )?;

    Ok(())
  }

  pub fn push(&self) -> Result<(), Error> {
    let repository = self.repository.lock().unwrap();

    dbg!(repository.head()?.resolve()?.shorthand());

    let mut remote = repository.find_remote("origin")?;

    let mut callbacks = RemoteCallbacks::new();

    callbacks.credentials(|_, username_from_url, _| {
      Cred::ssh_key(
        username_from_url.unwrap(),
        self.config.pages_git.public_key.as_deref(),
        &self.config.pages_git.private_key,
        None,
      )
    });

    callbacks.push_update_reference(|_, status| match status {
      Some(err) => Err(git2::Error::new(
        git2::ErrorCode::GenericError,
        git2::ErrorClass::Repository,
        dbg!(err),
      )),
      None => Ok(()),
    });

    let mut options = git2::PushOptions::new();

    options.remote_callbacks(callbacks);

    let head = repository.head()?;
    let head = head.resolve()?;

    let branch_name = head.shorthand().ok_or(git2::Error::new(
      git2::ErrorCode::NotFound,
      git2::ErrorClass::Repository,
      "reference 'HEAD' doesn't point to a branch?",
    ))?;

    remote.push(
      &[format!(
        "refs/heads/{}:refs/heads/{}",
        branch_name, branch_name
      )],
      Some(&mut options),
    )?;

    Ok(())
  }

  pub fn get_file(&self, path: &Path, commit: git2::Oid) -> Result<String, Error> {
    let repository = self.repository.lock().unwrap();

    let path = path.strip_prefix(&self.config.pages_directory).unwrap();

    let commit = repository.find_commit(commit)?;

    let blob = commit.tree()?.get_path(path)?.to_object(&repository)?;
    let blob = blob.as_blob().unwrap().content().to_vec();

    let contents = String::from_utf8(blob)?;

    Ok(contents)
  }

  pub fn file_history(&self, path: &Path, _: &Config) -> Result<Vec<Commit>, Error> {
    let repository = self.repository.lock().unwrap();

    let mut revwalk = repository.revwalk()?;
    revwalk.set_sorting(git2::Sort::TIME)?;
    revwalk.push_head()?;

    let mut commits = Vec::new();

    for id in revwalk {
      let id = id?;
      let commit = repository.find_commit(id)?;

      if commit.parent_count() != 1 {
        continue;
      }

      let tree = commit.tree()?;

      let parent = commit.parent(0)?;
      let parent_tree = parent.tree()?;

      let diff = repository.diff_tree_to_tree(Some(&parent_tree), Some(&tree), None)?;

      for delta in diff.deltas() {
        let delta_path = delta.new_file().path().unwrap();

        if delta_path != path {
          continue;
        }

        let message = commit.message().unwrap().to_string();

        let date = commit.time();
        let date = time::OffsetDateTime::from_unix_timestamp(date.seconds()).unwrap();
        let date = date
          .format(&time::format_description::well_known::Rfc3339)
          .unwrap();

        let author = commit.author().name().unwrap().to_owned();
        let hash = commit.id().to_string();

        commits.push(Commit {
          author,
          hash,
          date,
          message,
        })
      }
    }

    Ok(commits)
  }

  pub async fn history_handler(
    self: Arc<Self>,
    page: &Page,
    revision: String,
    state: Arc<State>,
  ) -> Result<impl IntoResponse, Error> {
    let oid = git2::Oid::from_str(&revision)?;
    let file = state.git.get_file(&page.filepath, oid)?;

    let mut renderer = page.renderer_with(&file, state).await?;
    renderer.context_mut().insert("revision", &revision);

    let html = renderer.render().await?;

    Ok(Html(html))
  }

  pub async fn history_listing_handler(
    self: Arc<Self>,
    page: &Page,
    state: Arc<State>,
  ) -> Result<impl IntoResponse, Error> {
    {
      let mut tera = state.tera.lock().unwrap();
      tera.full_reload()?;
    }

    let (front_matter, _) = page.context().await?;
    let mut context = front_matter.context()?;

    let path = dbg!(page.filepath.canonicalize()?);
    let path = path
      .strip_prefix(dbg!(&self.config.pages_directory))
      .unwrap();
    let path = dbg!(path.to_owned());

    let render = tokio::task::spawn_blocking(move || {
      let file_history = self.file_history(&path, &state.config)?;
      context.insert("commits", &file_history);

      let tera = state.tera.lock().unwrap();

      let rendered = tera.render("history.html", &context)?;

      Ok::<_, Error>(rendered)
    })
    .await
    .unwrap()?;

    Ok(Html(render))
  }
}

fn find_last_commit(repo: &git2::Repository) -> Result<git2::Commit, git2::Error> {
  let obj = repo.head()?.resolve()?.peel(git2::ObjectType::Commit)?;
  obj
    .into_commit()
    .map_err(|_| git2::Error::from_str("Couldn't find commit"))
}
