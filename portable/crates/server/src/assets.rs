//! Immutable, bounded HTTPS assets loaded by an operator at startup. Requests
//! only look up canonical URLs; no client URL ever becomes a filesystem path.
use crate::Error;
use std::{collections::BTreeMap, io::Read, path::Path, sync::Arc};
const TOTAL: usize = 32 * 1024 * 1024;
const FILE: usize = 8 * 1024 * 1024;
#[derive(Clone)]
struct Asset {
    bytes: Arc<[u8]>,
    mime: &'static str,
}
pub struct Bundle {
    assets: BTreeMap<String, Asset>,
    bytes: usize,
}
pub struct Response {
    pub head: Vec<u8>,
    pub body: Arc<[u8]>,
}
fn error(s: &str) -> Error {
    Error::Protocol(s.into())
}
fn canonical(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= 1024
        && (path == "/"
            || path[1..].split('/').all(|part| {
                !part.is_empty()
                    && part != "."
                    && part != ".."
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
            }))
}
fn mime(path: &str) -> Option<&'static str> {
    Some(match path.rsplit('.').next()? {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "map" => "application/json",
        _ => return None,
    })
}
impl Bundle {
    pub fn load(root: &Path) -> Result<Self, Error> {
        let mut bundle = Self {
            assets: BTreeMap::new(),
            bytes: 0,
        };
        bundle.directory(root, "", 0)?;
        if !bundle.assets.contains_key("/index.html") {
            return Err(error("Web asset bundle is missing index.html"));
        }
        Ok(bundle)
    }
    fn directory(&mut self, root: &Path, prefix: &str, depth: usize) -> Result<(), Error> {
        let meta = std::fs::symlink_metadata(root)?;
        if depth > 8 || !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(error("Web asset directories must be real and bounded"));
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 {
                return Err(error("Web asset reparse directory is forbidden"));
            }
        }
        for entry in std::fs::read_dir(root)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| error("Web asset names must be UTF-8"))?;
            let url = format!("{prefix}/{name}");
            if !canonical(&url) {
                return Err(error("Web asset name is not a canonical URL"));
            }
            let meta = std::fs::symlink_metadata(entry.path())?;
            if meta.file_type().is_symlink() {
                return Err(error("Web asset symlinks are forbidden"));
            }
            if meta.is_dir() {
                self.directory(&entry.path(), &url, depth + 1)?;
                continue;
            }
            if !meta.is_file() {
                return Err(error("Web assets must be regular files"));
            }
            let Some(mime) = mime(&url) else { continue };
            if self.assets.len() >= 512
                || meta.len() > FILE as u64
                || self.bytes.saturating_add(meta.len() as usize) > TOTAL
            {
                return Err(error("Web asset bundle exceeds its budget"));
            }
            let mut options = std::fs::OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.custom_flags(0x00200000);
            }
            let file = options.open(entry.path())?;
            let meta = file.metadata()?;
            if !meta.is_file() {
                return Err(error("Web asset changed during startup"));
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if meta.file_attributes() & 0x400 != 0 {
                    return Err(error("Web asset reparse file is forbidden"));
                }
            }
            let mut bytes = vec![];
            file.take(FILE as u64 + 1).read_to_end(&mut bytes)?;
            self.bytes = self.bytes.saturating_add(bytes.len());
            if bytes.len() > FILE || self.bytes > TOTAL {
                return Err(error("Web asset bundle exceeds its budget"));
            }
            self.assets.insert(
                url,
                Asset {
                    bytes: bytes.into(),
                    mime,
                },
            );
        }
        Ok(())
    }
    /// Single HTTP request, no body, pipelining, encoded paths or content negotiation.
    pub fn response(&self, header: &[u8]) -> Result<Response, Error> {
        let text = std::str::from_utf8(header).map_err(|_| error("Invalid HTTP header"))?;
        if header.len() > 16384
            || !text.ends_with("\r\n\r\n")
            || text
                .bytes()
                .any(|b| b == 0 || b == 127 || (b < 32 && !matches!(b, b'\r' | b'\n')))
        {
            return Err(error("Invalid HTTP header"));
        }
        let mut lines = text.split("\r\n");
        let mut request = lines.next().unwrap_or("").split(' ');
        let method = request.next().unwrap_or("");
        let path = request.next().unwrap_or("");
        let version = request.next().unwrap_or("");
        if !["GET", "HEAD"].contains(&method)
            || version != "HTTP/1.1"
            || request.next().is_some()
            || !canonical(path)
        {
            return Err(error("Unsupported asset request"));
        }
        let mut fields = BTreeMap::new();
        for line in lines {
            if line.is_empty() {
                continue;
            }
            let (key, value) = line
                .split_once(':')
                .ok_or_else(|| error("Malformed HTTP header"))?;
            if key.is_empty()
                || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                || value.bytes().any(|b| b < 32 || b == 127)
                || fields
                    .insert(key.to_ascii_lowercase(), value.trim())
                    .is_some()
            {
                return Err(error("Duplicate or malformed HTTP header"));
            }
        }
        if fields.get("host").map_or(true, |host| host.is_empty())
            || fields.contains_key("content-length")
            || fields.contains_key("transfer-encoding")
            || fields.contains_key("upgrade")
        {
            return Err(error("HTTP asset request cannot contain a body or Upgrade"));
        }
        let path = if path == "/" { "/index.html" } else { path };
        let asset = self.assets.get(path);
        let status = if asset.is_some() {
            "200 OK"
        } else {
            "404 Not Found"
        };
        let mime = asset.map_or("text/plain; charset=utf-8", |a| a.mime);
        let body = asset.map_or_else(
            || Arc::<[u8]>::from(&b"Not found\n"[..]),
            |a| a.bytes.clone(),
        );
        let head=format!("HTTP/1.1 {status}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-cache\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nCross-Origin-Opener-Policy: same-origin\r\nX-Frame-Options: DENY\r\nContent-Security-Policy: default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self' wss:; worker-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'\r\n\r\n",body.len()).into_bytes();
        Ok(Response {
            head,
            body: if method == "HEAD" {
                Arc::from([])
            } else {
                body
            },
        })
    }
}
