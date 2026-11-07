//! Helpers shared by the HTTP tests: a minimal HTTP/1.1 client and a
//! resolver over `LocalBackend`.

#![allow(dead_code)]

use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use vpsfiles_vfs::local::LocalBackend;
use vpsfiles_vfs::{Backend, BackendResolver, Result, SourceId, VfsError};

pub struct LocalOnly(pub Arc<LocalBackend>);

#[async_trait]
impl BackendResolver for LocalOnly {
    async fn backend(&self, source: &SourceId) -> Result<Arc<dyn Backend>> {
        match source {
            SourceId::Local => Ok(self.0.clone()),
            other => Err(VfsError::NotConnected(other.to_string())),
        }
    }
}

pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// Minimal HTTP/1.1 client: one request per connection.
pub async fn request(url: &str, method: &str, extra: &[(&str, &str)]) -> Reply {
    let rest = url.strip_prefix("http://").unwrap();
    let (authority, path) = rest.split_at(rest.find('/').unwrap());
    let mut sock = tokio::net::TcpStream::connect(authority).await.unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n");
    for (k, v) in extra {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    sock.write_all(req.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    sock.read_to_end(&mut raw).await.unwrap();

    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8(raw[..split].to_vec()).unwrap();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers = lines
        .map(|l| {
            let (k, v) = l.split_once(':').unwrap();
            (k.trim().to_string(), v.trim().to_string())
        })
        .collect();
    Reply { status, headers, body: raw[split + 4..].to_vec() }
}

