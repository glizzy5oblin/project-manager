//! GitHub module tests. Network tests run against a scripted HTTP server on 127.0.0.1 that
//! stands in for both github.com and api.github.com; nothing here talks to the real GitHub
//! except the ignored keychain round trip, which only touches the local keychain.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::device::{self, valid_client_id};
use super::gh::parse_version;
use super::http::{Client, Endpoints};
use super::token::{MemoryStore, Token, TokenStore, keychain_error};
use super::*;
use crate::ipc::{AppError, ErrorKind};

const SECRET: &str = "gho_s3cretTOKENvalue0123456789";

// ---------------------------------------------------------------------------------------------
// Mock server

#[derive(Debug, Clone)]
struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn form(&self, key: &str) -> Option<String> {
        self.body.split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            (k == key).then(|| v.replace("%3A", ":").replace('+', " "))
        })
    }
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

fn json(status: u16, body: serde_json::Value) -> Reply {
    Reply {
        status,
        headers: vec![("Content-Type".into(), "application/json".into())],
        body: body.to_string(),
    }
}

impl Reply {
    fn with(mut self, name: &str, value: impl ToString) -> Self {
        self.headers.push((name.into(), value.to_string()));
        self
    }
}

/// Answers each request with the next scripted reply (404 when the script runs out) and
/// records what it received.
struct Server {
    base: String,
    log: Arc<Mutex<Vec<Request>>>,
}

impl Server {
    fn start(replies: Vec<Reply>) -> Server {
        Self::start_with(|_| replies)
    }

    /// For scripts that must name the server's own address.
    fn start_with(script: impl FnOnce(&str) -> Vec<Reply>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let replies = script(&base);
        Self::serve(listener, base, replies)
    }

    fn serve(listener: TcpListener, base: String, replies: Vec<Reply>) -> Server {
        let log = Arc::new(Mutex::new(Vec::new()));
        let replies = Arc::new(Mutex::new(VecDeque::from(replies)));
        let log_t = log.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let Some(req) = read_request(&mut stream) else {
                    continue;
                };
                log_t.lock().unwrap().push(req);
                let reply = replies
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or_else(|| json(404, serde_json::json!({"message": "Not Found"})));
                let mut out = format!("HTTP/1.1 {} X\r\n", reply.status);
                for (k, v) in &reply.headers {
                    out.push_str(&format!("{k}: {v}\r\n"));
                }
                out.push_str(&format!(
                    "Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    reply.body.len(),
                    reply.body
                ));
                let _ = stream.write_all(out.as_bytes());
            }
        });
        Server { base, log }
    }

    fn requests(&self) -> Vec<Request> {
        self.log.lock().unwrap().clone()
    }

    fn client(&self) -> Client {
        Client::new(Endpoints::local(&self.base))
    }
}

fn read_request(stream: &mut std::net::TcpStream) -> Option<Request> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        reader.read_line(&mut h).ok()?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        let (k, v) = h.split_once(':')?;
        headers.push((k.trim().to_string(), v.trim().to_string()));
    }
    let len: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0; len];
    reader.read_exact(&mut body).ok()?;
    Some(Request {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

fn token() -> Token {
    Token::new(SECRET).unwrap()
}

fn user(login: &str) -> Reply {
    json(200, serde_json::json!({"login": login, "name": "Some One"}))
        .with("x-oauth-scopes", "public_repo, read:org")
        .with("x-ratelimit-limit", 5000)
        .with("x-ratelimit-remaining", 4999)
        .with("x-ratelimit-reset", unix_now() + 3600)
}

fn code_reply(base: &str) -> Reply {
    json(
        200,
        serde_json::json!({
            "device_code": "dev-code-123",
            "user_code": "WDJB-MJHT",
            "verification_uri": format!("{base}/login/device"),
            "expires_in": 900,
            "interval": 5,
        }),
    )
}

fn oauth_error(code: &str) -> Reply {
    json(200, serde_json::json!({"error": code}))
}

/// A `wait` that records each requested pause instead of sleeping.
fn recorder(waits: &mut Vec<Duration>) -> impl FnMut(Duration) -> Result<(), GitHubError> + '_ {
    |d| {
        waits.push(d);
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Token

#[test]
fn token_debug_is_redacted() {
    let t = token();
    let shown = format!("{t:?} {:?}", Some(&t));
    assert!(!shown.contains(SECRET), "{shown}");
    assert!(shown.contains("redacted"));
}

#[test]
fn token_accepts_trimmed_ascii_and_rejects_the_rest() {
    assert_eq!(
        Token::new(format!("  {SECRET}\n")).unwrap().expose(),
        SECRET
    );
    for bad in [
        "",
        "   ",
        "abc def",
        "abc\r\nX-Evil: 1",
        "tökén",
        &"a".repeat(1025),
    ] {
        assert!(Token::new(bad).is_err(), "{bad:?} accepted");
    }
}

#[test]
fn keychain_errors_never_include_the_stored_value() {
    let errs = [
        keyring::Error::BadEncoding(SECRET.as_bytes().to_vec()),
        keyring::Error::BadDataFormat(SECRET.as_bytes().to_vec(), "bad".into()),
        keyring::Error::NoStorageAccess("locked".into()),
        keyring::Error::NoDefaultStore,
    ];
    for e in errs {
        let text = keychain_error(e).to_string();
        assert!(!text.contains(SECRET), "{text}");
        assert!(
            text.starts_with("Could not use the system keychain"),
            "{text}"
        );
    }
}

#[test]
fn errors_never_include_the_token() {
    // Every error that can come out of a request made with the token.
    let server = Server::start(vec![
        json(401, serde_json::json!({"message": "Bad credentials"})),
        json(
            403,
            serde_json::json!({"message": "Resource not accessible"}),
        ),
        json(500, serde_json::json!({"message": "oops"})),
    ]);
    let client = server.client();
    for _ in 0..3 {
        let e = client
            .api_get::<serde_json::Value>("/user", &token())
            .err()
            .unwrap();
        let app: AppError = e.into();
        let text = format!("{app:?}");
        assert!(!text.contains(SECRET), "{text}");
    }
}

// ---------------------------------------------------------------------------------------------
// HTTP client

#[test]
fn api_get_sends_the_expected_headers() {
    let server = Server::start(vec![user("octocat")]);
    let reply = server
        .client()
        .api_get::<serde_json::Value>("/user", &token())
        .unwrap();
    assert_eq!(reply.value["login"], "octocat");
    let reqs = server.requests();
    assert_eq!(reqs.len(), 1);
    let r = &reqs[0];
    assert_eq!((r.method.as_str(), r.path.as_str()), ("GET", "/user"));
    assert_eq!(
        r.header("authorization"),
        Some(&*format!("Bearer {SECRET}"))
    );
    assert_eq!(r.header("accept"), Some("application/vnd.github+json"));
    assert_eq!(r.header("x-github-api-version"), Some("2022-11-28"));
    assert!(
        r.header("user-agent")
            .unwrap()
            .starts_with("ProjectManager/")
    );
}

#[test]
fn api_get_refuses_paths_that_could_leave_the_api_host() {
    let server = Server::start(vec![]);
    let client = server.client();
    for path in [
        "user",
        "//evil.example/x",
        "/a\\b",
        "/a b",
        "/a\nb",
        "/x#frag",
        "@evil.example/x",
        "https://evil.example/x",
    ] {
        let err = client
            .api_get::<serde_json::Value>(path, &token())
            .err()
            .unwrap_or_else(|| panic!("{path:?} accepted"));
        assert!(
            matches!(err, GitHubError::Unexpected(_)),
            "{path:?}: {err:?}"
        );
    }
    assert!(server.requests().is_empty(), "a refused path was requested");
}

#[test]
fn status_codes_map_to_errors() {
    let server = Server::start(vec![
        json(401, serde_json::json!({"message": "Bad credentials"})),
        json(404, serde_json::json!({"message": "Not Found"})),
        json(
            403,
            serde_json::json!({"message": "Resource not accessible by integration"}),
        ),
        json(502, serde_json::json!({})),
    ]);
    let c = server.client();
    let get = || {
        c.api_get::<serde_json::Value>("/x", &token())
            .err()
            .unwrap()
    };
    assert!(matches!(get(), GitHubError::Unauthorized));
    assert!(matches!(get(), GitHubError::NotFound));
    match get() {
        GitHubError::Forbidden(m) => assert!(m.contains("not accessible")),
        e => panic!("{e:?}"),
    }
    assert!(matches!(get(), GitHubError::Unexpected(m) if m.contains("502")));
}

#[test]
fn an_exhausted_rate_limit_fails_fast_until_reset() {
    let reset = unix_now() + 600;
    let server = Server::start(vec![
        json(
            403,
            serde_json::json!({"message": "API rate limit exceeded"}),
        )
        .with("x-ratelimit-limit", 5000)
        .with("x-ratelimit-remaining", 0)
        .with("x-ratelimit-reset", reset),
    ]);
    let c = server.client();
    let first = c
        .api_get::<serde_json::Value>("/x", &token())
        .err()
        .unwrap();
    assert!(
        matches!(first, GitHubError::RateLimited { reset_at: Some(r), .. } if r == reset),
        "{first:?}"
    );
    assert_eq!(c.rate_limit().unwrap().remaining, 0);
    let second = c
        .api_get::<serde_json::Value>("/x", &token())
        .err()
        .unwrap();
    assert!(matches!(second, GitHubError::RateLimited { .. }));
    assert_eq!(
        server.requests().len(),
        1,
        "the second call reached the server"
    );
    assert!(second.to_string().contains("10 minutes"), "{second}");
    assert_eq!(AppError::from(second).kind, ErrorKind::RateLimited);
}

#[test]
fn secondary_rate_limits_use_retry_after() {
    let server = Server::start(vec![
        json(429, serde_json::json!({"message": "slow down"})).with("retry-after", 30),
        json(403, serde_json::json!({"message": "secondary rate limit"})).with("retry-after", 120),
    ]);
    let c = server.client();
    for want in [30, 120] {
        let e = c
            .api_get::<serde_json::Value>("/x", &token())
            .err()
            .unwrap();
        assert!(
            matches!(e, GitHubError::RateLimited { retry_after: Some(s), reset_at: None } if s == want),
            "{e:?}"
        );
    }
}

#[test]
fn rate_limit_messages() {
    assert!(rate_limit_message(None, Some(10), 0).contains("under a minute"));
    assert!(rate_limit_message(Some(1060), None, 1000).ends_with("in 1 minute"));
    assert!(rate_limit_message(Some(1000 + 601), None, 1000).ends_with("in 11 minutes"));
    assert!(rate_limit_message(None, None, 0).ends_with("later"));
    // A reset time already past still reads sensibly.
    assert!(rate_limit_message(Some(5), None, 1000).contains("under a minute"));
}

#[test]
fn redirects_off_github_are_refused() {
    let evil = Server::start(vec![user("evil")]);
    let server = Server::start(vec![Reply {
        status: 302,
        headers: vec![("Location".into(), format!("{}/steal", evil.base))],
        body: String::new(),
    }]);
    let err = server
        .client()
        .api_get::<serde_json::Value>("/user", &token())
        .err()
        .unwrap();
    assert!(
        matches!(&err, GitHubError::Network(m) if m.contains("refused a redirect")),
        "{err:?}"
    );
    assert!(evil.requests().is_empty(), "the redirect was followed");
}

#[test]
fn an_upload_goes_to_the_uploads_origin_not_the_api_origin() {
    let api = Server::start(Vec::new());
    let uploads = Server::start(vec![json(
        201,
        serde_json::json!({"id": 42, "name": "asset.zip"}),
    )]);
    let client = Client::new(Endpoints::local_with_uploads(&api.base, &uploads.base));
    let token = Token::new(SECRET).unwrap();

    let reply = client
        .api_upload::<serde_json::Value>(
            "/repos/example/example/releases/1/assets?name=asset.zip",
            &token,
            "application/octet-stream",
            b"zip-bytes",
        )
        .unwrap();

    assert_eq!(reply.value["id"], 42);
    assert!(
        api.requests().is_empty(),
        "the API origin must not see the upload"
    );
    let requests = uploads.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body, "zip-bytes");
}

#[test]
fn same_origin_redirect_is_followed() {
    let server = Server::start_with(|base| {
        vec![
            Reply {
                status: 302,
                headers: vec![("Location".into(), format!("{base}/moved"))],
                body: String::new(),
            },
            user("octocat"),
        ]
    });
    let reply = server
        .client()
        .api_get::<serde_json::Value>("/user", &token())
        .unwrap();
    assert_eq!(reply.value["login"], "octocat");
    let paths: Vec<_> = server.requests().into_iter().map(|r| r.path).collect();
    assert_eq!(paths, ["/user", "/moved"]);
}

#[test]
fn network_errors_do_not_name_the_url() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener); // nothing listens there now
    let c = Client::new(Endpoints::local(&base));
    let e = c
        .api_get::<serde_json::Value>("/user?secret=1", &token())
        .err()
        .unwrap();
    let text = e.to_string();
    assert!(matches!(e, GitHubError::Network(_)), "{e:?}");
    assert!(!text.contains("secret=1"), "{text}");
}

// ---------------------------------------------------------------------------------------------
// Device flow

#[test]
fn device_flow_polls_until_approved() {
    let server = Server::start_with(|base| {
        vec![
            code_reply(base),
            oauth_error("authorization_pending"),
            json(
                200,
                serde_json::json!({"error": "slow_down", "interval": 12}),
            ),
            oauth_error("slow_down"),
            json(
                200,
                serde_json::json!({"access_token": SECRET, "token_type": "bearer"}),
            ),
        ]
    });
    let c = server.client();
    let grant = device::start(&c, "Iv1.test").unwrap();
    assert_eq!(grant.prompt.user_code, "WDJB-MJHT");
    assert_eq!(
        grant.prompt.verification_uri,
        format!("{}/login/device", server.base)
    );
    assert!(!format!("{grant:?}").contains("dev-code-123"));

    let mut waits = Vec::new();
    let t = device::poll(&c, "Iv1.test", &grant, &mut recorder(&mut waits)).unwrap();
    assert_eq!(t.expose(), SECRET);
    let secs: Vec<u64> = waits.iter().map(Duration::as_secs).collect();
    // 5; pending → 5; slow_down to 12 → 12; slow_down without interval → 17.
    assert_eq!(secs, [5, 5, 12, 17]);

    let reqs = server.requests();
    assert_eq!(reqs[0].path, "/login/device/code");
    assert_eq!(reqs[0].form("client_id").as_deref(), Some("Iv1.test"));
    assert_eq!(reqs[0].form("scope").as_deref(), Some("public_repo"));
    for r in &reqs[1..] {
        assert_eq!(
            (r.method.as_str(), r.path.as_str()),
            ("POST", "/login/oauth/access_token")
        );
        assert_eq!(r.form("device_code").as_deref(), Some("dev-code-123"));
        assert_eq!(
            r.form("grant_type").as_deref(),
            Some("urn:ietf:params:oauth:grant-type:device_code")
        );
    }
    for r in &reqs {
        assert_eq!(r.header("accept"), Some("application/json"));
        assert!(
            r.header("authorization").is_none(),
            "token sent to the sign-in host"
        );
    }
}

#[test]
fn device_flow_stops_on_denial_expiry_and_cancel() {
    let server = Server::start_with(|b| vec![code_reply(b), oauth_error("access_denied")]);
    let c = server.client();
    let g = device::start(&c, "id").unwrap();
    let mut w = Vec::new();
    let e = device::poll(&c, "id", &g, &mut recorder(&mut w))
        .err()
        .unwrap();
    assert!(matches!(e, GitHubError::Denied), "{e:?}");

    let server = Server::start_with(|b| vec![code_reply(b), oauth_error("expired_token")]);
    let c = server.client();
    let g = device::start(&c, "id").unwrap();
    let e = device::poll(&c, "id", &g, &mut recorder(&mut w))
        .err()
        .unwrap();
    assert!(matches!(e, GitHubError::Expired));

    // Never approved: gives up once expires_in has passed without asking again.
    let server = Server::start_with(|b| {
        let mut v = vec![json(
            200,
            serde_json::json!({
                "device_code": "d", "user_code": "AB-CD",
                "verification_uri": format!("{b}/login/device"),
                "expires_in": 12, "interval": 5,
            }),
        )];
        v.extend((0..10).map(|_| oauth_error("authorization_pending")));
        v
    });
    let c = server.client();
    let g = device::start(&c, "id").unwrap();
    let e = device::poll(&c, "id", &g, &mut recorder(&mut w))
        .err()
        .unwrap();
    assert!(matches!(e, GitHubError::Expired));
    assert_eq!(
        server.requests().len(),
        1 + 3,
        "polled after the code expired"
    );

    let server = Server::start_with(|b| vec![code_reply(b)]);
    let c = server.client();
    let g = device::start(&c, "id").unwrap();
    let e = device::poll(&c, "id", &g, &mut |_| Err(GitHubError::Cancelled))
        .err()
        .unwrap();
    assert!(matches!(e, GitHubError::Cancelled));
    assert_eq!(server.requests().len(), 1, "polled after cancel");
}

#[test]
fn device_flow_rejects_a_foreign_verification_page() {
    let server = Server::start(vec![json(
        200,
        serde_json::json!({
            "device_code": "d", "user_code": "AB-CD",
            "verification_uri": "https://evil.example/login/device",
            "expires_in": 900, "interval": 5,
        }),
    )]);
    let e = device::start(&server.client(), "id").err().unwrap();
    assert!(
        matches!(&e, GitHubError::Unexpected(m) if m.contains("evil.example")),
        "{e:?}"
    );
}

#[test]
fn device_flow_reports_app_misconfiguration() {
    let server = Server::start(vec![oauth_error("device_flow_disabled")]);
    let e = device::start(&server.client(), "id").err().unwrap();
    assert!(matches!(e, GitHubError::NotConfigured(_)), "{e:?}");
    assert_eq!(AppError::from(e).kind, ErrorKind::Unavailable);

    // What github.com really answers for an unregistered client ID.
    let server = Server::start(vec![json(404, serde_json::json!({"error": "Not Found"}))]);
    let e = device::start(&server.client(), "Iv1.0000000000000000")
        .err()
        .unwrap();
    assert!(
        matches!(&e, GitHubError::NotConfigured(m) if m.contains("client ID")),
        "{e:?}"
    );
}

#[test]
fn client_ids_are_validated() {
    for ok in ["Iv1.0123abcdef", "Ov23liABCdef012", "a_b-c.d"] {
        assert!(valid_client_id(ok), "{ok}");
    }
    for bad in ["", "has space", "x&scope=repo", "a/b", &"a".repeat(65)] {
        assert!(!valid_client_id(bad), "{bad:?}");
    }
}

// ---------------------------------------------------------------------------------------------
// The account service

struct Setup {
    server: Server,
    store: Arc<MemoryStore>,
    github: GitHub,
    _dir: tempfile::TempDir,
    prefs: std::path::PathBuf,
}

fn setup(script: impl FnOnce(&str) -> Vec<Reply>, gh: GhCli) -> Setup {
    let server = Server::start_with(script);
    let store = Arc::new(MemoryStore::default());
    let dir = tempfile::tempdir().unwrap();
    let prefs = dir.path().join("github.json");
    let github = GitHub::new(
        server.client(),
        Box::new(store.clone()),
        gh,
        Some("Iv1.test".into()),
        prefs.clone(),
    );
    Setup {
        server,
        store,
        github,
        _dir: dir,
        prefs,
    }
}

fn no_gh() -> GhCli {
    GhCli::at("/nonexistent/pm-test-gh")
}

#[test]
fn sign_in_saves_the_token_and_status_reports_the_account() {
    let s = setup(
        |b| {
            vec![
                code_reply(b),
                json(200, serde_json::json!({"access_token": SECRET})),
                user("octocat"),
                user("octocat"),
            ]
        },
        no_gh(),
    );
    let before = s.github.status(true, false);
    assert!(before.account.is_none() && before.notice.is_none() && before.can_sign_in);

    let grant = s.github.sign_in_start().unwrap();
    let mut w = Vec::new();
    let account = s
        .github
        .sign_in_finish(&grant, &mut recorder(&mut w))
        .unwrap();
    assert_eq!(account.login, "octocat");
    assert_eq!(account.name.as_deref(), Some("Some One"));
    assert_eq!(account.scopes, ["public_repo", "read:org"]);
    assert_eq!(account.source, AuthSource::App);
    assert_eq!(
        s.store.token.lock().unwrap().as_ref().unwrap().expose(),
        SECRET
    );

    // Preferences hold the source and nothing secret.
    let prefs = std::fs::read_to_string(&s.prefs).unwrap();
    assert!(prefs.contains("app") && !prefs.contains(SECRET), "{prefs}");

    let status = s.github.status(true, false);
    assert_eq!(status.account.unwrap().login, "octocat");
    assert_eq!(status.rate_limit.unwrap().remaining, 4999);
    let shown = serde_json::to_string(&s.github.status(false, false)).unwrap();
    assert!(!shown.contains(SECRET), "{shown}");

    s.github.sign_out().unwrap();
    assert!(s.store.token.lock().unwrap().is_none());
    assert!(s.github.status(false, false).account.is_none());
    assert!(s.github.status(true, false).account.is_none());
    assert_eq!(s.server.requests().len(), 4, "a request after sign-out");
}

#[test]
fn a_revoked_token_is_removed_and_reported() {
    let s = setup(
        |_| vec![json(401, serde_json::json!({"message": "Bad credentials"}))],
        no_gh(),
    );
    s.store.save(&token()).unwrap();
    let status = s.github.status(true, false);
    assert!(status.account.is_none());
    assert!(status.notice.unwrap().contains("sign in again"));
    assert!(
        s.store.token.lock().unwrap().is_none(),
        "revoked token kept"
    );
}

#[test]
fn offline_status_keeps_the_known_account() {
    let s = setup(
        |_| {
            vec![
                user("octocat"),
                json(503, serde_json::json!({"message": "unavailable"})),
            ]
        },
        no_gh(),
    );
    s.store.save(&token()).unwrap();
    assert!(s.github.status(true, false).account.is_some());
    let status = s.github.status(true, false);
    assert_eq!(status.account.unwrap().login, "octocat");
    assert!(status.notice.unwrap().contains("503"));
    assert!(s.store.token.lock().unwrap().is_some());
}

#[test]
fn a_locked_keychain_is_reported_not_hidden() {
    let s = setup(|_| vec![], no_gh());
    s.store.fail.store(true, Ordering::SeqCst);
    let status = s.github.status(true, false);
    assert!(status.account.is_none());
    assert_eq!(status.notice.as_deref(), Some("locked"));
}

#[test]
fn sign_in_without_a_client_id_is_not_configured() {
    let server = Server::start(vec![]);
    let dir = tempfile::tempdir().unwrap();
    let github = GitHub::new(
        server.client(),
        Box::new(MemoryStore::default()),
        no_gh(),
        None,
        dir.path().join("github.json"),
    );
    assert!(!github.status(false, false).can_sign_in);
    let e = github.sign_in_start().err().unwrap();
    assert!(matches!(e, GitHubError::NotConfigured(_)));
    assert!(server.requests().is_empty());
}

// ---------------------------------------------------------------------------------------------
// GitHub CLI

#[test]
fn gh_version_is_parsed() {
    assert_eq!(
        parse_version(
            "gh version 2.62.0 (2024-11-14)\nhttps://github.com/cli/cli/releases/tag/v2.62.0\n"
        )
        .as_deref(),
        Some("2.62.0")
    );
    assert_eq!(
        parse_version("gh version 2.40.1\n").as_deref(),
        Some("2.40.1")
    );
    assert_eq!(parse_version("something else"), None);
    assert_eq!(parse_version(""), None);
}

#[test]
fn missing_gh_is_reported() {
    let gh = no_gh();
    let st = gh.status();
    assert!(!st.installed && !st.signed_in && st.version.is_none());
    let e = gh.token().err().unwrap();
    assert!(e.to_string().contains("not installed"), "{e}");
}

/// A fake `gh` script. `signed_in` decides whether `auth token` succeeds.
#[cfg(unix)]
fn fake_gh(dir: &std::path::Path, signed_in: bool) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("gh");
    let auth = if signed_in {
        format!("echo '{SECRET}'; exit 0")
    } else {
        // stdout here must never reach an error message.
        "echo 'STDOUT-NOT-SHOWN'; echo 'no oauth token found for github.com' >&2; exit 1"
            .to_string()
    };
    let script = format!(
        "#!/bin/sh\n\
         [ \"$GH_PROMPT_DISABLED\" = 1 ] || {{ echo 'prompts not disabled' >&2; exit 9; }}\n\
         case \"$1 $2 $3 $4\" in\n\
         \"--version   \") echo 'gh version 2.62.0 (2024-11-14)'; exit 0 ;;\n\
         \"auth token --hostname github.com\") {auth} ;;\n\
         *) echo \"unexpected: $*\" >&2; exit 2 ;;\n\
         esac\n"
    );
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn gh_login_can_be_used_and_is_left_alone_on_sign_out() {
    let bin = tempfile::tempdir().unwrap();
    let gh = GhCli::at(fake_gh(bin.path(), true));
    let st = gh.status();
    assert_eq!(
        (st.installed, st.version.as_deref(), st.signed_in),
        (true, Some("2.62.0"), true)
    );

    let s = setup(|_| vec![user("cli-user"), user("cli-user")], gh);
    let account = s.github.use_gh().unwrap();
    assert_eq!(
        (account.login.as_str(), account.source),
        ("cli-user", AuthSource::GhCli)
    );
    assert!(
        s.store.token.lock().unwrap().is_none(),
        "gh token copied to the keychain"
    );
    assert_eq!(
        s.server.requests()[0].header("authorization"),
        Some(&*format!("Bearer {SECRET}"))
    );
    let prefs = std::fs::read_to_string(&s.prefs).unwrap();
    assert!(
        prefs.contains("ghCli") && !prefs.contains(SECRET),
        "{prefs}"
    );

    // An app token saved earlier is not touched by signing out of the gh source.
    s.store.save(&token()).unwrap();
    s.github.sign_out().unwrap();
    assert!(s.store.token.lock().unwrap().is_some());
}

#[cfg(unix)]
#[test]
fn gh_without_login_explains_from_stderr_only() {
    let bin = tempfile::tempdir().unwrap();
    let gh = GhCli::at(fake_gh(bin.path(), false));
    assert!(!gh.status().signed_in);
    let e = gh.token().err().unwrap();
    let text = e.to_string();
    assert!(
        text.contains("no oauth token found") && text.contains("gh auth login"),
        "{text}"
    );
    assert!(!text.contains("STDOUT-NOT-SHOWN"), "{text}");
    assert_eq!(AppError::from(e).kind, ErrorKind::Unavailable);
}

// ---------------------------------------------------------------------------------------------
// Error mapping

#[test]
fn errors_map_to_frontend_kinds() {
    let cases = [
        (GitHubError::NotSignedIn, ErrorKind::NotSignedIn),
        (GitHubError::Unauthorized, ErrorKind::Auth),
        (GitHubError::Denied, ErrorKind::Auth),
        (GitHubError::Expired, ErrorKind::Auth),
        (GitHubError::Forbidden("x".into()), ErrorKind::Auth),
        (GitHubError::Keychain("x".into()), ErrorKind::Unavailable),
        (GitHubError::Gh("x".into()), ErrorKind::Unavailable),
        (GitHubError::NotFound, ErrorKind::NotFound),
        (GitHubError::Network("x".into()), ErrorKind::Network),
        (GitHubError::Cancelled, ErrorKind::Cancelled),
    ];
    for (e, kind) in cases {
        assert_eq!(AppError::from(e).kind, kind);
    }
}

// ---------------------------------------------------------------------------------------------
// Real keychain (needs a desktop session; run with `cargo test -- --ignored keychain`)

#[test]
#[ignore = "touches the real OS keychain"]
fn keychain_round_trip() {
    let k =
        super::token::Keychain::named("io.github.thepotato456.projectmanager.test", "round-trip");
    k.clear().unwrap();
    assert!(k.load().unwrap().is_none());
    k.save(&token()).unwrap();
    assert_eq!(k.load().unwrap().unwrap().expose(), SECRET);
    k.clear().unwrap();
    assert!(k.load().unwrap().is_none());
    k.clear().unwrap();
}

/// One read-only `GET /user` against the real API with the GitHub CLI's login: checks TLS,
/// headers and scope parsing end to end. Run with `cargo test -- --ignored live_gh`.
#[test]
#[ignore = "calls api.github.com with the local gh login"]
fn live_gh_account() {
    let dir = tempfile::tempdir().unwrap();
    let github = GitHub::new(
        Client::new(Endpoints::github()),
        Box::new(MemoryStore::default()),
        GhCli::from_env(),
        None,
        dir.path().join("github.json"),
    );
    let account = github.use_gh().unwrap();
    assert!(!account.login.is_empty());
    assert_eq!(account.source, AuthSource::GhCli);
    let status = github.status(true, true);
    let rate = status.rate_limit.expect("rate limit headers");
    assert!(rate.limit >= 60 && rate.remaining <= rate.limit);
    assert!(status.gh.unwrap().signed_in);
    println!(
        "signed in as {}, scopes {:?}, {}/{} requests left",
        account.login, account.scopes, rate.remaining, rate.limit
    );
}
