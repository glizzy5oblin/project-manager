//! OAuth device flow (<https://docs.github.com/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps#device-flow>):
//! ask GitHub for a code, show it to the user, who enters it on github.com, and poll until
//! they approve. No client secret and no local web server are needed.

use std::time::Duration;

use reqwest::Url;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::GitHubError;
use super::http::Client;
use super::token::Token;

/// Scopes asked for. `public_repo` covers releases and assets on public repositories, which is
/// where releases are published (two-repo mode publishes from the public repository).
pub const SCOPES: &str = "public_repo";

const CODE_PATH: &str = "/login/device/code";
const TOKEN_PATH: &str = "/login/oauth/access_token";
const GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// What the user needs to finish signing in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct DevicePrompt {
    /// The code to type on GitHub, e.g. `WDJB-MJHT`.
    pub user_code: String,
    /// The page to enter it on (checked to be on github.com).
    pub verification_uri: String,
    /// Seconds until the code expires.
    #[ts(type = "number")]
    pub expires_in: u64,
}

/// A started sign-in. `device_code` is a secret of the flow and stays in the backend.
pub struct DeviceGrant {
    device_code: String,
    interval: Duration,
    pub prompt: DevicePrompt,
}

impl std::fmt::Debug for DeviceGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceGrant")
            .field("interval", &self.interval)
            .field("prompt", &self.prompt)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct CodeReply {
    device_code: Option<String>,
    user_code: Option<String>,
    verification_uri: Option<String>,
    expires_in: Option<u64>,
    interval: Option<u64>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct TokenReply {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
    interval: Option<u64>,
}

/// Client IDs are short ASCII ids like `Iv1.0123abcd` or `Ov23li0123abcd`.
pub fn valid_client_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

/// Errors GitHub reports for a misconfigured OAuth app, as opposed to a user action.
fn config_error(code: &str, description: Option<String>) -> GitHubError {
    let why = match code {
        "device_flow_disabled" => "device flow is not enabled for this app's OAuth registration",
        // An unknown client ID gets a 404 with `{"error": "Not Found"}`.
        "incorrect_client_credentials" | "Not Found" => {
            "GitHub does not recognise this app's client ID"
        }
        "unsupported_grant_type" => "GitHub did not accept the sign-in request",
        _ => "",
    };
    if why.is_empty() {
        let d = description.unwrap_or_default();
        GitHubError::Unexpected(format!("sign-in failed: {code} {d}").trim().to_string())
    } else {
        GitHubError::NotConfigured(why.to_string())
    }
}

/// Step 1: get a user code.
pub fn start(client: &Client, client_id: &str) -> Result<DeviceGrant, GitHubError> {
    let v = client.web_post(CODE_PATH, &[("client_id", client_id), ("scope", SCOPES)])?;
    let r: CodeReply = serde_json::from_value(v)
        .map_err(|_| GitHubError::Unexpected("unreadable sign-in reply".into()))?;
    if let Some(code) = r.error {
        return Err(config_error(&code, r.error_description));
    }
    let (Some(device_code), Some(user_code), Some(uri), Some(expires_in)) =
        (r.device_code, r.user_code, r.verification_uri, r.expires_in)
    else {
        return Err(GitHubError::Unexpected("incomplete sign-in reply".into()));
    };
    // The user is sent to this page, so it must be GitHub's own.
    let url =
        Url::parse(&uri).map_err(|_| GitHubError::Unexpected("bad verification address".into()))?;
    if !client.endpoints().is_web(&url) {
        return Err(GitHubError::Unexpected(format!(
            "refused verification address on {}",
            url.host_str().unwrap_or("?")
        )));
    }
    if user_code.is_empty()
        || user_code.len() > 32
        || !user_code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(GitHubError::Unexpected("bad user code".into()));
    }
    Ok(DeviceGrant {
        device_code,
        interval: Duration::from_secs(r.interval.unwrap_or(5)).max(client.endpoints().min_poll),
        prompt: DevicePrompt {
            user_code,
            verification_uri: url.to_string(),
            expires_in,
        },
    })
}

/// Step 2: poll until the user approves, declines or the code expires. `wait` sleeps between
/// polls and returns `Err(Cancelled)` if the user gave up; tests pass a recorder.
pub fn poll(
    client: &Client,
    client_id: &str,
    grant: &DeviceGrant,
    wait: &mut dyn FnMut(Duration) -> Result<(), GitHubError>,
) -> Result<Token, GitHubError> {
    let mut interval = grant.interval;
    let lifetime = Duration::from_secs(grant.prompt.expires_in);
    let mut waited = Duration::ZERO;
    loop {
        if waited >= lifetime {
            return Err(GitHubError::Expired);
        }
        wait(interval)?;
        waited += interval;
        let v = client.web_post(
            TOKEN_PATH,
            &[
                ("client_id", client_id),
                ("device_code", &grant.device_code),
                ("grant_type", GRANT_TYPE),
            ],
        )?;
        let r: TokenReply = serde_json::from_value(v)
            .map_err(|_| GitHubError::Unexpected("unreadable sign-in reply".into()))?;
        if let Some(t) = r.access_token {
            return Token::new(t);
        }
        match r.error.as_deref() {
            Some("authorization_pending") => {}
            // GitHub sends the new interval; never go below the old one plus 5 s.
            Some("slow_down") => {
                interval = (interval + Duration::from_secs(5))
                    .max(Duration::from_secs(r.interval.unwrap_or(0)));
            }
            Some("expired_token") => return Err(GitHubError::Expired),
            Some("access_denied") => return Err(GitHubError::Denied),
            Some(code) => return Err(config_error(code, r.error_description)),
            None => return Err(GitHubError::Unexpected("incomplete sign-in reply".into())),
        }
    }
}
