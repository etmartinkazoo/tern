//! OAuth sign-in for providers that no longer take passwords (Google, Microsoft), on
//! io-oauth (Pimalaya): authorization code with PKCE through the browser, then silent
//! refreshes. Tokens live in a file only you can read, one per account.

use crate::config::OAuth;
use anyhow::{anyhow, bail, Context, Result};
use io_oauth::client::{await_redirect, Oauth20ClientStd};
use io_oauth::rfc6749::access_token_request::Oauth20AccessTokenRequestParams;
use io_oauth::rfc6749::auth_request::Oauth20AuthRequestParams;
use io_oauth::rfc6749::auth_response::Oauth20AuthParams;
use io_oauth::rfc6749::refresh_access_token::Oauth20AccessTokenRefreshParams;
use io_oauth::rfc6749::state::Oauth20State;
use io_oauth::rfc7636::pkce::Oauth20PkceCodeChallenge;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use url::Url;

pub struct Provider {
    pub auth_url: String,
    pub token_url: String,
    pub scopes: Vec<String>,
    /// Extra authorization parameters (Google only issues a refresh token with these).
    pub extras: Vec<(&'static str, &'static str)>,
    pub loopback_host: &'static str,
}

pub fn provider(o: &OAuth) -> Result<Provider> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let mut p = match o.provider.to_lowercase().as_str() {
        "google" | "gmail" => Provider {
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token_url: "https://oauth2.googleapis.com/token".into(),
            scopes: s(&[
                "https://mail.google.com/",
                "https://www.googleapis.com/auth/calendar",
                "https://www.googleapis.com/auth/carddav",
                "https://www.googleapis.com/auth/meetings.space.created",
            ]),
            extras: vec![("access_type", "offline"), ("prompt", "consent")],
            loopback_host: "127.0.0.1",
        },
        "microsoft" | "outlook" | "office365" => {
            let tenant = o.tenant.clone().unwrap_or_else(|| "common".into());
            Provider {
                auth_url: format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize"),
                token_url: format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token"),
                scopes: s(&[
                    "https://outlook.office.com/IMAP.AccessAsUser.All",
                    "https://outlook.office.com/SMTP.Send",
                    "offline_access",
                ]),
                extras: vec![],
                loopback_host: "localhost",
            }
        }
        "custom" => Provider {
            auth_url: o.auth_url.clone().ok_or_else(|| anyhow!("custom OAuth needs auth_url"))?,
            token_url: o.token_url.clone().ok_or_else(|| anyhow!("custom OAuth needs token_url"))?,
            scopes: vec![],
            extras: vec![],
            loopback_host: "127.0.0.1",
        },
        other => bail!("unknown OAuth provider `{other}` (google, microsoft or custom)"),
    };
    if let Some(sc) = &o.scopes {
        p.scopes = sc.clone();
    }
    if let Some(u) = &o.auth_url {
        p.auth_url = u.clone();
    }
    if let Some(u) = &o.token_url {
        p.token_url = u.clone();
    }
    Ok(p)
}

#[derive(Serialize, Deserialize)]
struct Tokens {
    access_token: String,
    /// Unix seconds.
    expires_at: i64,
    refresh_token: Option<String>,
}

fn token_file(account: &str) -> PathBuf {
    let safe: String = account.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    dirs::data_dir().unwrap_or_default().join("tern/oauth").join(format!("{safe}.json"))
}

fn load(account: &str) -> Option<Tokens> {
    serde_json::from_slice(&std::fs::read(token_file(account)).ok()?).ok()
}

fn save(account: &str, t: &Tokens) -> Result<()> {
    let path = token_file(account);
    std::fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension("json.tmp");
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(&serde_json::to_vec(t)?)?;
    }
    std::fs::rename(tmp, path)?;
    Ok(())
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn client(p: &Provider, o: &OAuth) -> Result<Oauth20ClientStd> {
    let url: Url = p.token_url.parse()?;
    let mut c = Oauth20ClientStd::connect(url, &Default::default(), o.client_id.clone())
        .map_err(|e| anyhow!("connecting to {}: {e}", p.token_url))?;
    c.client_secret = o.secret()?.map(SecretString::from);
    Ok(c)
}

/// A valid access token, refreshed first when it is within a minute of expiring.
pub async fn access_token(account: &str, o: &OAuth) -> Result<String> {
    let t = load(account).ok_or_else(|| anyhow!("{account} isn't signed in — run `tern auth {account}`"))?;
    if t.expires_at - 60 > now() {
        return Ok(t.access_token);
    }
    let refresh = t.refresh_token.clone().ok_or_else(|| anyhow!("{account}: token expired — run `tern auth {account}`"))?;
    let (account, o) = (account.to_string(), o.clone());
    tokio::task::spawn_blocking(move || -> Result<String> {
        let p = provider(&o)?;
        let mut c = client(&p, &o)?;
        let mut params = Oauth20AccessTokenRefreshParams::new(&o.client_id, SecretString::from(refresh.clone()));
        params.client_secret = o.secret()?.map(SecretString::from);
        let ok = c
            .refresh_access_token(params)
            .map_err(|e| anyhow!("refreshing {account}'s token: {e}"))?
            .map_err(|e| anyhow!("{account}: refresh refused ({e:?}) — run `tern auth {account}`"))?;
        let tokens = Tokens {
            access_token: ok.access_token.expose_secret().to_string(),
            expires_at: now() + ok.expires_in.unwrap_or(3600) as i64,
            // Providers may or may not rotate the refresh token.
            refresh_token: ok.refresh_token.map(|r| r.expose_secret().to_string()).or(Some(refresh)),
        };
        save(&account, &tokens)?;
        Ok(tokens.access_token)
    })
    .await?
}

/// The browser sign-in: `tern auth <account>`. Prints the URL, opens it, waits for the
/// redirect on a local port and stores the tokens.
pub fn authorize(account: &str, o: &OAuth) -> Result<()> {
    let p = provider(o)?;
    let port = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    let redirect: Url = format!("http://{}:{port}/", p.loopback_host).parse()?;
    let pkce = Oauth20PkceCodeChallenge::default();
    let state = Oauth20State::new(32);
    let params = Oauth20AuthRequestParams {
        client_id: Cow::Borrowed(&o.client_id),
        redirect_uri: Some(Cow::Owned(redirect.to_string())),
        scope: p.scopes.iter().map(|s| Cow::Owned(s.clone())).collect::<BTreeSet<_>>(),
        state: Some(Cow::Borrowed(&state)),
        pkce_code_challenge: Some(Cow::Borrowed(&pkce)),
        extras: p.extras.iter().map(|(k, v)| (Cow::Borrowed(*k), Cow::Borrowed(*v))).collect::<BTreeMap<_, _>>(),
    };
    let url = params.build_url(&p.auth_url.parse()?);
    println!("Signing in {account} with {}.\n\nOpen this in your browser if it didn't open by itself:\n\n  {url}\n", o.provider);
    let _ = std::process::Command::new("xdg-open")
        .arg(url.as_str())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    let back = await_redirect(&redirect).context("waiting for the browser to come back")?;
    let code = Oauth20AuthParams::from(&back).validate(Some(&state)).map_err(|e| anyhow!("sign-in refused: {e}"))?;
    let mut c = client(&p, o)?;
    let req = Oauth20AccessTokenRequestParams {
        code: Cow::Owned(code.into_owned()),
        redirect_uri: Some(Cow::Owned(redirect.to_string())),
        client_id: Cow::Borrowed(&o.client_id),
        client_secret: o.secret()?.map(SecretString::from),
        pkce_code_verifier: Some(Cow::Borrowed(&pkce.verifier)),
    };
    let ok = c
        .request_access_token(req)
        .map_err(|e| anyhow!("exchanging the code: {e}"))?
        .map_err(|e| anyhow!("token request refused: {e:?}"))?;
    let tokens = Tokens {
        access_token: ok.access_token.expose_secret().to_string(),
        expires_at: now() + ok.expires_in.unwrap_or(3600) as i64,
        refresh_token: ok.refresh_token.map(|r| r.expose_secret().to_string()),
    };
    if tokens.refresh_token.is_none() {
        eprintln!("warning: no refresh token issued; you'll need to sign in again when this one expires");
    }
    save(account, &tokens)?;
    println!("Signed in. Tokens saved to {}", token_file(account).display());
    Ok(())
}
