//! Error type with user-readable messages.

use serde::Deserialize;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// GitHub rejected the token (HTTP 401).
    #[error("GitHub rejected the token: {0}")]
    Auth(String),
    /// The refresh token is expired or revoked; only a new sign-in helps.
    #[error("Your GitHub sign-in has expired. Sign in again.")]
    ReauthRequired,
    #[error("Couldn't reach GitHub: {0}")]
    Network(String),
    #[error("GitHub request failed: {message}")]
    Api { status: u16, message: String },
    /// Input that can't be sent to GitHub, such as a malformed repository name.
    #[error("{0}")]
    Invalid(String),
    #[error("Couldn't access the saved GitHub sign-in: {0}")]
    Store(String),
    #[error(
        "This build has no GitHub App client ID. Register the app (see docs/github-app.md) \
         or use a personal access token."
    )]
    Placeholder,
}

impl Error {
    /// Maps a non-success REST response, using GitHub's `message` field when there is one.
    pub(crate) fn from_response(status: u16, body: &str) -> Self {
        #[derive(Deserialize)]
        struct Body {
            message: String,
        }
        let message = serde_json::from_str::<Body>(body)
            .map(|b| b.message)
            .unwrap_or_else(|_| format!("HTTP {status}"));
        match status {
            401 => Self::Auth(message),
            _ => Self::Api { status, message },
        }
    }

    pub(crate) fn unexpected(status: u16, err: serde_json::Error) -> Self {
        Self::Api {
            status,
            message: format!("unexpected response ({err})"),
        }
    }
}
