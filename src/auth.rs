//! How an account proves who it is: a password, or an OAuth access token for providers
//! (Google, Microsoft) that no longer accept passwords.

use crate::config::{Account, Secret};
use anyhow::Result;

pub enum Login {
    Password { user: String, password: String },
    Token { user: String, token: String },
}

#[derive(Clone, Copy)]
pub enum Proto {
    Imap,
    Smtp,
}

/// Credentials for one protocol of an account. OAuth accounts get a fresh access token
/// (refreshed when it is about to expire); the rest use their password.
pub async fn login(acct: &Account, proto: Proto) -> Result<Login> {
    let (user, secret): (&str, &Secret) = match proto {
        Proto::Imap => (&acct.imap.username, &acct.imap.secret),
        Proto::Smtp => (&acct.smtp.username, &acct.smtp.secret),
    };
    if let Some(o) = &acct.oauth {
        let token = crate::oauth::access_token(&acct.name, o).await?;
        return Ok(Login::Token { user: user.to_string(), token });
    }
    Ok(Login::Password { user: user.to_string(), password: secret.resolve()? })
}
