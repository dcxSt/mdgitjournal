//! GitHub client: client ID, device flow, user and repository lookups, URLs.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::http::{Http, ReqwestHttp};

pub const DEFAULT_CLIENT_ID: &str = "MDGJ_PLACEHOLDER_CLIENT_ID";
pub const DEFAULT_APP_SLUG: &str = "mdgitjournal";

/// The GitHub App's public client ID, compiled in from `MDGJ_GITHUB_CLIENT_ID`.
pub fn client_id() -> &'static str {
    build_setting(option_env!("MDGJ_GITHUB_CLIENT_ID"), DEFAULT_CLIENT_ID)
}

/// The GitHub App's URL name, compiled in from `MDGJ_GITHUB_APP_SLUG`.
pub fn app_slug() -> &'static str {
    build_setting(option_env!("MDGJ_GITHUB_APP_SLUG"), DEFAULT_APP_SLUG)
}

/// True when the build has no registered GitHub App, so only personal access tokens work.
pub fn is_placeholder() -> bool {
    client_id() == DEFAULT_CLIENT_ID
}

// CI passes unset secrets as empty strings.
fn build_setting(value: Option<&'static str>, default: &'static str) -> &'static str {
    value.filter(|v| !v.is_empty()).unwrap_or(default)
}

const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
const API_URL: &str = "https://api.github.com";
const API_VERSION: &str = "2022-11-28";
const USER_AGENT: &str = concat!("mdgitjournal/", env!("CARGO_PKG_VERSION"));
const PER_PAGE: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    /// Seconds until `device_code` expires.
    pub expires_in: u64,
    /// Minimum seconds between polls.
    pub interval: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Poll {
    Pending,
    /// Polling too fast: wait `interval` seconds between polls from now on.
    SlowDown {
        interval: u64,
    },
    Complete(Tokens),
    Expired,
    Denied,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    /// Unix seconds; `None` for tokens that don't expire.
    pub expires_at: Option<i64>,
    pub refresh_token: Option<String>,
    /// Unix seconds.
    pub refresh_expires_at: Option<i64>,
}

// Keeps tokens out of logs and panic messages.
impl fmt::Debug for Tokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tokens")
            .field("access_token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("refresh_expires_at", &self.refresh_expires_at)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct User {
    pub login: String,
    pub id: u64,
    pub name: Option<String>,
    pub email: Option<String>,
}

/// Read from GitHub's snake_case JSON, written as camelCase for the UI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct RepoInfo {
    pub full_name: String,
    pub private: bool,
    pub clone_url: String,
    pub default_branch: String,
}

/// A token response from the OAuth endpoint, with expiries relative to now.
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: Option<i64>,
    refresh_token: Option<String>,
    refresh_token_expires_in: Option<i64>,
}

impl TokenResponse {
    fn at(self, now: i64) -> Tokens {
        Tokens {
            access_token: self.access_token,
            expires_at: self.expires_in.map(|s| now.saturating_add(s)),
            refresh_token: self.refresh_token,
            refresh_expires_at: self.refresh_token_expires_in.map(|s| now.saturating_add(s)),
        }
    }
}

/// The OAuth endpoints report errors in the body, usually with HTTP 200.
#[derive(Deserialize)]
struct OAuthError {
    error: String,
    error_description: Option<String>,
    interval: Option<u64>,
    #[serde(skip)]
    status: u16,
}

impl From<OAuthError> for Error {
    fn from(e: OAuthError) -> Self {
        Error::Api {
            status: e.status,
            message: e.error_description.unwrap_or(e.error),
        }
    }
}

#[derive(Deserialize)]
struct Installation {
    id: u64,
    suspended_at: Option<String>,
}

#[derive(Deserialize)]
struct Page<T> {
    total_count: usize,
    #[serde(alias = "installations", alias = "repositories")]
    items: Vec<T>,
}

pub struct GitHub<H: Http = ReqwestHttp> {
    http: H,
    client_id: &'static str,
    app_slug: &'static str,
    now: fn() -> i64,
}

impl GitHub {
    /// Page that creates a private repository; the GitHub App can then be installed on it.
    pub fn new_repo_url(name: &str) -> String {
        let mut url = reqwest::Url::parse("https://github.com/new").expect("constant URL");
        url.query_pairs_mut()
            .append_pair("name", name)
            .append_pair("visibility", "private");
        url.into()
    }
}

impl<H: Http> GitHub<H> {
    /// A client for the GitHub App this build was compiled for.
    pub fn new(http: H) -> Self {
        Self {
            http,
            client_id: client_id(),
            app_slug: app_slug(),
            now: unix_now,
        }
    }

    pub(crate) fn now(&self) -> i64 {
        (self.now)()
    }

    pub fn start_device_flow(&self) -> Result<DeviceCode> {
        if self.client_id == DEFAULT_CLIENT_ID {
            return Err(Error::Placeholder);
        }
        // GitHub Apps take their permissions from the app settings, so no scope is sent.
        self.oauth(DEVICE_CODE_URL, &[("client_id", self.client_id)])?
            .map_err(Error::from)
    }

    pub fn poll_device_flow(&self, dc: &DeviceCode) -> Result<Poll> {
        let form = [
            ("client_id", self.client_id),
            ("device_code", dc.device_code.as_str()),
            ("grant_type", DEVICE_GRANT),
        ];
        match self.oauth::<TokenResponse>(ACCESS_TOKEN_URL, &form)? {
            Ok(tokens) => Ok(Poll::Complete(tokens.at(self.now()))),
            Err(e) => match e.error.as_str() {
                "authorization_pending" => Ok(Poll::Pending),
                // RFC 8628 §3.5: without a new interval, add 5 seconds.
                "slow_down" => Ok(Poll::SlowDown {
                    interval: e.interval.unwrap_or(dc.interval + 5),
                }),
                "expired_token" => Ok(Poll::Expired),
                "access_denied" => Ok(Poll::Denied),
                _ => Err(e.into()),
            },
        }
    }

    /// Exchanges a refresh token for new tokens. The old refresh token stops working.
    pub fn refresh(&self, refresh_token: &str) -> Result<Tokens> {
        let form = [
            ("client_id", self.client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ];
        match self.oauth::<TokenResponse>(ACCESS_TOKEN_URL, &form)? {
            Ok(tokens) => Ok(tokens.at(self.now())),
            Err(e) if e.error == "bad_refresh_token" => Err(Error::ReauthRequired),
            Err(e) => Err(e.into()),
        }
    }

    pub fn user(&self, token: &str) -> Result<User> {
        self.get(token, "/user")
    }

    /// Repositories the user has installed this GitHub App on, across all accounts.
    pub fn installation_repos(&self, token: &str) -> Result<Vec<RepoInfo>> {
        let installations: Vec<Installation> = self.get_all(token, "/user/installations")?;
        let mut repos = Vec::new();
        // The app can't reach the repositories of a suspended installation.
        for installation in installations.iter().filter(|i| i.suspended_at.is_none()) {
            let path = format!("/user/installations/{}/repositories", installation.id);
            repos.extend(self.get_all(token, &path)?);
        }
        Ok(repos)
    }

    /// Looks up `owner/repo`.
    pub fn repo(&self, token: &str, full_name: &str) -> Result<RepoInfo> {
        if !is_full_name(full_name) {
            return Err(Error::Invalid(format!(
                "\"{full_name}\" isn't a repository name. Write it as owner/repo."
            )));
        }
        match self.get(token, &format!("/repos/{full_name}")) {
            Err(Error::Api { status: 404, .. }) => Err(Error::Api {
                status: 404,
                message: format!("{full_name} doesn't exist, or the token can't access it"),
            }),
            other => other,
        }
    }

    /// Page that installs the GitHub App on repositories the user picks.
    pub fn install_url(&self) -> String {
        format!(
            "https://github.com/apps/{}/installations/new",
            self.app_slug
        )
    }

    /// POSTs to an OAuth endpoint. The inner error is GitHub's OAuth error code, for the
    /// caller to interpret.
    fn oauth<T: DeserializeOwned>(
        &self,
        url: &str,
        form: &[(&str, &str)],
    ) -> Result<Result<T, OAuthError>> {
        let headers = [("Accept", "application/json"), ("User-Agent", USER_AGENT)];
        let (status, body) = self.http.post_form(url, form, &headers)?;
        if let Ok(mut err) = serde_json::from_str::<OAuthError>(&body) {
            err.status = status;
            return Ok(Err(err));
        }
        parse(status, &body).map(Ok)
    }

    fn get<T: DeserializeOwned>(&self, token: &str, path: &str) -> Result<T> {
        let authorization = format!("Bearer {token}");
        let headers = [
            ("Accept", "application/vnd.github+json"),
            ("Authorization", authorization.as_str()),
            ("User-Agent", USER_AGENT),
            ("X-GitHub-Api-Version", API_VERSION),
        ];
        let (status, body) = self.http.get(&format!("{API_URL}{path}"), &headers)?;
        parse(status, &body)
    }

    /// Follows page numbers until a short page or `total_count` items.
    fn get_all<T: DeserializeOwned>(&self, token: &str, path: &str) -> Result<Vec<T>> {
        let mut items = Vec::new();
        for page in 1.. {
            let path = format!("{path}?per_page={PER_PAGE}&page={page}");
            let Page {
                total_count,
                items: batch,
            } = self.get::<Page<T>>(token, &path)?;
            let short = batch.len() < PER_PAGE;
            items.extend(batch);
            if short || items.len() >= total_count {
                break;
            }
        }
        Ok(items)
    }
}

fn parse<T: DeserializeOwned>(status: u16, body: &str) -> Result<T> {
    if !(200..300).contains(&status) {
        return Err(Error::from_response(status, body));
    }
    serde_json::from_str(body).map_err(|e| Error::unexpected(status, e))
}

/// `owner/repo` with GitHub's name characters, so it can't escape `/repos/` in a URL.
fn is_full_name(name: &str) -> bool {
    let valid = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    };
    matches!(name.split_once('/'), Some((owner, repo)) if valid(owner) && valid(repo))
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
impl<H: Http> GitHub<H> {
    pub(crate) fn for_tests(http: H, client_id: &'static str) -> Self {
        Self {
            http,
            client_id,
            app_slug: "mdgj-test",
            now: || crate::mock::NOW,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{MockHttp, NOW};

    fn github(http: &MockHttp) -> GitHub<MockHttp> {
        GitHub::for_tests(http.clone(), "Iv1.test")
    }

    fn device_code() -> DeviceCode {
        DeviceCode {
            device_code: "dev-123".into(),
            user_code: "ABCD-1234".into(),
            verification_uri: "https://github.com/login/device".into(),
            expires_in: 899,
            interval: 5,
        }
    }

    fn repo_json(full_name: &str) -> String {
        format!(
            r#"{{"id":1,"full_name":"{full_name}","private":true,
                "clone_url":"https://github.com/{full_name}.git","default_branch":"main"}}"#
        )
    }

    fn page(key: &str, total: usize, items: &[String]) -> String {
        format!(r#"{{"total_count":{total},"{key}":[{}]}}"#, items.join(","))
    }

    #[test]
    fn build_settings_fall_back_when_unset_or_empty() {
        assert_eq!(build_setting(None, "d"), "d");
        assert_eq!(build_setting(Some(""), "d"), "d");
        assert_eq!(build_setting(Some("Iv1.x"), "d"), "Iv1.x");
    }

    #[test]
    fn start_device_flow_posts_client_id_and_parses_code() {
        let http = MockHttp::new();
        http.reply(
            200,
            r#"{"device_code":"dev-123","user_code":"ABCD-1234",
                "verification_uri":"https://github.com/login/device","expires_in":899,"interval":5}"#,
        );
        assert_eq!(github(&http).start_device_flow().unwrap(), device_code());

        let [request] = http.requests().try_into().unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(request.url, "https://github.com/login/device/code");
        assert_eq!(request.form, [("client_id".into(), "Iv1.test".into())]);
        assert_eq!(request.header("Accept"), Some("application/json"));
        assert_eq!(request.header("User-Agent"), Some(USER_AGENT));
    }

    #[test]
    fn start_device_flow_without_client_id_is_placeholder() {
        let http = MockHttp::new();
        let err = GitHub::for_tests(http.clone(), DEFAULT_CLIENT_ID)
            .start_device_flow()
            .unwrap_err();
        assert!(matches!(err, Error::Placeholder));
        assert_eq!(
            err.to_string(),
            "This build has no GitHub App client ID. Register the app (see docs/github-app.md) \
             or use a personal access token."
        );
        assert!(http.requests().is_empty());
    }

    #[test]
    fn start_device_flow_reports_oauth_errors() {
        let http = MockHttp::new();
        http.reply(
            200,
            r#"{"error":"device_flow_disabled","error_description":"Device Flow must be explicitly enabled for this App"}"#,
        );
        let err = github(&http).start_device_flow().unwrap_err();
        assert_eq!(
            err.to_string(),
            "GitHub request failed: Device Flow must be explicitly enabled for this App"
        );
    }

    #[test]
    fn poll_sends_device_code_grant() {
        let http = MockHttp::new();
        http.reply(200, r#"{"error":"authorization_pending"}"#);
        github(&http).poll_device_flow(&device_code()).unwrap();

        let [request] = http.requests().try_into().unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(request.url, "https://github.com/login/oauth/access_token");
        assert_eq!(request.field("client_id"), Some("Iv1.test"));
        assert_eq!(request.field("device_code"), Some("dev-123"));
        assert_eq!(request.field("grant_type"), Some(DEVICE_GRANT));
        assert_eq!(request.header("Accept"), Some("application/json"));
    }

    #[test]
    fn poll_maps_each_state() {
        let cases = [
            (
                r#"{"error":"authorization_pending","error_description":"pending"}"#,
                Poll::Pending,
            ),
            (
                r#"{"error":"slow_down","interval":10}"#,
                Poll::SlowDown { interval: 10 },
            ),
            (r#"{"error":"slow_down"}"#, Poll::SlowDown { interval: 10 }),
            (r#"{"error":"expired_token"}"#, Poll::Expired),
            (r#"{"error":"access_denied"}"#, Poll::Denied),
        ];
        for (body, expected) in cases {
            let http = MockHttp::new();
            http.reply(200, body);
            let poll = github(&http).poll_device_flow(&device_code()).unwrap();
            assert_eq!(poll, expected, "{body}");
        }
    }

    #[test]
    fn poll_reports_other_errors() {
        let http = MockHttp::new();
        http.reply(
            200,
            r#"{"error":"incorrect_device_code","error_description":"The device_code provided is not valid."}"#,
        );
        let err = github(&http).poll_device_flow(&device_code()).unwrap_err();
        assert!(
            matches!(&err, Error::Api { status: 200, message } if message == "The device_code provided is not valid."),
            "{err:?}"
        );
    }

    #[test]
    fn poll_complete_converts_expiry_to_absolute_time() {
        let http = MockHttp::new();
        http.reply(
            200,
            r#"{"access_token":"ghu_a","expires_in":28800,"refresh_token":"ghr_r",
                "refresh_token_expires_in":15897600,"token_type":"bearer","scope":""}"#,
        );
        let poll = github(&http).poll_device_flow(&device_code()).unwrap();
        assert_eq!(
            poll,
            Poll::Complete(Tokens {
                access_token: "ghu_a".into(),
                expires_at: Some(NOW + 28800),
                refresh_token: Some("ghr_r".into()),
                refresh_expires_at: Some(NOW + 15897600),
            })
        );
    }

    #[test]
    fn poll_complete_without_expiry() {
        let http = MockHttp::new();
        http.reply(
            200,
            r#"{"access_token":"ghu_a","token_type":"bearer","scope":""}"#,
        );
        let poll = github(&http).poll_device_flow(&device_code()).unwrap();
        assert_eq!(
            poll,
            Poll::Complete(Tokens {
                access_token: "ghu_a".into(),
                expires_at: None,
                refresh_token: None,
                refresh_expires_at: None,
            })
        );
    }

    #[test]
    fn refresh_sends_refresh_grant_without_secret() {
        let http = MockHttp::new();
        http.reply(
            200,
            r#"{"access_token":"ghu_new","expires_in":28800,"refresh_token":"ghr_new",
                "refresh_token_expires_in":15897600}"#,
        );
        let tokens = github(&http).refresh("ghr_old").unwrap();
        assert_eq!(tokens.access_token, "ghu_new");
        assert_eq!(tokens.expires_at, Some(NOW + 28800));
        assert_eq!(tokens.refresh_token.as_deref(), Some("ghr_new"));
        assert_eq!(tokens.refresh_expires_at, Some(NOW + 15897600));

        let [request] = http.requests().try_into().unwrap();
        assert_eq!(request.url, "https://github.com/login/oauth/access_token");
        assert_eq!(
            request.form,
            [
                ("client_id".into(), "Iv1.test".into()),
                ("grant_type".into(), "refresh_token".into()),
                ("refresh_token".into(), "ghr_old".into()),
            ]
        );
    }

    #[test]
    fn bad_refresh_token_requires_reauth() {
        let http = MockHttp::new();
        http.reply(200, r#"{"error":"bad_refresh_token","error_description":"The refresh token passed is incorrect or expired."}"#);
        let err = github(&http).refresh("ghr_old").unwrap_err();
        assert!(matches!(err, Error::ReauthRequired), "{err:?}");
        assert_eq!(
            err.to_string(),
            "Your GitHub sign-in has expired. Sign in again."
        );
    }

    #[test]
    fn user_sends_rest_headers() {
        let http = MockHttp::new();
        http.reply(
            200,
            r#"{"login":"dcxSt","id":42,"name":"Stephen Fay","email":null}"#,
        );
        let user = github(&http).user("ghu_a").unwrap();
        assert_eq!(
            user,
            User {
                login: "dcxSt".into(),
                id: 42,
                name: Some("Stephen Fay".into()),
                email: None,
            }
        );

        let [request] = http.requests().try_into().unwrap();
        assert_eq!(request.method, "GET");
        assert_eq!(request.url, "https://api.github.com/user");
        assert_eq!(request.header("Authorization"), Some("Bearer ghu_a"));
        assert_eq!(
            request.header("Accept"),
            Some("application/vnd.github+json")
        );
        assert_eq!(request.header("X-GitHub-Api-Version"), Some("2022-11-28"));
        assert_eq!(
            request.header("User-Agent"),
            Some(concat!("mdgitjournal/", env!("CARGO_PKG_VERSION")))
        );
    }

    #[test]
    fn unauthorized_maps_to_auth() {
        let http = MockHttp::new();
        http.reply(401, r#"{"message":"Bad credentials","documentation_url":"https://docs.github.com/rest","status":"401"}"#);
        let err = github(&http).user("ghp_bad").unwrap_err();
        assert!(
            matches!(&err, Error::Auth(m) if m == "Bad credentials"),
            "{err:?}"
        );
        assert_eq!(
            err.to_string(),
            "GitHub rejected the token: Bad credentials"
        );
    }

    #[test]
    fn non_json_error_names_the_status() {
        let http = MockHttp::new();
        http.reply(502, "<html>Bad Gateway</html>");
        let err = github(&http).user("ghu_a").unwrap_err();
        assert!(
            matches!(&err, Error::Api { status: 502, message } if message == "HTTP 502"),
            "{err:?}"
        );
    }

    #[test]
    fn malformed_success_is_an_api_error() {
        let http = MockHttp::new();
        http.reply(200, r#"{"id":42}"#);
        let err = github(&http).user("ghu_a").unwrap_err();
        assert!(
            matches!(&err, Error::Api { status: 200, message } if message.starts_with("unexpected response")),
            "{err:?}"
        );
    }

    #[test]
    fn network_errors_pass_through() {
        let http = MockHttp::new();
        http.fail(Error::Network("dns error".into()));
        let err = github(&http).user("ghu_a").unwrap_err();
        assert!(
            matches!(&err, Error::Network(m) if m == "dns error"),
            "{err:?}"
        );
    }

    #[test]
    fn installation_repos_paginates_every_installation() {
        let http = MockHttp::new();
        let installations = [r#"{"id":7}"#.to_owned(), r#"{"id":9}"#.to_owned()];
        let first: Vec<String> = (0..100).map(|i| repo_json(&format!("me/r{i}"))).collect();
        let second: Vec<String> = (100..150).map(|i| repo_json(&format!("me/r{i}"))).collect();
        http.reply(200, &page("installations", 2, &installations))
            .reply(200, &page("repositories", 150, &first))
            .reply(200, &page("repositories", 150, &second))
            .reply(200, &page("repositories", 1, &[repo_json("org/journal")]));

        let repos = github(&http).installation_repos("ghu_a").unwrap();
        assert_eq!(repos.len(), 151);
        assert_eq!(repos[0].full_name, "me/r0");
        assert_eq!(repos[149].full_name, "me/r149");
        assert_eq!(
            repos[150],
            RepoInfo {
                full_name: "org/journal".into(),
                private: true,
                clone_url: "https://github.com/org/journal.git".into(),
                default_branch: "main".into(),
            }
        );

        let urls: Vec<String> = http.requests().into_iter().map(|r| r.url).collect();
        assert_eq!(
            urls,
            [
                "https://api.github.com/user/installations?per_page=100&page=1",
                "https://api.github.com/user/installations/7/repositories?per_page=100&page=1",
                "https://api.github.com/user/installations/7/repositories?per_page=100&page=2",
                "https://api.github.com/user/installations/9/repositories?per_page=100&page=1",
            ]
        );
    }

    #[test]
    fn installation_repos_skips_suspended_installations() {
        let http = MockHttp::new();
        let installations = [
            r#"{"id":7,"suspended_at":"2026-11-01T12:00:00Z"}"#.to_owned(),
            r#"{"id":9,"suspended_at":null}"#.to_owned(),
        ];
        http.reply(200, &page("installations", 2, &installations))
            .reply(200, &page("repositories", 1, &[repo_json("me/journal")]));

        let repos = github(&http).installation_repos("ghu_a").unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(
            http.requests()[1].url,
            "https://api.github.com/user/installations/9/repositories?per_page=100&page=1"
        );
    }

    #[test]
    fn installation_repos_stops_at_total_count() {
        let http = MockHttp::new();
        let full: Vec<String> = (0..100).map(|i| repo_json(&format!("me/r{i}"))).collect();
        http.reply(200, &page("installations", 1, &[r#"{"id":7}"#.to_owned()]))
            .reply(200, &page("repositories", 100, &full));
        assert_eq!(
            github(&http).installation_repos("ghu_a").unwrap().len(),
            100
        );
        assert_eq!(http.requests().len(), 2);
    }

    #[test]
    fn repo_looks_up_full_name() {
        let http = MockHttp::new();
        http.reply(200, &repo_json("dcxSt/journal"));
        let repo = github(&http).repo("ghp_a", "dcxSt/journal").unwrap();
        assert_eq!(repo.clone_url, "https://github.com/dcxSt/journal.git");
        assert_eq!(
            http.requests()[0].url,
            "https://api.github.com/repos/dcxSt/journal"
        );
    }

    #[test]
    fn repo_not_found_is_readable() {
        let http = MockHttp::new();
        http.reply(404, r#"{"message":"Not Found"}"#);
        let err = github(&http).repo("ghp_a", "dcxSt/nope").unwrap_err();
        assert_eq!(
            err.to_string(),
            "GitHub request failed: dcxSt/nope doesn't exist, or the token can't access it"
        );
    }

    #[test]
    fn repo_rejects_malformed_names_without_a_request() {
        let http = MockHttp::new();
        for name in [
            "", "journal", "a/b/c", "../user", "a/..", "a b/c", "/repo", "owner/",
        ] {
            let err = github(&http).repo("ghp_a", name).unwrap_err();
            assert!(matches!(err, Error::Invalid(_)), "{name}: {err:?}");
        }
        assert!(http.requests().is_empty());
        assert!(is_full_name("dcx-St/my_journal.md"));
    }

    #[test]
    fn repo_info_serializes_camel_case() {
        let repo: RepoInfo = serde_json::from_str(&repo_json("a/b")).unwrap();
        assert_eq!(
            serde_json::to_string(&repo).unwrap(),
            r#"{"fullName":"a/b","private":true,"cloneUrl":"https://github.com/a/b.git","defaultBranch":"main"}"#
        );
    }

    #[test]
    fn urls() {
        assert_eq!(
            GitHub::new_repo_url("journal"),
            "https://github.com/new?name=journal&visibility=private"
        );
        assert_eq!(
            GitHub::new_repo_url("my notes&x"),
            "https://github.com/new?name=my+notes%26x&visibility=private"
        );
        assert_eq!(
            github(&MockHttp::new()).install_url(),
            "https://github.com/apps/mdgj-test/installations/new"
        );
    }

    #[test]
    fn debug_redacts_tokens() {
        let tokens = Tokens {
            access_token: "ghu_secret".into(),
            expires_at: Some(1),
            refresh_token: Some("ghr_secret".into()),
            refresh_expires_at: None,
        };
        let debug = format!("{tokens:?}");
        assert!(!debug.contains("secret"), "{debug}");
    }
}
