//! The GitHub token and where it is kept.
//!
//! [`Token`] never shows its value in `Debug` output and has no `Display` or `Serialize`, so it
//! cannot end up in a log line, an error message or an IPC payload by accident. Only the HTTP
//! client reads it, to fill the `Authorization` header of API requests.

#[cfg(test)]
use std::sync::Mutex;

use super::GitHubError;

/// Keychain service name: the app identifier, so the entry is easy to find and remove by hand.
pub const KEYCHAIN_SERVICE: &str = "io.github.thepotato456.projectmanager";
/// Keychain account name. One GitHub account per install.
pub const KEYCHAIN_ACCOUNT: &str = "github.com";

#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    /// Accepts printable ASCII without spaces, up to 1 KB: what GitHub issues. Anything else
    /// (a newline above all) could break the request header it is put in.
    pub fn new(value: impl Into<String>) -> Result<Self, GitHubError> {
        let value = value.into();
        let value = value.trim();
        if value.is_empty() || value.len() > 1024 || !value.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(GitHubError::Unexpected(
                "GitHub returned a token in an unexpected format".into(),
            ));
        }
        Ok(Token(value.to_string()))
    }

    /// The raw value. Only for the `Authorization` header and the keychain.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

/// Somewhere to keep the token between runs.
pub trait TokenStore: Send + Sync {
    fn load(&self) -> Result<Option<Token>, GitHubError>;
    fn save(&self, token: &Token) -> Result<(), GitHubError>;
    /// Remove the token. Succeeds when there was none.
    fn clear(&self) -> Result<(), GitHubError>;
}

impl<T: TokenStore + ?Sized> TokenStore for std::sync::Arc<T> {
    fn load(&self) -> Result<Option<Token>, GitHubError> {
        (**self).load()
    }
    fn save(&self, token: &Token) -> Result<(), GitHubError> {
        (**self).save(token)
    }
    fn clear(&self) -> Result<(), GitHubError> {
        (**self).clear()
    }
}

/// The OS keychain: Keychain Services on macOS, Credential Manager on Windows, the Secret
/// Service (GNOME Keyring, KWallet) on Linux.
pub struct Keychain {
    service: String,
    account: String,
}

impl Keychain {
    pub fn new() -> Self {
        Self::named(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
    }

    pub fn named(service: &str, account: &str) -> Self {
        Keychain {
            service: service.to_string(),
            account: account.to_string(),
        }
    }

    fn entry(&self) -> Result<keyring::Entry, GitHubError> {
        keyring::Entry::new(&self.service, &self.account).map_err(keychain_error)
    }
}

impl Default for Keychain {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenStore for Keychain {
    fn load(&self) -> Result<Option<Token>, GitHubError> {
        match self.entry()?.get_password() {
            Ok(value) => Token::new(value).map(Some).or_else(|_| {
                // A damaged entry is useless; drop it so the user can sign in again.
                self.clear()?;
                Ok(None)
            }),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(keychain_error(e)),
        }
    }

    fn save(&self, token: &Token) -> Result<(), GitHubError> {
        self.entry()?
            .set_password(token.expose())
            .map_err(keychain_error)
    }

    fn clear(&self) -> Result<(), GitHubError> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(keychain_error(e)),
        }
    }
}

/// A readable keychain error. Built by hand per variant: some variants carry the stored secret
/// (`BadEncoding`, `BadDataFormat`), so the error itself is never formatted.
pub(super) fn keychain_error(e: keyring::Error) -> GitHubError {
    let reason = match &e {
        keyring::Error::NoStorageAccess(_) => "access was refused or the keychain is locked",
        keyring::Error::NoDefaultStore => {
            if cfg!(target_os = "linux") {
                "no keychain service is running (install or start GNOME Keyring or KWallet)"
            } else {
                "the system keychain is not available"
            }
        }
        keyring::Error::PlatformFailure(_) => "the system keychain reported an error",
        keyring::Error::BadEncoding(_) | keyring::Error::BadDataFormat(..) => {
            "the saved entry is damaged"
        }
        _ => "the keychain rejected the request",
    };
    let detail = match &e {
        keyring::Error::NoStorageAccess(p) | keyring::Error::PlatformFailure(p) => {
            format!(" ({p})")
        }
        _ => String::new(),
    };
    GitHubError::Keychain(format!(
        "Could not use the system keychain: {reason}{detail}"
    ))
}

/// In-memory store for tests. `fail` makes every call fail, like a locked keychain.
#[cfg(test)]
#[derive(Default)]
pub struct MemoryStore {
    pub token: Mutex<Option<Token>>,
    pub fail: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
impl MemoryStore {
    fn check(&self) -> Result<(), GitHubError> {
        if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
            Err(GitHubError::Keychain("locked".into()))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
impl TokenStore for MemoryStore {
    fn load(&self) -> Result<Option<Token>, GitHubError> {
        self.check()?;
        Ok(self.token.lock().unwrap().clone())
    }

    fn save(&self, token: &Token) -> Result<(), GitHubError> {
        self.check()?;
        *self.token.lock().unwrap() = Some(token.clone());
        Ok(())
    }

    fn clear(&self) -> Result<(), GitHubError> {
        self.check()?;
        *self.token.lock().unwrap() = None;
        Ok(())
    }
}
