use std::{fmt, fmt::Write as _};

use axum::response::Html;
use maud::{html, Escaper, Markup, PreEscaped, Render, DOCTYPE};

use crate::{role::Role, user::User};

#[derive(Clone, Default)]
pub struct Template {
  head: Option<Markup>,
  title: Option<Markup>,
  script: Option<String>,
  tabs: Option<Markup>,
  content: Option<Markup>,
}

impl Template {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn head(mut self, head: Markup) -> Self {
    self.head = Some(head);
    self
  }

  pub fn title(mut self, title: impl Render) -> Self {
    self.title = Some(html! { (title) });
    self
  }

  pub fn script(mut self, script: impl ToString) -> Self {
    self.script = Some(script.to_string());
    self
  }

  pub fn tabs(mut self, tabs: Markup) -> Self {
    self.tabs = Some(tabs);
    self
  }

  pub fn content(mut self, content: Markup) -> Self {
    self.content = Some(content);
    self
  }

  pub fn render(self, user: Option<User>) -> Html<String> {
    let PreEscaped(html) = html! {
      (DOCTYPE)
      meta charset="utf-8";
      html lang="en" {
        head {
          title {
            @if let Some(title) = self.title {
              (title) " - "
            }
            "Title"
          }
          link rel="stylesheet" type="text/css" href="/bundle.css";
          script type="module" src="/bundle.js" {}
          @if let Some(head) = self.head {
            (head)
          }
          @if let Some(script) = self.script {
            script type="module" {
              (PreEscaped(script))
            }
          }
        }

        body {
          #sidebar {
            a href="/" {
              img src="/logo.png";
            }

            fieldset {
              legend { "Site" }
              ul {
                li { a href="/" { "Front page "} }
                li { "All pages" }
                li { a href="/meta/categories" { "Categories" } }
                li { "Random page" }
                li { "Recent activity" }
                @if let Some(user) = &user {
                  @if user.roles.contains(&Role::Administrator) {
                    li { "Admin" }
                  } @else {
                    li { "Regular user" }
                  }
                } @else {
                  li { "Not logged in" }
                }
              }
            }

            fieldset {
              legend { "Settings" }
              label {
                span { "Theme:" }
                select #color-scheme {
                  option value="light" { "Light" }
                  option value="system" { "System" }
                  option value="dark" { "Dark" }
                }
              }
            }
          }

          #header {
            #account {
              @if let Some(user) = &user {
                a href={ "/meta/profile/" (user.email) } {
                  (user.name) "⟨" (user.email) "⟩"
                }
                "·"
                a href="/meta/logout" { "log out" }
              } @else {
                a href="/meta/login" { "log in" }
              }
            }

            @if let Some(tabs) = self.tabs {
              #tabs { (tabs) }
            }
          }

          @if let Some(content) = self.content {
            #content { (content) }
          }

          #footer {
            "© Copyright 2008 by " a href="http://domain.invalid" { "you" }
          }
        }
      }
    };

    Html(html)
  }
}

pub struct PrettyPrint<T: fmt::Debug>(pub T);

impl<T: fmt::Debug> Render for PrettyPrint<T> {
  fn render_to(&self, output: &mut String) {
    let mut escaper = Escaper::new(output);
    write!(escaper, "{:?}", self.0).unwrap();
  }
}
