/* update/tests/local_server.rs
 *
 * Copyright 2026 sequ
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

//! The check-and-install path against a fake releases server.
//!
//! A `std::net::TcpListener` on localhost serves a `releases/latest` JSON
//! body and a payload of our choosing, so the whole flow -- parse the
//! release, pick this machine's asset, download it over the running image --
//! runs with no network and no GitHub.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::mpsc;
use timetrack_update::{UpdateError, Version, check_for_update_with, current_arch, install_with};

/// A minimal HTTP server. `GET /releases/latest` answers a release naming
/// the given assets (each served from `/dl/<name>`); `GET /dl/<name>`
/// answers `asset`. Returns the base URL; dropping the sender stops the
/// server.
fn serve(asset: Vec<u8>) -> (String, mpsc::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        while stop_rx.try_recv().is_err() {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(std::time::Duration::from_millis(5));
                continue;
            };
            let mut buf = vec![0u8; 4096];
            let Ok(n) = stream.read(&mut buf) else {
                continue;
            };
            let req = String::from_utf8_lossy(&buf[..n]).into_owned();
            let path = req
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .unwrap_or("");
            let (status, content_type, body) = if path == "/releases/latest" {
                let json = format!(
                    r#"{{"tag_name": "v9.9.9", "assets": [{{"name": "TimeTrack-{}.AppImage", "browser_download_url": "http://{}/dl/TimeTrack-{}.AppImage"}}]}}"#,
                    current_arch(),
                    addr,
                    current_arch(),
                );
                ("200 OK", "application/json", json.into_bytes())
            } else if path.starts_with("/dl/") {
                ("200 OK", "application/octet-stream", asset.clone())
            } else {
                ("404 Not Found", "text/plain", b"nope".to_vec())
            };
            let header = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(&header.into_bytes());
            let _ = stream.write_all(&body);
        }
    });
    (format!("http://{addr}"), stop_tx)
}

fn fake_image(marker: &[u8]) -> Vec<u8> {
    let mut v = b"\x7fELF-fake-TimeTrack-".to_vec();
    v.extend_from_slice(marker);
    v.resize(1024, 0);
    v
}

fn scratch_image(name: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("tt-update-e2e-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("TimeTrack-x86_64.AppImage");
    std::fs::write(&dest, fake_image(b"old-version")).unwrap();
    (dir, dest)
}

#[test]
fn a_newer_release_installs_over_the_image() {
    let new = fake_image(b"new-version");
    let (base, _stop) = serve(new.clone());

    let release =
        check_for_update_with(&format!("{base}/releases/latest"), &Version::parse("0.1.0"))
            .expect("local server must answer")
            .expect("v9.9.9 is newer than the test build");
    assert_eq!(release.version.as_str(), "9.9.9");
    let picked = release
        .asset_for(current_arch())
        .expect("this arch's asset");
    assert!(picked.url.starts_with(&base));

    let (dir, dest) = scratch_image("install");
    install_with(&picked.url, &dest).expect("install must succeed");
    assert_eq!(std::fs::read(&dest).unwrap(), new);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(
            std::fs::metadata(&dest).unwrap().permissions().mode() & 0o111,
            0,
            "the installed image stays executable"
        );
    }
    assert!(
        !dest.with_extension("AppImage.new").exists(),
        "no temp file left behind"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_newer_release_without_this_arch_is_silence_not_error() {
    // Releases publish only the architectures in `release.yml`; an AppImage
    // on any other arch must not announce an update it cannot install.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        while stop_rx.try_recv().is_err() {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(std::time::Duration::from_millis(5));
                continue;
            };
            let body = br#"{"tag_name": "v9.9.9", "assets": [{"name": "TimeTrack-riscv64.AppImage", "browser_download_url": "http://127.0.0.1:9/x"}]}"#.to_vec();
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(&header.into_bytes());
            let _ = stream.write_all(&body);
        }
    });
    let found = check_for_update_with(
        &format!("http://{addr}/releases/latest"),
        &Version::parse("0.1.0"),
    )
    .expect("local server must answer");
    assert!(found.is_none(), "no asset for this arch means no update");
    drop(stop_tx);
}

#[test]
fn an_html_error_page_is_never_installed() {
    let (base, _stop) = serve(b"<html><body>not an image</body></html>".to_vec());
    let release =
        check_for_update_with(&format!("{base}/releases/latest"), &Version::parse("0.1.0"))
            .unwrap()
            .unwrap();

    let (dir, dest) = scratch_image("error-page");
    let before = std::fs::read(&dest).unwrap();
    let err = install_with(
        release.asset_for(current_arch()).unwrap().url.as_str(),
        &dest,
    )
    .unwrap_err();
    assert_eq!(err, UpdateError::NotAnAppImage);
    assert_eq!(
        std::fs::read(&dest).unwrap(),
        before,
        "a refused payload leaves the image untouched"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
