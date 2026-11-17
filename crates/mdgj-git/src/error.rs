//! Error type with user-readable messages.

use git2::{ErrorClass, ErrorCode};

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("The commit message is empty.")]
    EmptyMessage,
    #[error("Sign-in failed: {0}")]
    Auth(String),
    #[error("Network error: {0}")]
    Network(String),
    #[error("No remote repository is connected.")]
    NotConnected,
    #[error("Git error: {0}")]
    Git(String),
    #[error("File error: {0}")]
    Io(String),
}

impl From<git2::Error> for Error {
    fn from(e: git2::Error) -> Self {
        let message = e.message().to_owned();
        let status = message
            .strip_prefix("unexpected http status code: ")
            .and_then(|code| code.parse::<u16>().ok());
        match (e.code(), e.class(), status) {
            (ErrorCode::Auth, ..) => Error::Auth(message),
            (_, _, Some(code @ (401 | 403))) => Error::Auth(format!(
                "access denied (HTTP {code}); check that the token can write to this repository"
            )),
            // GitHub answers 404 for private repositories the token can't see.
            (_, _, Some(404)) => Error::Auth(
                "the repository was not found, or this sign-in has no access to it (HTTP 404)"
                    .into(),
            ),
            (_, _, Some(code)) => Error::Network(format!("the server answered HTTP {code}")),
            (_, ErrorClass::Net | ErrorClass::Http | ErrorClass::Ssl, _) => Error::Network(message),
            (_, ErrorClass::Os, _) => Error::Io(message),
            _ => Error::Git(message),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(code: ErrorCode, class: ErrorClass, message: &str) -> Error {
        git2::Error::new(code, class, message).into()
    }

    #[test]
    fn libgit2_errors_become_readable_kinds() {
        let http = |status: u16| {
            let message = format!("unexpected http status code: {status}");
            map(ErrorCode::GenericError, ErrorClass::Http, &message)
        };
        assert!(matches!(http(401), Error::Auth(_)));
        assert!(matches!(http(403), Error::Auth(_)));
        assert!(matches!(http(404), Error::Auth(_)));
        assert_eq!(
            http(502),
            Error::Network("the server answered HTTP 502".into())
        );
        let offline = "failed to resolve address for github.com";
        assert_eq!(
            map(ErrorCode::GenericError, ErrorClass::Net, offline),
            Error::Network(offline.into())
        );
        assert!(matches!(
            map(
                ErrorCode::Auth,
                ErrorClass::Http,
                "unexpected authentication failure"
            ),
            Error::Auth(_)
        ));
        assert!(matches!(
            map(ErrorCode::Locked, ErrorClass::Index, "index is locked"),
            Error::Git(_)
        ));
    }
}
