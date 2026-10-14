//! Video-call links for events: a fresh Google Meet space through the Meet REST API, or
//! your personal Zoom room.

use crate::config::Config;
use anyhow::{anyhow, bail, Result};

/// The Google account Meet links are made with: `meet_account`, else the first account
/// signing in with Google OAuth.
fn google_account(cfg: &Config) -> Result<&crate::config::Account> {
    let pick = |a: &&crate::config::Account| a.oauth.as_ref().is_some_and(|o| o.provider.eq_ignore_ascii_case("google"));
    match &cfg.ui.meet_account {
        Some(name) => cfg
            .accounts
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(name))
            .filter(|a| a.oauth.is_some())
            .ok_or_else(|| anyhow!("Meet account `{name}` isn't an OAuth (Google) account")),
        None => cfg
            .accounts
            .iter()
            .find(pick)
            .ok_or_else(|| anyhow!("Meet links need a Google account signed in with OAuth (Settings → Accounts)")),
    }
}

/// Create a Meet space and return its join link.
pub async fn create_meet(cfg: &Config) -> Result<String> {
    let acct = google_account(cfg)?;
    let token = crate::oauth::access_token(&acct.name, acct.oauth.as_ref().unwrap()).await?;
    // Overridable so the request can be exercised against a local stand-in.
    let endpoint = std::env::var("TERN_MEET_ENDPOINT").unwrap_or_else(|_| "https://meet.googleapis.com/v2/spaces".into());
    let resp = reqwest::Client::new()
        .post(&endpoint)
        .bearer_auth(token)
        .json(&serde_json::json!({}))
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| anyhow!("reaching Google Meet: {e}"))?;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    if status.as_u16() == 403 || status.as_u16() == 401 {
        let why = body["error"]["message"].as_str().unwrap_or("not allowed");
        bail!(
            "Google refused ({status}: {why}). Enable the Google Meet REST API in your Cloud project, then run `tern auth {}` again to allow creating meetings",
            acct.name
        );
    }
    if !status.is_success() {
        bail!("Google Meet: {status} {}", body["error"]["message"].as_str().unwrap_or(""));
    }
    body["meetingUri"].as_str().map(String::from).ok_or_else(|| anyhow!("Google Meet answered without a link"))
}

pub fn zoom(cfg: &Config) -> Result<String> {
    cfg.ui
        .zoom_link
        .clone()
        .filter(|l| !l.is_empty())
        .ok_or_else(|| anyhow!("add your Zoom link under Settings → General → Zoom link"))
}

/// The event with a call link: in Where (unless something is there already), as its
/// URL, and at the top of the notes.
pub fn attach(d: &mut crate::when::Draft, link: &str, label: &str) {
    if d.location.trim().is_empty() || d.location.eq_ignore_ascii_case("meet") || d.location.eq_ignore_ascii_case("zoom") {
        d.location = link.to_string();
    }
    d.url = link.to_string();
    if !d.description.contains(link) {
        let rest = d.description.trim();
        d.description = if rest.is_empty() { format!("Join {label}: {link}") } else { format!("Join {label}: {link}\n\n{rest}") };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attaches_once() {
        let mut d = crate::when::Draft { title: "x".into(), location: "zoom".into(), description: "agenda".into(), ..Default::default() };
        attach(&mut d, "https://zoom.us/j/1", "Zoom");
        attach(&mut d, "https://zoom.us/j/1", "Zoom");
        assert_eq!(d.location, "https://zoom.us/j/1");
        assert_eq!(d.url, "https://zoom.us/j/1");
        assert_eq!(d.description, "Join Zoom: https://zoom.us/j/1\n\nagenda");
        let mut d = crate::when::Draft { location: "Room 4".into(), ..Default::default() };
        attach(&mut d, "https://meet.google.com/a", "Meet");
        assert_eq!(d.location, "Room 4");
    }
}
