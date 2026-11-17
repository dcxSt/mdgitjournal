//! Errors as the UI receives them: `{ kind, message }`, where the message is meant for people.

use std::fmt::Display;

use serde::Serialize;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Auth,
    Reauth,
    Network,
    NotConnected,
    EmptyMessage,
    Invalid,
    Io,
    Git,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Error {
    pub kind: Kind,
    pub message: String,
}

impl Error {
    pub fn new(kind: Kind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(Kind::Invalid, message)
    }

    pub fn io(context: &str, err: impl Display) -> Self {
        Self::new(Kind::Io, format!("{context}: {err}"))
    }

    /// A command that panicked or couldn't be scheduled.
    pub fn internal(err: impl Display) -> Self {
        Self::io("mdgitjournal hit an internal error", err)
    }
}

impl From<mdgj_core::Error> for Error {
    fn from(err: mdgj_core::Error) -> Self {
        use mdgj_core::Error as E;
        let kind = match err {
            E::Invalid(_) | E::Image(_) => Kind::Invalid,
            E::NotFound(_) | E::Io(_) => Kind::Io,
        };
        Self::new(kind, err.to_string())
    }
}

impl From<mdgj_git::Error> for Error {
    fn from(err: mdgj_git::Error) -> Self {
        use mdgj_git::Error as E;
        let kind = match err {
            E::EmptyMessage => Kind::EmptyMessage,
            E::Auth(_) => Kind::Auth,
            E::Network(_) => Kind::Network,
            E::NotConnected => Kind::NotConnected,
            E::Git(_) => Kind::Git,
            E::Io(_) => Kind::Io,
        };
        Self::new(kind, err.to_string())
    }
}

impl From<mdgj_github::Error> for Error {
    fn from(err: mdgj_github::Error) -> Self {
        use mdgj_github::Error as E;
        let kind = match err {
            E::Auth(_) | E::Api { status: 403, .. } => Kind::Auth,
            E::ReauthRequired => Kind::Reauth,
            // Worth retrying, which the sign-in flow does for network errors only.
            E::Network(_) => Kind::Network,
            E::Api { status, .. } if status >= 500 => Kind::Network,
            E::Api { .. } | E::Invalid(_) | E::Placeholder => Kind::Invalid,
            E::Store(_) => Kind::Io,
        };
        Self::new(kind, err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serializes_as_kind_and_message() {
        let err = Error::new(Kind::NotConnected, "No remote repository is connected.");
        assert_eq!(
            serde_json::to_value(err).unwrap(),
            json!({ "kind": "not_connected", "message": "No remote repository is connected." })
        );
    }

    #[test]
    fn core_errors() {
        use mdgj_core::Error as E;
        let kind = |e: E| Error::from(e).kind;
        assert_eq!(kind(E::Invalid("bad name".into())), Kind::Invalid);
        assert_eq!(kind(E::Image("not an image".into())), Kind::Invalid);
        assert_eq!(kind(E::Io("disk full".into())), Kind::Io);
        let missing = Error::from(E::NotFound("2026-11-16-1402-run.md".into()));
        assert_eq!(missing.kind, Kind::Io);
        assert_eq!(missing.message, "2026-11-16-1402-run.md does not exist");
    }

    #[test]
    fn git_errors() {
        use mdgj_git::Error as E;
        let cases = [
            (E::EmptyMessage, Kind::EmptyMessage),
            (E::Auth("no".into()), Kind::Auth),
            (E::Network("offline".into()), Kind::Network),
            (E::NotConnected, Kind::NotConnected),
            (E::Git("index is locked".into()), Kind::Git),
            (E::Io("disk full".into()), Kind::Io),
        ];
        for (err, kind) in cases {
            let message = err.to_string();
            assert_eq!(Error::from(err), Error::new(kind, message));
        }
    }

    #[test]
    fn github_errors() {
        use mdgj_github::Error as E;
        let api = |status| E::Api {
            status,
            message: "nope".into(),
        };
        let cases = [
            (E::Auth("Bad credentials".into()), Kind::Auth),
            (E::ReauthRequired, Kind::Reauth),
            (E::Network("offline".into()), Kind::Network),
            (api(403), Kind::Auth),
            (api(404), Kind::Invalid),
            // OAuth errors such as device_flow_disabled come with HTTP 200.
            (api(200), Kind::Invalid),
            (api(502), Kind::Network),
            (E::Invalid("not owner/repo".into()), Kind::Invalid),
            (E::Store("locked".into()), Kind::Io),
        ];
        for (err, kind) in cases {
            assert_eq!(Error::from(err).kind, kind);
        }
        let placeholder = Error::from(E::Placeholder);
        assert_eq!(placeholder.kind, Kind::Invalid);
        assert_eq!(placeholder.message, E::Placeholder.to_string());
    }
}
