use std::sync::Arc;

use axum::{
  extract::Query,
  response::{Html, IntoResponse, Response},
  Extension,
};
use pandoc::{InputKind, OutputFormat, OutputKind, Pandoc, PandocOption, PandocOutput};
use pandoc_ast::MutVisitor;
use serde::{Deserialize, Deserializer};

use crate::State;

#[derive(Debug, thiserror::Error)]
pub enum Error {
  #[error(transparent)]
  PandocError(#[from] pandoc::PandocError),
  #[error("Output from Pandoc is wrong\nExpected:\n{expected}\n\n\nActual:\n{actual}")]
  PandocWrongOutput { expected: String, actual: String },
}

pub const VALID_FORMATS_WITH_NAME: [(&'static str, &'static str); 14] = [
  ("markdown", "Markdown"),
  ("rst", "reStructuredText"),
  ("html", "HTML"),
  ("latex", "LaTeX"),
  ("mediawiki", "MediaWiki"),
  ("textile", "Textile"),
  ("org", "Emacs Org-Mode"),
  ("opml", "OPML"),
  ("docx", ".docx"),
  ("haddock", "Haddock"),
  ("epub", "EPUP"),
  ("docbook", "DocBook"),
  ("t2t", "txt2tags"),
  ("twiki", "TWiki"),
];

const VALID_FORMATS: [&'static str; 14] = [
  "markdown",
  "rst",
  "html",
  "latex",
  "mediawiki",
  "textile",
  "org",
  "opml",
  "docx",
  "haddock",
  "epub",
  "docbook",
  "t2t",
  "twiki",
];

#[derive(serde::Deserialize)]
pub struct QueryFormat {
  format: Format,
}

impl Into<Format> for QueryFormat {
  fn into(self) -> Format {
    self.format
  }
}

#[derive(Debug, Clone)]
pub struct Format(::pandoc::InputFormat);

impl Format {
  pub fn from_extension(extension: &str) -> Option<Self> {
    use ::pandoc::InputFormat;

    match extension {
      "lhs" => Some(Self(InputFormat::Native)),
      "json" => Some(Self(InputFormat::Json)),
      "md" => Some(Self(InputFormat::Markdown)),
      "textile" => Some(Self(InputFormat::Textile)),
      "rst" => Some(Self(InputFormat::Rst)),
      "html" => Some(Self(InputFormat::Html)),
      "dbk" => Some(Self(InputFormat::DocBook)),
      "t2t" => Some(Self(InputFormat::T2t)),
      "docx" => Some(Self(InputFormat::Docx)),
      "epub" => Some(Self(InputFormat::Epub)),
      "opml" => Some(Self(InputFormat::Opml)),
      "org" => Some(Self(InputFormat::Org)),
      "wiki" => Some(Self(InputFormat::MediaWiki)),
      "twiki" => Some(Self(InputFormat::Twiki)),
      "hs" => Some(Self(InputFormat::Haddock)),
      "tex" => Some(Self(InputFormat::Latex)),
      _ => None,
    }
  }

  pub fn extension(&self) -> &'static str {
    use ::pandoc::InputFormat;

    match self.clone().into() {
      InputFormat::Native => "lhs",
      InputFormat::Json => "json",
      InputFormat::Markdown
      | InputFormat::MarkdownStrict
      | InputFormat::MarkdownPhpextra
      | InputFormat::MarkdownGithub
      | InputFormat::Commonmark => "md",
      InputFormat::Textile => "textile",
      InputFormat::Rst => "rst",
      InputFormat::Html => "html",
      InputFormat::DocBook => "dbk",
      InputFormat::T2t => "t2t",
      InputFormat::Docx => "docx",
      InputFormat::Epub => "epub",
      InputFormat::Opml => "opml",
      InputFormat::Org => "org",
      InputFormat::MediaWiki => "wiki",
      InputFormat::Twiki => "twiki",
      InputFormat::Haddock => "hs",
      InputFormat::Latex => "tex",
      other => panic!("Unsupported format: {}", other),
    }
  }
}

impl Into<::pandoc::InputFormat> for Format {
  fn into(self) -> ::pandoc::InputFormat {
    self.0
  }
}

impl<'de> Deserialize<'de> for Format {
  fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
  where
    D: Deserializer<'de>,
  {
    deserializer.deserialize_str(InputFormatVisitor)
  }
}

struct InputFormatVisitor;

impl<'de> serde::de::Visitor<'de> for InputFormatVisitor {
  type Value = Format;

  fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
    formatter.write_str("an input format that `pandoc` recognises")
  }

  fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
  where
    E: serde::de::Error,
  {
    use ::pandoc::InputFormat;
    let format = match v {
      "native" => InputFormat::Native,
      "json" => InputFormat::Json,
      "markdown" => InputFormat::Markdown,
      "markdown_strict" => InputFormat::MarkdownStrict,
      "markdown_phpextra" => InputFormat::MarkdownPhpextra,
      "markdown_github" => InputFormat::MarkdownGithub,
      "commonmark" => InputFormat::Commonmark,
      "rst" => InputFormat::Rst,
      "html" => InputFormat::Html,
      "latex" => InputFormat::Latex,
      "mediawiki" => InputFormat::MediaWiki,
      "textile" => InputFormat::Textile,
      "org" => InputFormat::Org,
      "opml" => InputFormat::Opml,
      "docx" => InputFormat::Docx,
      "haddock" => InputFormat::Haddock,
      "epub" => InputFormat::Epub,
      "docbook" => InputFormat::DocBook,
      "t2t" => InputFormat::T2t,
      "twiki" => InputFormat::Twiki,
      _ => return Err(serde::de::Error::unknown_variant(v, &VALID_FORMATS)),
    };

    Ok(Format(format))
  }
}

pub fn test_output() -> Result<(), Error> {
  let mut pandoc = Pandoc::new();

  pandoc
    .set_input(InputKind::Pipe(String::from("# Hello, world!")))
    .set_output(OutputKind::Pipe)
    .set_output_format(OutputFormat::Html5, vec![]);

  let out = pandoc.execute()?;

  let actual = match out {
    PandocOutput::ToBuffer(buffer) => buffer,
    _ => unreachable!(),
  };

  let expected = String::from("<h1 id=\"hello-world\">Hello, world!</h1>\n");

  if expected != actual {
    Err(Error::PandocWrongOutput { expected, actual })?;
  }

  Ok(())
}

pub fn to_html(doc: String, format: Option<Format>, state: Arc<State>) -> Result<String, Error> {
  let mut pandoc = Pandoc::new();

  if let Some(format) = format {
    pandoc.set_input_format(format.into(), Vec::new());
  }

  pandoc
    .set_input(InputKind::Pipe(doc))
    .set_output(OutputKind::Pipe)
    .set_output_format(OutputFormat::Html5, vec![]);

  pandoc.add_options(&[PandocOption::Katex(None)]);

  pandoc.add_filter(move |json| {
    pandoc_ast::filter(json, {
      let state = Arc::clone(&state);
      |mut pandoc| {
        KatexFilter { state }.walk_pandoc(&mut pandoc);
        pandoc
      }
    })
  });

  let out = pandoc.execute()?;

  let buffer = match out {
    PandocOutput::ToBuffer(buffer) => buffer,
    _ => unreachable!(),
  };

  Ok(buffer)
}

struct KatexFilter {
  state: Arc<State>,
}

impl pandoc_ast::MutVisitor for KatexFilter {
  fn visit_inline(&mut self, inline: &mut pandoc_ast::Inline) {
    if let pandoc_ast::Inline::Math(ty, block) = inline {
      let mut opts = katex::Opts::builder();
      opts.display_mode(*ty == pandoc_ast::MathType::DisplayMath);
      opts.macros(self.state.config.katex_macros.clone());
      opts.throw_on_error(false);
      let opts = opts.build().unwrap();

      let html = katex::render_with_opts(block, opts).unwrap();

      *inline = pandoc_ast::Inline::RawInline(pandoc_ast::Format(String::from("html")), html);
    }
  }
}

pub async fn render_handler(
  body: String,
  format: Option<Query<QueryFormat>>,
  Extension(state): Extension<Arc<State>>,
) -> Result<Response, crate::page::Error> {
  let format = format.map(|query| query.0);

  let html = tokio::task::spawn_blocking(move || {
    let rendered = to_html(body, format.map(|f| f.into()), state)?;

    Ok::<_, crate::page::Error>(Html(rendered))
  })
  .await
  .unwrap()?;

  Ok(html.into_response())
}
