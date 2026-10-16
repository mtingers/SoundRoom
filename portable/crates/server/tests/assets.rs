use soundroom_server::assets::Bundle;
use std::path::PathBuf;
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "soundroom-web-assets-{}",
            soundroom_client::auth::new_id().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(
            root.join("index.html"),
            b"<script type=module src=app.js></script>",
        )
        .unwrap();
        std::fs::write(root.join("app.js"), b"export const localAudio=false;").unwrap();
        std::fs::write(root.join("credential.json"), b"never a public asset").unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn header(method: &str, path: &str) -> Vec<u8> {
    format!("{method} {path} HTTP/1.1\r\nHost: studio.example\r\n\r\n").into_bytes()
}
#[test]
fn immutable_https_bundle_serves_known_assets_and_security_headers() {
    let fixture = Fixture::new();
    let bundle = Bundle::load(&fixture.0).unwrap();
    std::fs::write(fixture.0.join("app.js"), b"changed after startup").unwrap();
    let response = bundle.response(&header("GET", "/app.js")).unwrap();
    assert_eq!(&*response.body, b"export const localAudio=false;");
    let head = std::str::from_utf8(&response.head).unwrap();
    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(head.contains("X-Content-Type-Options: nosniff"));
    assert!(head.contains("Content-Security-Policy:"));
    assert!(head.contains("Connection: close"));
    let html = bundle.response(&header("GET", "/")).unwrap();
    assert!(std::str::from_utf8(&html.head)
        .unwrap()
        .contains("text/html"));
    let head = bundle.response(&header("HEAD", "/app.js")).unwrap();
    assert!(head.body.is_empty());
    assert!(std::str::from_utf8(&head.head).unwrap().contains(&format!(
        "Content-Length: {}",
        b"export const localAudio=false;".len()
    )));
    assert!(std::str::from_utf8(
        &bundle
            .response(&header("GET", "/credential.json"))
            .unwrap()
            .head
    )
    .unwrap()
    .contains("404 Not Found"));
}
#[test]
fn traversal_bodies_duplicates_encoded_paths_and_header_injection_fail() {
    let fixture = Fixture::new();
    let bundle = Bundle::load(&fixture.0).unwrap();
    for path in [
        "/../registry.json",
        "/%2e%2e/registry.json",
        "//app.js",
        "/a/./app.js",
        "/a\\app.js",
        "/app.js?secret=1",
        "/app.js#hash",
    ] {
        assert!(bundle.response(&header("GET", path)).is_err(), "{path}");
    }
    for value in [
        "GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: a\r\nContent-Length: 0\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: a\r\nUpgrade: websocket\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: a\nInjected: x\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: \r\n\r\n",
        "POST / HTTP/1.1\r\nHost: a\r\n\r\n",
    ] {
        assert!(bundle.response(value.as_bytes()).is_err(), "{value:?}");
    }
}
#[test]
fn preload_rejects_large_files_missing_index_and_deep_trees() {
    let f = Fixture::new();
    let file = std::fs::File::create(f.0.join("oversized.js")).unwrap();
    file.set_len(8 * 1024 * 1024 + 1).unwrap();
    assert!(Bundle::load(&f.0).is_err());
    std::fs::remove_file(f.0.join("oversized.js")).unwrap();
    let mut path = f.0.clone();
    for _ in 0..9 {
        path = path.join("deep");
        std::fs::create_dir(&path).unwrap();
    }
    assert!(Bundle::load(&f.0).is_err());
    let f = Fixture::new();
    std::fs::remove_file(f.0.join("index.html")).unwrap();
    assert!(Bundle::load(&f.0).is_err());
}
#[cfg(unix)]
#[test]
fn symlink_assets_and_directories_are_rejected() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    symlink(f.0.join("app.js"), f.0.join("alias.js")).unwrap();
    assert!(Bundle::load(&f.0).is_err());
    std::fs::remove_file(f.0.join("alias.js")).unwrap();
    symlink(&f.0, f.0.join("loop")).unwrap();
    assert!(Bundle::load(&f.0).is_err());
}
