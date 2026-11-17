//! Test doubles: a scripted [`Http`] and an in-memory [`TokenStore`].

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::error::Result;
use crate::http::Http;
use crate::store::{StoredAuth, TokenStore};

/// The fixed clock of [`GitHub::for_tests`](crate::GitHub), in unix seconds.
pub(crate) const NOW: i64 = 1_800_000_000;

#[derive(Clone, Debug)]
pub(crate) struct Request {
    pub method: &'static str,
    pub url: String,
    pub form: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
}

impl Request {
    pub fn field(&self, name: &str) -> Option<&str> {
        lookup(&self.form, name)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        lookup(&self.headers, name)
    }
}

fn lookup<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

/// Records every request and answers with queued replies, in order. Panics when a
/// request arrives with nothing queued, so unexpected requests fail the test.
#[derive(Clone, Default)]
pub(crate) struct MockHttp(Arc<Inner>);

#[derive(Default)]
struct Inner {
    replies: Mutex<VecDeque<Result<(u16, String)>>>,
    requests: Mutex<Vec<Request>>,
    delay: Duration,
}

impl MockHttp {
    pub fn new() -> Self {
        Self::default()
    }

    /// Answers each request only after `delay`, to widen race windows.
    pub fn slow(delay: Duration) -> Self {
        Self(Arc::new(Inner {
            delay,
            ..Inner::default()
        }))
    }

    pub fn reply(&self, status: u16, body: &str) -> &Self {
        self.queue(Ok((status, body.to_owned())))
    }

    pub fn fail(&self, err: crate::Error) -> &Self {
        self.queue(Err(err))
    }

    pub fn requests(&self) -> Vec<Request> {
        self.0.requests.lock().unwrap().clone()
    }

    fn queue(&self, reply: Result<(u16, String)>) -> &Self {
        self.0.replies.lock().unwrap().push_back(reply);
        self
    }

    fn call(
        &self,
        method: &'static str,
        url: &str,
        form: &[(&str, &str)],
        headers: &[(&str, &str)],
    ) -> Result<(u16, String)> {
        let owned = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        self.0.requests.lock().unwrap().push(Request {
            method,
            url: url.to_owned(),
            form: owned(form),
            headers: owned(headers),
        });
        thread::sleep(self.0.delay);
        let reply = self.0.replies.lock().unwrap().pop_front();
        reply.unwrap_or_else(|| panic!("unexpected request: {method} {url}"))
    }
}

impl Http for MockHttp {
    fn post_form(
        &self,
        url: &str,
        form: &[(&str, &str)],
        headers: &[(&str, &str)],
    ) -> Result<(u16, String)> {
        self.call("POST", url, form, headers)
    }

    fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<(u16, String)> {
        self.call("GET", url, &[], headers)
    }
}

#[derive(Default)]
pub(crate) struct MemoryStore(Mutex<Option<StoredAuth>>);

impl MemoryStore {
    pub fn with(auth: StoredAuth) -> Self {
        Self(Mutex::new(Some(auth)))
    }
}

impl TokenStore for MemoryStore {
    fn load(&self) -> Result<Option<StoredAuth>> {
        Ok(self.0.lock().unwrap().clone())
    }

    fn save(&self, auth: &StoredAuth) -> Result<()> {
        *self.0.lock().unwrap() = Some(auth.clone());
        Ok(())
    }

    fn clear(&self) -> Result<()> {
        *self.0.lock().unwrap() = None;
        Ok(())
    }
}
