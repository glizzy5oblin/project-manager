//! Remote URL handling: credential redaction for anything shown or logged, and validation of
//! URLs the user types into the clone flow.

/// Replace credentials embedded in URLs (`https://user:token@host/...`) with `***`, anywhere in
/// `text`. Used on remote URLs, error messages and stderr before they reach logs or the UI.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find("://") {
        let (before, after) = rest.split_at(pos + 3);
        out.push_str(before);
        // The authority ends at the first '/', whitespace or quote; credentials end at the last
        // '@' inside it.
        let end = after
            .find(|c: char| c == '/' || c.is_whitespace() || c == '\'' || c == '"' || c == '>')
            .unwrap_or(after.len());
        let authority = &after[..end];
        match authority.rfind('@') {
            Some(at) if at > 0 => {
                out.push_str("***");
                out.push_str(&authority[at..]);
            }
            _ => out.push_str(authority),
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UrlError {
    #[error("the URL is empty")]
    Empty,
    #[error("URLs may not start with '-'")]
    LeadingDash,
    #[error("the URL contains control characters or spaces")]
    BadCharacters,
    #[error("the `{0}` transport is not allowed; use https, ssh, git or a local path")]
    Transport(String),
    #[error(
        "the URL contains a password or token; Project Manager uses your git credential helper instead"
    )]
    EmbeddedCredentials,
}

/// Validate a clone URL from user input. Allowed: `https://`, `http://`, `ssh://`, `git://`,
/// `file://`, scp-style `user@host:path` and local paths. Refused: other transports (notably
/// `ext::` and `fd::`, which can run commands), option-like values and embedded passwords.
pub fn validate_clone_url(url: &str) -> Result<(), UrlError> {
    let url = url.trim();
    if url.is_empty() {
        return Err(UrlError::Empty);
    }
    if url.starts_with('-') {
        return Err(UrlError::LeadingDash);
    }
    if url.chars().any(|c| c.is_control() || c == ' ') {
        return Err(UrlError::BadCharacters);
    }
    if let Some(pos) = url.find("::") {
        // `<transport>::<address>` is git's remote-helper syntax (`ext::`, `fd::`, ...).
        let scheme = &url[..pos];
        if !scheme.contains(['/', '\\']) {
            return Err(UrlError::Transport(scheme.to_string()));
        }
    }
    if let Some(pos) = url.find("://") {
        let scheme = url[..pos].to_ascii_lowercase();
        if !matches!(scheme.as_str(), "https" | "http" | "ssh" | "git" | "file") {
            return Err(UrlError::Transport(scheme));
        }
        let authority = url[pos + 3..].split('/').next().unwrap_or("");
        if let Some((userinfo, _)) = authority.rsplit_once('@')
            && userinfo.contains(':')
        {
            return Err(UrlError::EmbeddedCredentials);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_userinfo() {
        assert_eq!(
            redact("https://user:ghp_secret@github.com/a/b.git"),
            "https://***@github.com/a/b.git"
        );
        assert_eq!(
            redact("https://ghp_secret@github.com/a/b"),
            "https://***@github.com/a/b"
        );
        assert_eq!(
            redact("fatal: unable to access 'https://x:y@host/r.git/': 403"),
            "fatal: unable to access 'https://***@host/r.git/': 403"
        );
        assert_eq!(
            redact("two https://a:b@h1/x and ssh://c@h2/y"),
            "two https://***@h1/x and ssh://***@h2/y"
        );
    }

    #[test]
    fn leaves_plain_text_alone() {
        for s in [
            "https://github.com/a/b.git",
            "git@github.com:a/b.git",
            "no url here",
            "",
            "email me@example.com",
        ] {
            assert_eq!(redact(s), s);
        }
    }

    #[test]
    fn accepts_normal_clone_urls() {
        for u in [
            "https://github.com/a/b.git",
            "http://localhost:8080/r.git",
            "ssh://git@github.com/a/b.git",
            "git@github.com:a/b.git",
            "git://example.com/r.git",
            "file:///srv/git/r.git",
            "/srv/git/r",
            "../r",
            "C:\\repos\\r",
            "https://user@github.com/a/b.git",
        ] {
            assert_eq!(validate_clone_url(u), Ok(()), "{u}");
        }
    }

    #[test]
    fn refuses_dangerous_clone_urls() {
        assert!(matches!(
            validate_clone_url("ext::sh -c touch% /tmp/x"),
            Err(UrlError::BadCharacters)
        ));
        assert!(matches!(
            validate_clone_url("ext::sh"),
            Err(UrlError::Transport(t)) if t == "ext"
        ));
        assert!(matches!(
            validate_clone_url("fd::17"),
            Err(UrlError::Transport(_))
        ));
        assert!(matches!(
            validate_clone_url("--upload-pack=touch /tmp/x"),
            Err(UrlError::LeadingDash)
        ));
        assert!(matches!(
            validate_clone_url("ftp://example.com/r"),
            Err(UrlError::Transport(_))
        ));
        assert_eq!(
            validate_clone_url("https://u:tok@github.com/a/b"),
            Err(UrlError::EmbeddedCredentials)
        );
        assert_eq!(validate_clone_url("  "), Err(UrlError::Empty));
        assert_eq!(
            validate_clone_url("https://x/\nrepo"),
            Err(UrlError::BadCharacters)
        );
    }
}
