# GitHub sign-in

Project Manager uses GitHub to publish releases. It talks to the REST API directly. The code is in
`app/src/desktop/src/github/`.

## Ways to sign in

| Source               | How                                                                  | Where the token lives                                                          |
| -------------------- | -------------------------------------------------------------------- | ------------------------------------------------------------------------------ |
| Project Manager's own (`app`)  | OAuth device flow: Project Manager shows a code, the user enters it on GitHub. | The OS keychain: service `io.github.thepotato456.projectmanager`, account `github.com`. |
| GitHub CLI (`ghCli`) | Reuse the login of an installed, signed-in `gh` (`gh auth token`).   | Stays in `gh`'s own storage; Project Manager reads it again on each start.               |

The chosen source is saved in `github.json` in the app data directory. That file holds no secrets.

The device flow asks for the `public_repo` scope only (create releases and upload assets on
public repositories).

## Setting up the OAuth app (device flow)

The device flow needs a GitHub OAuth app with **device flow enabled** (GitHub → Settings →
Developer settings → OAuth Apps → the app → "Enable Device Flow"). Its client ID is public and is
not a secret; no client secret is used.

Project Manager reads the client ID from `PM_GITHUB_CLIENT_ID`:

- at run time (environment of the running app), or, failing that,
- at build time. Project Manager's own client ID (`Ov23liz2huQkKgUBlKJd`) is set in
  `app/src/desktop/.cargo/config.toml`, so every build has it. Setting
  `PM_GITHUB_CLIENT_ID=... npx tauri build` overrides it.

The value must be 1 to 64 letters, digits, `.`, `_` or `-`; anything else is ignored. Without a
client ID the account dialog says sign-in is not set up and offers the GitHub CLI instead.

If GitHub answers that the app does not exist or has device flow turned off, the error says
"GitHub sign-in is not set up: …" rather than a generic failure.

## The GitHub CLI fallback

Project Manager runs `gh` without a shell, with prompts, colors and update checks turned off, stdin closed
and a 15-second timeout. It only reads `gh --version` and `gh auth token --hostname github.com`.
Set `PM_GH` to a full path to use a `gh` that is not on `PATH`.

Signing out of a `gh` login only stops Project Manager using it; `gh` stays signed in.

## Signing out and revoking

Signing out of Project Manager's own sign-in deletes the keychain entry. The token stays valid on GitHub
until it is revoked there: GitHub → Settings → Applications → Authorized OAuth Apps → Project Manager →
Revoke. If a saved token was revoked or expired, the next status check removes it from the
keychain and the account dialog says so.

## Token safety

- The token is held as `Token`, which prints as `Token(<redacted>)` and cannot be serialised, so it never
  reaches logs, errors or the frontend.
- It is sent only as an `Authorization: Bearer` header to `https://api.github.com`. API paths
  are checked so a request cannot leave that origin. Redirects are followed only within
  `github.com` or `api.github.com` (at most 5); anything else is refused.
- Only HTTPS is used (rustls).
- Error messages never include URLs, headers or the token. Errors from `gh` use only the first
  line of its stderr; its stdout (which holds the token) is never shown.
- The device code is not shown or logged; only the short user code is.

## Rate limits

Every API reply's `x-ratelimit-*` headers are recorded and shown in the account dialog. When the
limit is used up, requests fail at once with "GitHub's rate limit was reached; try again in N
minutes" instead of being sent. Secondary limits (`retry-after`) are reported the same way.

## Tests

`cargo test github` (in `app/src/desktop/`) runs against a local mock server: device-flow
polling and `slow_down`, denial, expiry, cancel, redirects, rate limits, revoked tokens, offline
use, a locked keychain, and a fake `gh`. Two tests need real services and are ignored by default:

- `cargo test -- --ignored keychain_round_trip` writes, reads and deletes a test entry in the real
  OS keychain (service `io.github.thepotato456.projectmanager.test`).
- `cargo test -- --ignored live_gh_account` uses the real `gh` login to fetch the account from
  github.com.
