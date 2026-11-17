//! Token manager: hands out a valid access token, refreshing and persisting as needed.

use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::client::GitHub;
use crate::error::{Error, Result};
use crate::http::Http;
use crate::store::{AuthKind, StoredAuth, TokenStore};

/// Refresh this long before the access token expires, so it can't expire mid-operation.
const REFRESH_MARGIN: i64 = 5 * 60;

pub struct TokenManager<S: TokenStore, H: Http> {
    store: S,
    github: GitHub<H>,
    // Held from load to save: refresh tokens are single-use, so two callers refreshing
    // at once would sign the user out. Saving a new sign-in waits for it too, so a
    // refresh still in flight can't overwrite it.
    refresh: Mutex<()>,
}

impl<S: TokenStore, H: Http> TokenManager<S, H> {
    pub fn new(store: S, github: GitHub<H>) -> Self {
        Self {
            store,
            github,
            refresh: Mutex::new(()),
        }
    }

    /// For reading; change the saved sign-in through [`save`](Self::save) and
    /// [`clear`](Self::clear).
    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn github(&self) -> &GitHub<H> {
        &self.github
    }

    /// Saves a new sign-in, replacing the old one.
    pub fn save(&self, auth: &StoredAuth) -> Result<()> {
        let _guard = self.lock();
        self.store.save(auth)
    }

    /// Forgets the sign-in. It stays valid on GitHub until the user revokes it there.
    pub fn clear(&self) -> Result<()> {
        let _guard = self.lock();
        self.store.clear()
    }

    /// The saved access token, refreshed first if it is about to expire; `None` when
    /// signed out. Personal access tokens are never refreshed.
    pub fn access_token(&self) -> Result<Option<String>> {
        let _guard = self.lock();
        let Some(mut auth) = self.store.load()? else {
            return Ok(None);
        };
        let now = self.github.now();
        let due = auth.kind == AuthKind::GithubApp
            && auth
                .tokens
                .expires_at
                .is_some_and(|at| at.saturating_sub(REFRESH_MARGIN) < now);
        if !due {
            return Ok(Some(auth.tokens.access_token));
        }
        let refresh_token = match &auth.tokens.refresh_token {
            Some(token) if auth.tokens.refresh_expires_at.is_none_or(|at| at > now) => token,
            _ => return Err(Error::ReauthRequired),
        };
        auth.tokens = self.github.refresh(refresh_token)?;
        self.store.save(&auth)?;
        Ok(Some(auth.tokens.access_token))
    }

    // The guarded data is `()`, so a panic elsewhere can't leave it inconsistent.
    fn lock(&self) -> MutexGuard<'_, ()> {
        self.refresh.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use super::*;
    use crate::client::Tokens;
    use crate::mock::{MemoryStore, MockHttp, NOW};

    const HOUR: i64 = 60 * 60;

    fn auth(kind: AuthKind, expires_at: Option<i64>) -> StoredAuth {
        StoredAuth {
            kind,
            tokens: Tokens {
                access_token: "ghu_old".into(),
                expires_at,
                refresh_token: Some("ghr_old".into()),
                refresh_expires_at: Some(NOW + 180 * 24 * HOUR),
            },
            login: Some("dcxSt".into()),
        }
    }

    fn manager(http: &MockHttp, auth: Option<StoredAuth>) -> TokenManager<MemoryStore, MockHttp> {
        let store = auth.map_or_else(MemoryStore::default, MemoryStore::with);
        TokenManager::new(store, GitHub::for_tests(http.clone(), "Iv1.test"))
    }

    fn reply_refreshed(http: &MockHttp) {
        http.reply(
            200,
            r#"{"access_token":"ghu_new","expires_in":28800,"refresh_token":"ghr_new",
                "refresh_token_expires_in":15897600}"#,
        );
    }

    #[test]
    fn signed_out_has_no_token() {
        let http = MockHttp::new();
        assert_eq!(manager(&http, None).access_token().unwrap(), None);
        assert!(http.requests().is_empty());
    }

    #[test]
    fn fresh_token_is_returned_as_is() {
        let http = MockHttp::new();
        let tokens = manager(&http, Some(auth(AuthKind::GithubApp, Some(NOW + HOUR))));
        assert_eq!(tokens.access_token().unwrap().as_deref(), Some("ghu_old"));
        assert!(http.requests().is_empty());
    }

    #[test]
    fn non_expiring_app_token_is_never_refreshed() {
        let http = MockHttp::new();
        let tokens = manager(&http, Some(auth(AuthKind::GithubApp, None)));
        assert_eq!(tokens.access_token().unwrap().as_deref(), Some("ghu_old"));
        assert!(http.requests().is_empty());
    }

    #[test]
    fn pat_is_never_refreshed() {
        let http = MockHttp::new();
        let tokens = manager(&http, Some(auth(AuthKind::Pat, Some(NOW - HOUR))));
        assert_eq!(tokens.access_token().unwrap().as_deref(), Some("ghu_old"));
        assert!(http.requests().is_empty());
    }

    #[test]
    fn token_near_expiry_is_refreshed_and_rotation_persisted() {
        let http = MockHttp::new();
        reply_refreshed(&http);
        let tokens = manager(&http, Some(auth(AuthKind::GithubApp, Some(NOW + 4 * 60))));

        assert_eq!(tokens.access_token().unwrap().as_deref(), Some("ghu_new"));
        let [request] = http.requests().try_into().unwrap();
        assert_eq!(request.field("grant_type"), Some("refresh_token"));
        assert_eq!(request.field("refresh_token"), Some("ghr_old"));
        assert_eq!(
            tokens.store().load().unwrap(),
            Some(StoredAuth {
                kind: AuthKind::GithubApp,
                tokens: Tokens {
                    access_token: "ghu_new".into(),
                    expires_at: Some(NOW + 28800),
                    refresh_token: Some("ghr_new".into()),
                    refresh_expires_at: Some(NOW + 15897600),
                },
                login: Some("dcxSt".into()),
            })
        );

        // The rotated token is now fresh: no second request.
        assert_eq!(tokens.access_token().unwrap().as_deref(), Some("ghu_new"));
        assert_eq!(http.requests().len(), 1);
    }

    #[test]
    fn expired_token_is_refreshed() {
        let http = MockHttp::new();
        reply_refreshed(&http);
        let tokens = manager(&http, Some(auth(AuthKind::GithubApp, Some(NOW - HOUR))));
        assert_eq!(tokens.access_token().unwrap().as_deref(), Some("ghu_new"));
    }

    #[test]
    fn rejected_refresh_token_requires_reauth() {
        let http = MockHttp::new();
        http.reply(200, r#"{"error":"bad_refresh_token"}"#);
        let saved = auth(AuthKind::GithubApp, Some(NOW - HOUR));
        let tokens = manager(&http, Some(saved.clone()));

        let err = tokens.access_token().unwrap_err();
        assert!(matches!(err, Error::ReauthRequired), "{err:?}");
        assert_eq!(tokens.store().load().unwrap(), Some(saved));
    }

    #[test]
    fn expired_refresh_token_requires_reauth_without_a_request() {
        let http = MockHttp::new();
        let mut saved = auth(AuthKind::GithubApp, Some(NOW - HOUR));
        saved.tokens.refresh_expires_at = Some(NOW - 1);
        let err = manager(&http, Some(saved)).access_token().unwrap_err();
        assert!(matches!(err, Error::ReauthRequired), "{err:?}");
        assert!(http.requests().is_empty());
    }

    #[test]
    fn missing_refresh_token_requires_reauth() {
        let http = MockHttp::new();
        let mut saved = auth(AuthKind::GithubApp, Some(NOW - HOUR));
        saved.tokens.refresh_token = None;
        let err = manager(&http, Some(saved)).access_token().unwrap_err();
        assert!(matches!(err, Error::ReauthRequired), "{err:?}");
    }

    #[test]
    fn network_failure_keeps_the_saved_tokens() {
        let http = MockHttp::new();
        http.fail(Error::Network("offline".into()));
        let saved = auth(AuthKind::GithubApp, Some(NOW - HOUR));
        let tokens = manager(&http, Some(saved.clone()));
        assert!(matches!(tokens.access_token(), Err(Error::Network(_))));
        assert_eq!(tokens.store().load().unwrap(), Some(saved));
    }

    #[test]
    fn concurrent_callers_refresh_once() {
        // A second refresh request would find no queued reply and panic.
        let http = MockHttp::slow(Duration::from_millis(50));
        reply_refreshed(&http);
        let tokens = manager(&http, Some(auth(AuthKind::GithubApp, Some(NOW - HOUR))));

        thread::scope(|s| {
            let callers: Vec<_> = (0..4).map(|_| s.spawn(|| tokens.access_token())).collect();
            for caller in callers {
                assert_eq!(caller.join().unwrap().unwrap().as_deref(), Some("ghu_new"));
            }
        });
        assert_eq!(http.requests().len(), 1);
    }

    #[test]
    fn a_sign_in_saved_during_a_refresh_is_kept() {
        let http = MockHttp::slow(Duration::from_millis(200));
        reply_refreshed(&http);
        let tokens = manager(&http, Some(auth(AuthKind::GithubApp, Some(NOW - HOUR))));
        let mut pat = auth(AuthKind::Pat, None);
        pat.tokens.access_token = "ghp_new".into();

        thread::scope(|s| {
            let refresh = s.spawn(|| tokens.access_token());
            // The refresh request is out, so the refresh holds the lock.
            while http.requests().is_empty() {
                thread::sleep(Duration::from_millis(1));
            }
            tokens.save(&pat).unwrap();
            assert_eq!(refresh.join().unwrap().unwrap().as_deref(), Some("ghu_new"));
        });
        assert_eq!(tokens.store().load().unwrap(), Some(pat));
        assert_eq!(tokens.access_token().unwrap().as_deref(), Some("ghp_new"));
    }

    #[test]
    fn clear_signs_out() {
        let http = MockHttp::new();
        let tokens = manager(&http, Some(auth(AuthKind::Pat, None)));
        tokens.clear().unwrap();
        assert_eq!(tokens.access_token().unwrap(), None);
    }
}
