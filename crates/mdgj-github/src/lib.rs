//! GitHub App device flow, token refresh, token storage and REST calls.

mod client;
mod error;
mod http;
#[cfg(test)]
mod mock;
mod store;
mod tokens;

pub use client::{
    app_slug, client_id, is_placeholder, DeviceCode, GitHub, Poll, RepoInfo, Tokens, User,
    DEFAULT_APP_SLUG, DEFAULT_CLIENT_ID,
};
pub use error::{Error, Result};
pub use http::{Http, ReqwestHttp};
pub use store::{AuthKind, FileStore, KeyringStore, StoredAuth, TokenStore};
pub use tokens::TokenManager;
