//! The only way the app talks to GitHub.
//!
//! - Requests go to two fixed origins: `https://github.com` (device-flow sign-in) and
//!   `https://api.github.com` (the REST API). Callers pass a path, never a URL, and the built URL
//!   is checked to still be on the expected origin.
//! - The token is attached only to API requests, as a header, never in a URL.
//! - Redirects are followed only within those two origins; reqwest drops the `Authorization`
//!   header itself when a redirect changes host.
//! - TLS only (HTTPS is enforced outside tests).
//! - The last rate-limit headers are kept; when the limit is used up, requests fail at once
//!   with the reset time instead of being sent.

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use reqwest::blocking::{Client as Http, Response};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, RETRY_AFTER};
use reqwest::{StatusCode, Url, redirect};
use serde::Serialize;
use serde::de::DeserializeOwned;
use ts_rs::TS;

use super::{GitHubError, Token, unix_now};

/// REST API version requested in every API call.
pub const API_VERSION: &str = "2022-11-28";
const USER_AGENT: &str = concat!("ProjectManager/", env!("CARGO_PKG_VERSION"));

/// Where requests go. Outside tests only [`Endpoints::github`] exists.
#[derive(Debug, Clone)]
pub struct Endpoints {
    web: Url,
    api: Url,
    https_only: bool,
    /// Shortest wait between device-flow polls (GitHub's documented minimum is 5 s).
    pub(crate) min_poll: Duration,
}

impl Endpoints {
    pub fn github() -> Self {
        Endpoints {
            web: Url::parse("https://github.com").expect("static URL"),
            api: Url::parse("https://api.github.com").expect("static URL"),
            https_only: true,
            min_poll: Duration::from_secs(5),
        }
    }

    /// A local test server standing in for both origins.
    #[cfg(test)]
    pub(crate) fn local(base: &str) -> Self {
        let url = Url::parse(base).expect("test URL");
        Endpoints {
            web: url.clone(),
            api: url,
            https_only: false,
            min_poll: Duration::ZERO,
        }
    }

    pub fn web(&self) -> &Url {
        &self.web
    }

    /// `url` is on the sign-in (web) origin.
    pub fn is_web(&self, url: &Url) -> bool {
        same_origin(&self.web, url)
    }
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

/// Rate-limit state from GitHub's `x-ratelimit-*` headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RateLimit {
    pub limit: u32,
    pub remaining: u32,
    /// When the window resets, in Unix seconds.
    #[ts(type = "number")]
    pub reset: u64,
}

pub(crate) fn parse_rate_limit(headers: &HeaderMap) -> Option<RateLimit> {
    let num = |name: &str| -> Option<u64> { headers.get(name)?.to_str().ok()?.trim().parse().ok() };
    Some(RateLimit {
        limit: u32::try_from(num("x-ratelimit-limit")?).ok()?,
        remaining: u32::try_from(num("x-ratelimit-remaining")?).ok()?,
        reset: num("x-ratelimit-reset")?,
    })
}

/// A successful API reply: the parsed body and the response headers.
pub struct Reply<T> {
    pub value: T,
    pub headers: HeaderMap,
}

pub struct Client {
    endpoints: Endpoints,
    /// Built on first use: a blocking reqwest client must not be created or dropped on an
    /// async runtime thread, and the first request always happens on a worker thread.
    http: OnceLock<Http>,
    rate: Mutex<Option<RateLimit>>,
}

impl Client {
    pub fn new(endpoints: Endpoints) -> Self {
        Client {
            endpoints,
            http: OnceLock::new(),
            rate: Mutex::new(None),
        }
    }

    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    /// The last rate-limit state seen from the API.
    pub fn rate_limit(&self) -> Option<RateLimit> {
        *self.rate.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn http(&self) -> Result<&Http, GitHubError> {
        if let Some(h) = self.http.get() {
            return Ok(h);
        }
        let allowed = [self.endpoints.web.clone(), self.endpoints.api.clone()];
        let policy = redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 5 {
                attempt.error("too many redirects")
            } else if allowed.iter().any(|a| same_origin(a, attempt.url())) {
                attempt.follow()
            } else {
                let host = attempt.url().host_str().unwrap_or("?").to_string();
                attempt.error(format!("refused a redirect to {host}"))
            }
        });
        let http = Http::builder()
            .user_agent(USER_AGENT)
            .https_only(self.endpoints.https_only)
            .redirect(policy)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| GitHubError::Network(describe(&e)))?;
        Ok(self.http.get_or_init(|| http))
    }

    /// `base` + `path`, refusing anything that would leave `base`'s origin.
    fn url(base: &Url, path: &str) -> Result<Url, GitHubError> {
        let bad = || GitHubError::Unexpected(format!("refused request path {path:?}"));
        if !path.starts_with('/')
            || path.starts_with("//")
            || path.contains('\\')
            || path.contains('#')
            || path.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(bad());
        }
        let url = Url::parse(&format!("{}{path}", base.as_str().trim_end_matches('/')))
            .map_err(|_| bad())?;
        if !same_origin(base, &url) {
            return Err(bad());
        }
        Ok(url)
    }

    /// POST a form to the sign-in origin (no token). Returns the JSON body; GitHub answers
    /// device-flow errors with an `error` field, which the caller interprets.
    pub fn web_post(
        &self,
        path: &str,
        form: &[(&str, &str)],
    ) -> Result<serde_json::Value, GitHubError> {
        let url = Self::url(&self.endpoints.web, path)?;
        let resp = self
            .http()?
            .post(url)
            .header(ACCEPT, "application/json")
            .form(form)
            .send()
            .map_err(|e| GitHubError::Network(describe(&e)))?;
        let status = resp.status();
        let headers = resp.headers().clone();
        let body: Option<serde_json::Value> = resp.json().ok();
        match body {
            Some(v) if status.is_success() || v.get("error").is_some() => Ok(v),
            _ => Err(status_error(status, &headers, None)),
        }
    }

    /// GET an API path with the token.
    pub fn api_get<T: DeserializeOwned>(
        &self,
        path: &str,
        token: &Token,
    ) -> Result<Reply<T>, GitHubError> {
        self.check_rate_limit()?;
        let url = Self::url(&self.endpoints.api, path)?;
        let resp = self
            .http()?
            .get(url)
            .header(ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
            .header(AUTHORIZATION, format!("Bearer {}", token.expose()))
            .send()
            .map_err(|e| GitHubError::Network(describe(&e)))?;
        let resp = self.check(resp)?;
        let headers = resp.headers().clone();
        let value = resp
            .json()
            .map_err(|e| GitHubError::Unexpected(format!("unreadable reply: {}", describe(&e))))?;
        Ok(Reply { value, headers })
    }

    fn check_rate_limit(&self) -> Result<(), GitHubError> {
        match self.rate_limit() {
            Some(r) if r.remaining == 0 && r.reset > unix_now() => Err(GitHubError::RateLimited {
                reset_at: Some(r.reset),
                retry_after: None,
            }),
            _ => Ok(()),
        }
    }

    /// Record rate-limit headers and turn error statuses into [`GitHubError`]s.
    fn check(&self, resp: Response) -> Result<Response, GitHubError> {
        let rate = parse_rate_limit(resp.headers());
        if rate.is_some() {
            *self.rate.lock().unwrap_or_else(|e| e.into_inner()) = rate;
        }
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let headers = resp.headers().clone();
        let message = resp
            .json::<serde_json::Value>()
            .ok()
            .and_then(|v| Some(v.get("message")?.as_str()?.to_string()));
        Err(status_error(status, &headers, message))
    }
}

/// Map an error status to a [`GitHubError`]. `message` is GitHub's own explanation, if any.
fn status_error(status: StatusCode, headers: &HeaderMap, message: Option<String>) -> GitHubError {
    let rate = parse_rate_limit(headers);
    let retry_after = headers
        .get(RETRY_AFTER)
        .and_then(|v| v.to_str().ok()?.trim().parse::<u64>().ok());
    let message = message.map(|m| m.chars().take(300).collect::<String>());
    match status {
        StatusCode::UNAUTHORIZED => GitHubError::Unauthorized,
        StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS
            if rate.is_some_and(|r| r.remaining == 0) || retry_after.is_some() =>
        {
            GitHubError::RateLimited {
                reset_at: rate.filter(|r| r.remaining == 0).map(|r| r.reset),
                retry_after,
            }
        }
        StatusCode::TOO_MANY_REQUESTS => GitHubError::RateLimited {
            reset_at: None,
            retry_after: None,
        },
        StatusCode::FORBIDDEN => GitHubError::Forbidden(
            message.unwrap_or_else(|| "GitHub refused the request".to_string()),
        ),
        StatusCode::NOT_FOUND => GitHubError::NotFound,
        _ => GitHubError::Unexpected(match message {
            Some(m) => format!("HTTP {status}: {m}"),
            None => format!("HTTP {status}"),
        }),
    }
}

/// A reqwest error as text, with its causes but without the URL.
fn describe(e: &reqwest::Error) -> String {
    let mut out = if e.is_timeout() {
        "the request timed out".to_string()
    } else if e.is_connect() {
        "could not connect".to_string()
    } else if e.is_redirect() {
        "bad redirect".to_string()
    } else {
        "request failed".to_string()
    };
    let mut source = std::error::Error::source(e);
    while let Some(s) = source {
        out.push_str(": ");
        out.push_str(&s.to_string());
        source = s.source();
    }
    out
}
