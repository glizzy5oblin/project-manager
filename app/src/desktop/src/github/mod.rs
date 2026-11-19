//! GitHub account access: sign-in with the OAuth device flow, the token kept in the OS keychain,
//! an optional fallback to the GitHub CLI's login, and an API client that only talks to GitHub.
//!
//! Token rules (from the security checklist):
//! - stored only in the OS keychain ([`token::Keychain`]), or left in `gh` when using its login;
//! - held in memory as [`Token`], whose `Debug` output is redacted and which cannot be
//!   serialised, so it never reaches logs, error messages or the frontend;
//! - sent only as a header to `https://api.github.com` ([`http`]).
//!
//! The OAuth app's client ID comes from `PM_GITHUB_CLIENT_ID`, read at run time and, failing
//! that, at build time. Without one, sign-in reports that it is not set up and the `gh` fallback
//! still works.

mod account;
pub mod device;
pub mod gh;
pub mod http;
pub mod token;

#[cfg(test)]
mod tests;

pub use account::{Account, AuthSource, AuthStatus, GitHub};
pub use device::{DeviceGrant, DevicePrompt};
pub use gh::{GhCli, GhStatus};
pub use http::{Client, Endpoints, RateLimit};
pub use token::{Keychain, Token, TokenStore};

pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The OAuth client ID, if this build or environment has one.
pub fn client_id() -> Option<String> {
    let runtime = std::env::var("PM_GITHUB_CLIENT_ID").ok();
    let built = option_env!("PM_GITHUB_CLIENT_ID").map(str::to_string);
    runtime
        .or(built)
        .map(|s| s.trim().to_string())
        .filter(|s| device::valid_client_id(s))
}

#[derive(Debug, thiserror::Error)]
pub enum GitHubError {
    #[error("GitHub sign-in is not set up: {0}")]
    NotConfigured(String),
    #[error("Not signed in to GitHub")]
    NotSignedIn,
    #[error("GitHub did not accept the saved sign-in; sign in again")]
    Unauthorized,
    #[error("GitHub refused: {0}")]
    Forbidden(String),
    #[error("GitHub could not find that")]
    NotFound,
    #[error("{}", rate_limit_message(*reset_at, *retry_after, unix_now()))]
    RateLimited {
        /// When the hourly limit resets (Unix seconds).
        reset_at: Option<u64>,
        /// Seconds to wait, for secondary (abuse) limits.
        retry_after: Option<u64>,
    },
    #[error("Could not reach GitHub: {0}")]
    Network(String),
    #[error("Unexpected answer from GitHub: {0}")]
    Unexpected(String),
    #[error("The sign-in code expired before it was entered; start again")]
    Expired,
    #[error("Sign-in was declined on GitHub")]
    Denied,
    #[error("{0}")]
    Keychain(String),
    #[error("{0}")]
    Gh(String),
    #[error("Cancelled")]
    Cancelled,
}

pub(crate) fn rate_limit_message(
    reset_at: Option<u64>,
    retry_after: Option<u64>,
    now: u64,
) -> String {
    let wait = retry_after.or_else(|| reset_at.map(|r| r.saturating_sub(now)));
    match wait {
        Some(s) if s < 60 => {
            "GitHub's rate limit was reached; try again in under a minute".to_string()
        }
        Some(s) => {
            let m = s.div_ceil(60);
            format!(
                "GitHub's rate limit was reached; try again in {m} minute{}",
                if m == 1 { "" } else { "s" }
            )
        }
        None => "GitHub's rate limit was reached; try again later".to_string(),
    }
}
