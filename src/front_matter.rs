#[derive(serde::Deserialize, Debug, Default)]
pub struct FrontMatter {
  pub title: Option<String>,
  pub categories: Option<Vec<String>>,
}

impl FrontMatter {
  pub const DELIMITER: &'static str = "+++";
}
