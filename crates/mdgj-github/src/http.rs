//! HTTP transport trait and the blocking reqwest + rustls implementation.

use std::error::Error as _;

use reqwest::blocking::{Client, RequestBuilder};

use crate::error::{Error, Result};

/// The two requests GitHub needs, as a trait so the client can be tested without a network.
///
/// Implementations return the status and body of every response they receive; only a
/// failure to get a response at all is an error ([`Error::Network`]).
pub trait Http: Send + Sync {
    /// POSTs `form` as `application/x-www-form-urlencoded`.
    fn post_form(
        &self,
        url: &str,
        form: &[(&str, &str)],
        headers: &[(&str, &str)],
    ) -> Result<(u16, String)>;
    fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<(u16, String)>;
}

/// Blocking reqwest over rustls (ring), verifying certificates against the OS trust store.
///
/// Don't use it from inside an async task; run it on a blocking thread.
pub struct ReqwestHttp {
    client: Client,
}

impl ReqwestHttp {
    pub fn new() -> Result<Self> {
        // reqwest is built without a default crypto provider. Fails only if one is already set.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = Client::builder().build().map_err(network)?;
        Ok(Self { client })
    }
}

impl Http for ReqwestHttp {
    fn post_form(
        &self,
        url: &str,
        form: &[(&str, &str)],
        headers: &[(&str, &str)],
    ) -> Result<(u16, String)> {
        send(self.client.post(url).form(form), headers)
    }

    fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<(u16, String)> {
        send(self.client.get(url), headers)
    }
}

fn send(mut request: RequestBuilder, headers: &[(&str, &str)]) -> Result<(u16, String)> {
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = request.send().map_err(network)?;
    let status = response.status().as_u16();
    let body = response.text().map_err(network)?;
    Ok((status, body))
}

/// reqwest's own message is only "error sending request"; the cause is in the source chain.
fn network(err: reqwest::Error) -> Error {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    Error::Network(message)
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    /// Serves one request on loopback; returns the URL to call and a handle that yields the
    /// raw request.
    fn serve_once(
        status: &'static str,
        body: &'static str,
    ) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/path?q=1", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut request = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
                request.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            let mut content = vec![0; length];
            reader.read_exact(&mut content).unwrap();
            request.push_str(&String::from_utf8(content).unwrap());
            let reply = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            reader.get_mut().write_all(reply.as_bytes()).unwrap();
            request
        });
        (url, server)
    }

    #[test]
    fn post_form_sends_encoded_form_and_headers() {
        let (url, server) = serve_once("200 OK", r#"{"ok":true}"#);
        let http = ReqwestHttp::new().unwrap();
        let reply = http
            .post_form(
                &url,
                &[("a", "1 2"), ("b", "x&y")],
                &[("Accept", "application/json")],
            )
            .unwrap();
        assert_eq!(reply, (200, r#"{"ok":true}"#.to_owned()));
        let request = server.join().unwrap().to_ascii_lowercase();
        assert!(
            request.starts_with("post /path?q=1 http/1.1\r\n"),
            "{request}"
        );
        assert!(
            request.contains("accept: application/json\r\n"),
            "{request}"
        );
        assert!(
            request.contains("content-type: application/x-www-form-urlencoded\r\n"),
            "{request}"
        );
        assert!(request.ends_with("\r\n\r\na=1+2&b=x%26y"), "{request}");
    }

    #[test]
    fn get_returns_error_statuses_as_responses() {
        let (url, server) = serve_once("404 Not Found", "nope");
        let http = ReqwestHttp::new().unwrap();
        let reply = http.get(&url, &[("Authorization", "Bearer t")]).unwrap();
        assert_eq!(reply, (404, "nope".to_owned()));
        let request = server.join().unwrap();
        assert!(
            request.starts_with("GET /path?q=1 HTTP/1.1\r\n"),
            "{request}"
        );
        assert!(request.contains("authorization: Bearer t\r\n"), "{request}");
    }

    #[test]
    fn unreachable_host_is_a_network_error() {
        // Bind then drop, so the port is very likely closed.
        let addr = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let err = ReqwestHttp::new()
            .unwrap()
            .get(&format!("http://{addr}/"), &[])
            .unwrap_err();
        assert!(matches!(err, Error::Network(_)), "{err:?}");
        assert!(
            err.to_string().starts_with("Couldn't reach GitHub: "),
            "{err}"
        );
    }
}
