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

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::mpsc;
use timetrack_update::{
    UpdateError, Version, check_for_update_with, current_arch, install_with, install_with_checksum,
};

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A `SHA256SUMS` body naming this arch's asset, as `release.yml` writes it.
fn sums_for(asset: &[u8]) -> Vec<u8> {
    format!(
        "{}  TimeTrack-{}.AppImage\n",
        sha256_hex(asset),
        current_arch()
    )
    .into_bytes()
}

/// A minimal HTTP server. `GET /releases/latest` answers a release naming
/// the asset and `SHA256SUMS` (each served from `/dl/<name>`); `GET
/// /dl/<name>` answers `asset`, and `/dl/SHA256SUMS` the checksum computed
/// over it. Returns the base URL; dropping the sender stops the server.
fn serve(asset: Vec<u8>) -> (String, mpsc::Sender<()>) {
    let sums = sums_for(&asset);
    serve_inner(asset, sums, false)
}

/// Like [`serve`], but publishes the given `SHA256SUMS` body -- a wrong one
/// models a tampered or mismatched download.
fn serve_with_sums(asset: Vec<u8>, sums: Vec<u8>) -> (String, mpsc::Sender<()>) {
    serve_inner(asset, sums, false)
}

/// Like [`serve`], but the asset is sent `Transfer-Encoding: chunked` with
/// no `Content-Length` -- the shape a size cap must not trust a header for.
fn serve_chunked(asset: Vec<u8>) -> (String, mpsc::Sender<()>) {
    let sums = sums_for(&asset);
    serve_inner(asset, sums, true)
}

fn serve_inner(asset: Vec<u8>, sums: Vec<u8>, chunked: bool) -> (String, mpsc::Sender<()>) {
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
            if chunked && path.starts_with("/dl/") && !path.ends_with("SHA256SUMS") {
                // Chunked: no Content-Length, the body in sized pieces.
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                );
                for piece in asset.chunks(256) {
                    let _ = stream.write_all(format!("{:x}\r\n", piece.len()).as_bytes());
                    let _ = stream.write_all(piece);
                    let _ = stream.write_all(b"\r\n");
                }
                let _ = stream.write_all(b"0\r\n\r\n");
                continue;
            }
            let (status, content_type, body) = if path == "/releases/latest" {
                let json = format!(
                    r#"{{"tag_name": "v9.9.9", "assets": [{{"name": "TimeTrack-{arch}.AppImage", "browser_download_url": "http://{addr}/dl/TimeTrack-{arch}.AppImage"}}, {{"name": "SHA256SUMS", "browser_download_url": "http://{addr}/dl/SHA256SUMS"}}]}}"#,
                    arch = current_arch(),
                    addr = addr,
                );
                ("200 OK", "application/json", json.into_bytes())
            } else if path == "/dl/SHA256SUMS" {
                ("200 OK", "text/plain", sums.clone())
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

#[test]
fn a_tampered_download_is_refused_and_the_image_untouched() {
    // The payload is a fine ELF, but SHA256SUMS names different bytes: a
    // compromised or swapped asset must never replace the running image.
    let new = fake_image(b"tampered");
    let wrong = format!(
        "{}  TimeTrack-{}.AppImage\n",
        "0".repeat(64),
        current_arch()
    );
    let (base, _stop) = serve_with_sums(new, wrong.into_bytes());
    let release =
        check_for_update_with(&format!("{base}/releases/latest"), &Version::parse("0.1.0"))
            .unwrap()
            .unwrap();

    let (dir, dest) = scratch_image("tampered");
    let before = std::fs::read(&dest).unwrap();
    let outcome = install_with(
        release.asset_for(current_arch()).unwrap().url.as_str(),
        &dest,
    );
    assert_eq!(outcome, Err(UpdateError::ChecksumMismatch));
    assert_eq!(
        std::fs::read(&dest).unwrap(),
        before,
        "a refused payload leaves the image untouched"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// One oversized body, two wire shapes: the cap is on the byte stream, so
/// `Content-Length` and chunked must refuse identically. The cap is
/// injected small (the production one is 512 MiB) and the checksum is
/// correct, so size is the only thing that can refuse.
fn oversized_is_refused(chunked: bool, name: &str) {
    let big = fake_image(b"oversized-payload-that-keeps-going");
    assert!(big.len() > 512);
    let hash = sha256_hex(&big);
    let (base, _stop) = if chunked {
        serve_chunked(big)
    } else {
        serve(big)
    };
    let url = format!("{base}/dl/TimeTrack-{}.AppImage", current_arch());

    let (dir, dest) = scratch_image(name);
    let before = std::fs::read(&dest).unwrap();
    let err = install_with_checksum(&url, &dest, &hash, 512).unwrap_err();
    assert_eq!(err, UpdateError::TooLarge);
    assert_eq!(
        std::fs::read(&dest).unwrap(),
        before,
        "an oversized payload leaves the image untouched"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_oversized_download_with_content_length_is_refused() {
    oversized_is_refused(false, "oversized-length");
}

#[test]
fn an_oversized_chunked_download_without_length_is_refused() {
    oversized_is_refused(true, "oversized-chunked");
}

#[test]
fn a_chunked_download_within_the_cap_installs() {
    // Chunked is the normal shape behind a CDN, not an attack: a body under
    // the cap with a matching checksum installs exactly like a lengthed one.
    let new = fake_image(b"chunked-new-version");
    let (base, _stop) = serve_chunked(new.clone());
    let release =
        check_for_update_with(&format!("{base}/releases/latest"), &Version::parse("0.1.0"))
            .unwrap()
            .unwrap();

    let (dir, dest) = scratch_image("chunked-happy");
    install_with(
        release.asset_for(current_arch()).unwrap().url.as_str(),
        &dest,
    )
    .expect("chunked install must succeed");
    assert_eq!(std::fs::read(&dest).unwrap(), new);
    std::fs::remove_dir_all(&dir).unwrap();
}
