//! GitHub Releases tests. Network tests run against a scripted HTTP server on 127.0.0.1;
//! nothing here talks to the real GitHub.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::github::{
    GhCli, GitHub,
    http::{Client, Endpoints},
    token::{MemoryStore, TokenStore},
};
use crate::testing::FixtureRepo;

// -------------------------------------------------------------------------------------------------
// Mock server

#[derive(Debug, Clone)]
struct Request {
    method: String,
    path: String,
    content_type: Option<String>,
    body: String,
}

#[derive(Debug, Clone)]
struct Reply {
    status: u16,
    body: String,
}

fn json(status: u16, body: serde_json::Value) -> Reply {
    Reply {
        status,
        body: body.to_string(),
    }
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        404 => "Not Found",
        _ => "Unknown",
    }
}

struct Server {
    base: String,
    log: Arc<Mutex<Vec<Request>>>,
}

impl Server {
    fn start(replies: Vec<Reply>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
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
                let reply = replies.lock().unwrap().pop_front().unwrap_or(Reply {
                    status: 404,
                    body: "{\"message\":\"Not Found\"}".into(),
                });
                let mut out = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\n",
                    reply.status,
                    reason_phrase(reply.status)
                );
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
    let mut content_length = 0usize;
    let mut content_type = None;
    loop {
        let mut h = String::new();
        reader.read_line(&mut h).ok()?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        let (k, v) = h.split_once(':')?;
        let k = k.trim();
        let v = v.trim();
        if k.eq_ignore_ascii_case("content-length") {
            content_length = v.parse().unwrap_or(0);
        } else if k.eq_ignore_ascii_case("content-type") {
            content_type = Some(v.to_string());
        }
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).ok()?;
    Some(Request {
        method,
        path,
        content_type,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

// -------------------------------------------------------------------------------------------------
// Fixtures

const SINGLE_REPO_PROJ: &str = r#"schema_version = 1

[project]
name = "Example"
version = "1.2.3"

[workflow]
mode = "single-repo"

[workflow.private]
branch = "main"

[workflow.public]
branch = "public"

[release]
version_files = []
"#;

fn fixture() -> FixtureRepo {
    let r = FixtureRepo::new();
    r.write("example.proj", SINGLE_REPO_PROJ);
    r.commit_all("add proj");
    r.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/example/example.git",
    ]);
    r
}

fn github_for(server: &Server, dir: &tempfile::TempDir) -> GitHub {
    let store = std::sync::Arc::new(MemoryStore::default());
    store
        .save(&crate::github::token::Token::new("gho_test_token").unwrap())
        .unwrap();
    GitHub::new(
        server.client(),
        Box::new(store),
        GhCli::at("/nonexistent/pm-test-gh"),
        Some("Iv1.test".into()),
        dir.path().join("github.json"),
    )
}

// -------------------------------------------------------------------------------------------------
// Pure tests (no server)

#[test]
fn parse_repo_slug_handles_https_and_ssh_urls() {
    assert_eq!(
        parse_repo_slug("https://github.com/example/example.git").unwrap(),
        "example/example"
    );
    assert_eq!(
        parse_repo_slug("https://github.com/example/example").unwrap(),
        "example/example"
    );
    assert_eq!(
        parse_repo_slug("git@github.com:example/example.git").unwrap(),
        "example/example"
    );
    assert_eq!(
        parse_repo_slug("ssh://git@github.com/example/example.git").unwrap(),
        "example/example"
    );
}

#[test]
fn parse_repo_slug_refuses_non_github_or_unparseable_urls() {
    assert!(parse_repo_slug("https://example.com/example/example.git").is_err());
    assert!(parse_repo_slug("https://github.com/example").is_err());
    assert!(parse_repo_slug("https://github.com/example/example/extra").is_err());
    assert!(parse_repo_slug("").is_err());
}

// -------------------------------------------------------------------------------------------------
// Server-backed tests

#[test]
fn list_releases_parses_the_payload() {
    let server = Server::start(vec![json(
        200,
        serde_json::json!([
            {
                "id": 11,
                "tag_name": "v1.2.3",
                "name": null,
                "draft": false,
                "prerelease": false,
                "html_url": "https://github.com/example/example/releases/tag/v1.2.3",
                "published_at": "2026-11-26T00:00:00Z",
                "assets": [
                    {
                        "id": 7,
                        "name": "Example-1.2.3.zip",
                        "size": 4096,
                        "content_type": "application/zip",
                        "browser_download_url": "https://github.com/example/example/releases/download/v1.2.3/Example-1.2.3.zip"
                    }
                ]
            }
        ]),
    )]);
    let dir = tempfile::tempdir().unwrap();
    let github = github_for(&server, &dir);
    let r = fixture();

    let releases = list_releases(&github, r.path()).unwrap();

    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0].tag_name, "v1.2.3");
    assert_eq!(releases[0].assets.len(), 1);
    assert_eq!(releases[0].assets[0].name, "Example-1.2.3.zip");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, "/repos/example/example/releases");
}

#[test]
fn create_release_posts_the_tag_body_and_returns_the_release() {
    let server = Server::start(vec![json(
        201,
        serde_json::json!({
            "id": 9,
            "tag_name": "v1.2.3",
            "name": "v1.2.3",
            "draft": true,
            "prerelease": false,
            "html_url": "https://github.com/example/example/releases/tag/v1.2.3",
            "published_at": null,
            "assets": []
        }),
    )]);
    let dir = tempfile::tempdir().unwrap();
    let github = github_for(&server, &dir);
    let r = fixture();

    let release = create_release(&github, r.path(), None, None, "notes", true, false).unwrap();

    assert_eq!(release.id, 9);
    assert!(release.draft);
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/repos/example/example/releases");
    let body: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(body["tag_name"], "v1.2.3");
    assert_eq!(body["draft"], true);
    assert_eq!(body["body"], "notes");
    assert!(body.get("name").is_none(), "name must be omitted: {body}");
}

#[test]
fn update_release_sends_only_the_given_fields() {
    let server = Server::start(vec![json(
        200,
        serde_json::json!({
            "id": 9,
            "tag_name": "v1.2.3",
            "name": "Release v1.2.3",
            "draft": false,
            "prerelease": true,
            "html_url": "https://github.com/example/example/releases/tag/v1.2.3",
            "published_at": "2026-11-26T00:00:00Z",
            "assets": []
        }),
    )]);
    let dir = tempfile::tempdir().unwrap();
    let github = github_for(&server, &dir);
    let r = fixture();

    let release = update_release(
        &github,
        r.path(),
        9,
        Some("Release v1.2.3"),
        None,
        Some(false),
        Some(true),
    )
    .unwrap();

    assert_eq!(release.name.as_deref(), Some("Release v1.2.3"));
    let requests = server.requests();
    assert_eq!(requests[0].method, "PATCH");
    assert_eq!(requests[0].path, "/repos/example/example/releases/9");
    let body: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(body["name"], "Release v1.2.3");
    assert_eq!(body["draft"], false);
    assert_eq!(body["prerelease"], true);
    assert!(body.get("body").is_none(), "{body}");
}

#[test]
fn delete_release_is_a_delete() {
    let server = Server::start(vec![Reply {
        status: 204,
        body: String::new(),
    }]);
    let dir = tempfile::tempdir().unwrap();
    let github = github_for(&server, &dir);
    let r = fixture();

    delete_release(&github, r.path(), 9).unwrap();

    let requests = server.requests();
    assert_eq!(requests[0].method, "DELETE");
    assert_eq!(requests[0].path, "/repos/example/example/releases/9");
}

#[test]
fn upload_asset_posts_the_zip_bytes() {
    let server = Server::start(vec![json(
        201,
        serde_json::json!({
            "id": 7,
            "name": "Example-1.2.3.zip",
            "size": 11,
            "content_type": "application/zip",
            "browser_download_url": "https://example.com/example.zip"
        }),
    )]);
    let dir = tempfile::tempdir().unwrap();
    let releases = dir.path().join("releases");
    std::fs::create_dir_all(&releases).unwrap();
    std::fs::write(releases.join("Example-1.2.3.zip"), b"zip-bytes").unwrap();
    let github = github_for(&server, &dir);
    let r = fixture();

    let asset = upload_asset(&github, r.path(), dir.path(), 9).unwrap();

    assert_eq!(asset.id, 7);
    let requests = server.requests();
    assert_eq!(requests[0].method, "POST");
    assert_eq!(
        requests[0].path,
        "/repos/example/example/releases/9/assets?name=Example-1.2.3.zip"
    );
    assert_eq!(
        requests[0].content_type.as_deref(),
        Some("application/octet-stream")
    );
    assert_eq!(requests[0].body, "zip-bytes");
}

#[test]
fn replace_asset_deletes_then_reuploads() {
    let server = Server::start(vec![
        Reply {
            status: 204,
            body: String::new(),
        },
        json(
            201,
            serde_json::json!({
                "id": 8,
                "name": "Example-1.2.3.zip",
                "size": 11,
                "content_type": "application/zip",
                "browser_download_url": "https://example.com/example.zip"
            }),
        ),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let releases = dir.path().join("releases");
    std::fs::create_dir_all(&releases).unwrap();
    std::fs::write(releases.join("Example-1.2.3.zip"), b"zip-bytes").unwrap();
    let github = github_for(&server, &dir);
    let r = fixture();

    let release = GitHubRelease {
        id: 9,
        tag_name: "v1.2.3".into(),
        name: None,
        draft: false,
        prerelease: false,
        html_url: "https://github.com/example/example/releases/tag/v1.2.3".into(),
        published_at: None,
        assets: vec![ReleaseAsset {
            id: 7,
            name: "Example-1.2.3.zip".into(),
            size: 11,
            content_type: Some("application/zip".into()),
            browser_download_url: "https://example.com/example.zip".into(),
        }],
    };

    let asset = replace_asset(&github, r.path(), dir.path(), &release).unwrap();

    assert_eq!(asset.id, 8);
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, "DELETE");
    assert_eq!(requests[0].path, "/repos/example/example/releases/assets/7");
    assert_eq!(requests[1].method, "POST");
    assert_eq!(
        requests[1].path,
        "/repos/example/example/releases/9/assets?name=Example-1.2.3.zip"
    );
    assert_eq!(requests[1].body, "zip-bytes");
}
