# Gitalite

`gitalite` is a git-based wiki server/engine/thing.

The main idea is that the git repository should be plain text files, in regular formats that you already know how to use.

It draws inspiration from:
  * [deadwiki](https://github.com/xvxx/deadwiki)
  * [gitit](https://github.com/jgm/gitit)
  * [dokuwiki](https://dokuwiki.org)
  * many, many others
  
  It uses [IndieAuth](https://indieweb.org/IndieAuth) as the authentication protocol, as I didn't want to have to re-implement an auth system just for this.
  
  It currently depends on a postgres database just for the [async-sqlx-session](https://github.com/jbr/async-sqlx-session) dependency - this could probably be switched to sqlite super easily, but I already had a pg database running, so it was no big deal for me.
  
