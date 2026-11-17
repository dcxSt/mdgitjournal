use std::io;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors from journal, image and settings operations. `Display` is meant for the user.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A bad argument, such as a file name with a path in it, or a file that isn't UTF-8 text.
    #[error("{0}")]
    Invalid(String),
    /// The named entry file does not exist.
    #[error("{0} does not exist")]
    NotFound(String),
    /// The image could not be decoded or encoded.
    #[error("{0}")]
    Image(String),
    #[error("{0}")]
    Io(String),
}

impl Error {
    pub(crate) fn io(context: impl std::fmt::Display, err: io::Error) -> Self {
        Self::Io(format!("{context}: {err}"))
    }

    /// Maps "not found" to [`Error::NotFound`] for `file`, anything else to [`Error::Io`].
    pub(crate) fn file(action: &str, file: &str, err: io::Error) -> Self {
        if err.kind() == io::ErrorKind::NotFound {
            Self::NotFound(file.to_owned())
        } else {
            Self::io(format_args!("could not {action} {file}"), err)
        }
    }
}
