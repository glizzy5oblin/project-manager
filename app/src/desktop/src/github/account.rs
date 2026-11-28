//! The signed-in GitHub account: which token to use, checking it, signing in and out.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::device::{self, DeviceGrant};
use super::gh::{GhCli, GhStatus};
use super::http::{Client, Endpoints, RateLimit, Reply};
use super::token::{Keychain, Token, TokenStore};
use super::{GitHubError, client_id};
use crate::projects::{read_json, write_json};

/// Where the token comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum AuthSource {
    /// Signed in through Project Manager; the token is in the OS keychain.
    App,
    /// The GitHub CLI's login.
    GhCli,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub login: String,
    pub name: Option<String>,
    /// OAuth scopes the token has (empty when GitHub does not say, e.g. fine-grained tokens).
    pub scopes: Vec<String>,
    pub source: AuthSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub account: Option<Account>,
    /// This build can sign in with the device flow (it has an OAuth client ID).
    pub can_sign_in: bool,
    /// Present when the GitHub CLI was checked.
    pub gh: Option<GhStatus>,
    pub rate_limit: Option<RateLimit>,
    /// Something the user should know, e.g. that a saved sign-in was revoked.
    pub notice: Option<String>,
}

/// Saved choice of token source. Holds no secrets.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Prefs {
    source: Option<AuthSource>,
}

#[derive(Default)]
struct Cached {
    token: Option<(AuthSource, Token)>,
    account: Option<Account>,
}

#[derive(Deserialize)]
struct UserReply {
    login: String,
    name: Option<String>,
}

pub struct GitHub {
    client: Client,
    store: Box<dyn TokenStore>,
    gh: GhCli,
    client_id: Option<String>,
    prefs_path: PathBuf,
    cached: Mutex<Cached>,
}

impl GitHub {
    pub fn new(
        client: Client,
        store: Box<dyn TokenStore>,
        gh: GhCli,
        client_id: Option<String>,
        prefs_path: PathBuf,
    ) -> Self {
        GitHub {
            client,
            store,
            gh,
            client_id,
            prefs_path,
            cached: Mutex::new(Cached::default()),
        }
    }

    /// The real thing: github.com, the OS keychain, `gh` on PATH, preferences in `data_dir`.
    pub fn production(data_dir: &std::path::Path) -> Self {
        Self::new(
            Client::new(Endpoints::github()),
            Box::new(Keychain::new()),
            GhCli::from_env(),
            client_id(),
            data_dir.join("github.json"),
        )
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, Cached> {
        self.cached.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn prefs(&self) -> Prefs {
        read_json(&self.prefs_path)
    }

    fn set_source(&self, source: Option<AuthSource>) -> Result<(), GitHubError> {
        write_json(&self.prefs_path, &Prefs { source })
            .map_err(|e| GitHubError::Unexpected(format!("could not save settings: {e}")))
    }

    /// The token to use: cached, else from the chosen source.
    fn token(&self) -> Result<(AuthSource, Token), GitHubError> {
        if let Some(t) = self.cache().token.clone() {
            return Ok(t);
        }
        let t = match self.prefs().source {
            Some(AuthSource::GhCli) => (AuthSource::GhCli, self.gh.token()?),
            _ => (
                AuthSource::App,
                self.store.load()?.ok_or(GitHubError::NotSignedIn)?,
            ),
        };
        self.cache().token = Some(t.clone());
        Ok(t)
    }

    fn fetch_account(&self, source: AuthSource, token: &Token) -> Result<Account, GitHubError> {
        let Reply { value, headers } = self.client.api_get::<UserReply>("/user", token)?;
        let scopes = headers
            .get("x-oauth-scopes")
            .and_then(|v| v.to_str().ok())
            .map(|s| {
                s.split(',')
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        Ok(Account {
            login: value.login,
            name: value.name.filter(|n| !n.is_empty()),
            scopes,
            source,
        })
    }

    /// The account state. With `check`, the token is validated against GitHub (and a revoked
    /// app token removed from the keychain); with `with_gh`, the GitHub CLI is probed too.
    pub fn status(&self, check: bool, with_gh: bool) -> AuthStatus {
        let mut notice = None;
        let account = if check {
            match self.token() {
                Ok((source, token)) => match self.fetch_account(source, &token) {
                    Ok(a) => {
                        self.cache().account = Some(a.clone());
                        Some(a)
                    }
                    Err(GitHubError::Unauthorized) => {
                        *self.cache() = Cached::default();
                        notice = Some(match source {
                            AuthSource::App => {
                                let _ = self.store.clear();
                                "Your GitHub sign-in expired or was revoked; sign in again"
                                    .to_string()
                            }
                            AuthSource::GhCli => {
                                "GitHub rejected the GitHub CLI's login; run `gh auth login` again"
                                    .to_string()
                            }
                        });
                        None
                    }
                    // Offline or rate limited: keep what we knew and say why it is unchecked.
                    Err(e) => {
                        notice = Some(e.to_string());
                        self.cache().account.clone()
                    }
                },
                Err(GitHubError::NotSignedIn) => None,
                Err(e) => {
                    notice = Some(e.to_string());
                    None
                }
            }
        } else {
            self.cache().account.clone()
        };
        AuthStatus {
            account,
            can_sign_in: self.client_id.is_some(),
            gh: with_gh.then(|| self.gh.status()),
            rate_limit: self.client.rate_limit(),
            notice,
        }
    }

    /// Start the device flow: returns the code to show the user.
    pub fn sign_in_start(&self) -> Result<DeviceGrant, GitHubError> {
        let id = self.client_id.as_deref().ok_or_else(|| {
            GitHubError::NotConfigured("this build has no OAuth client ID".into())
        })?;
        device::start(&self.client, id)
    }

    /// Finish the device flow: wait for approval, check the token, keep it in the keychain.
    pub fn sign_in_finish(
        &self,
        grant: &DeviceGrant,
        wait: &mut dyn FnMut(Duration) -> Result<(), GitHubError>,
    ) -> Result<Account, GitHubError> {
        let id = self.client_id.as_deref().ok_or_else(|| {
            GitHubError::NotConfigured("this build has no OAuth client ID".into())
        })?;
        let token = device::poll(&self.client, id, grant, wait)?;
        let account = self.fetch_account(AuthSource::App, &token)?;
        self.store.save(&token)?;
        self.set_source(Some(AuthSource::App))?;
        *self.cache() = Cached {
            token: Some((AuthSource::App, token)),
            account: Some(account.clone()),
        };
        Ok(account)
    }

    /// Use the GitHub CLI's login from now on.
    pub fn use_gh(&self) -> Result<Account, GitHubError> {
        let token = self.gh.token()?;
        let account = self.fetch_account(AuthSource::GhCli, &token)?;
        self.set_source(Some(AuthSource::GhCli))?;
        *self.cache() = Cached {
            token: Some((AuthSource::GhCli, token)),
            account: Some(account.clone()),
        };
        Ok(account)
    }

    /// Forget the account. An app sign-in is deleted from the keychain; a `gh` login is left
    /// alone (it belongs to `gh`). The token stays valid on GitHub until revoked there.
    pub fn sign_out(&self) -> Result<(), GitHubError> {
        if self.prefs().source != Some(AuthSource::GhCli) {
            self.store.clear()?;
        }
        *self.cache() = Cached::default();
        self.set_source(None)
    }

    /// Run `f` with the signed-in user's token. A rejected token is dropped from the cache so
    /// the next status check reports it.
    fn with_token<T>(
        &self,
        f: impl FnOnce(&Token) -> Result<T, GitHubError>,
    ) -> Result<T, GitHubError> {
        let (_, token) = self.token()?;
        let r = f(&token);
        if matches!(r, Err(GitHubError::Unauthorized)) {
            self.cache().token = None;
        }
        r
    }

    /// GET an API path as the signed-in user. A rejected token is dropped from the cache so the
    /// next status check reports it.
    pub fn api_get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<Reply<T>, GitHubError> {
        self.with_token(|token| self.client.api_get(path, token))
    }

    /// POST a JSON body as the signed-in user (create a resource).
    pub fn post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &impl Serialize,
    ) -> Result<Reply<T>, GitHubError> {
        self.with_token(|token| self.client.api_post(path, token, body))
    }

    /// PATCH a JSON body as the signed-in user (edit a resource).
    pub fn patch<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &impl Serialize,
    ) -> Result<Reply<T>, GitHubError> {
        self.with_token(|token| self.client.api_patch(path, token, body))
    }

    /// DELETE an API path as the signed-in user.
    pub fn delete(&self, path: &str) -> Result<(), GitHubError> {
        self.with_token(|token| self.client.api_delete(path, token))
    }

    /// POST raw bytes with the given content type as the signed-in user (e.g. an asset upload).
    pub fn upload<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        content_type: &str,
        data: &[u8],
    ) -> Result<Reply<T>, GitHubError> {
        self.with_token(|token| self.client.api_upload(path, token, content_type, data))
    }
}
