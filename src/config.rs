use std::{
  collections::{HashMap, HashSet},
  net::SocketAddr,
  path::PathBuf,
};

use oauth2::url::Url;

#[derive(clap::Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct Args {
  /// Path to the config file
  #[clap(short, long)]
  pub config: PathBuf,
}

impl Args {
  pub fn parse() -> Self {
    <Self as clap::StructOpt>::parse()
  }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Git {
  pub repository: String,
  pub private_key: PathBuf,
  pub public_key: Option<PathBuf>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct InitialUser {
  pub name: String,
  pub email: String,
  pub url: Url,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Users {
  pub initial: InitialUser,
  pub password: PathBuf,
  pub database: PathBuf,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Config {
  pub listen_on: SocketAddr,
  pub client_id: String,
  pub allowed_mime_types: HashSet<String>,
  pub static_directory: PathBuf,
  pub pages_directory: PathBuf,
  pub pages_git: Git,
  pub templates_directory: PathBuf,
  pub katex_macros: HashMap<String, String>,
  pub postgresql: String,
  pub users: Users,
}

impl Config {
  pub fn canonicalize(&mut self) -> Result<(), std::io::Error> {
    self.pages_directory = self.pages_directory.canonicalize()?;
    self.static_directory = self.static_directory.canonicalize()?;
    self.templates_directory = self.templates_directory.canonicalize()?;

    Ok(())
  }
}
