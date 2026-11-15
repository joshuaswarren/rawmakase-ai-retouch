//! AI Remove on this computer: the `local` generator runs a model on the
//! user's machine, after downloading it once. The model here is a tiny stand-in
//! with LaMa's interface (see `tests/fixtures/make-fill128.py`) that returns
//! mid-grey for the hole and the input elsewhere, so the tests check this
//! program's plumbing (mask, scaling, paste-back, download) with no real
//! weights and no network.
// ONNX Runtime has no binary for Intel Macs, where the generator only says so.
#![cfg(not(all(target_os = "macos", target_arch = "x86_64")))]
mod support;

use rawmakase_extension::ErrorCode;
use rawmakase_extension::protocol::method::RetouchGenerate;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};
use support::{H, Reply, Server, W};

const MODEL: &[u8] = include_bytes!("fixtures/fill128.onnx");
const GOOD_PATH: &str = "/lama.onnx";

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn local_request() -> RetouchGenerate {
    RetouchGenerate {
        generator: rawmakase_ai_retouch::LOCAL_GENERATOR.into(),
        ..support::request()
    }
}

/// A config whose model comes from `server` and is pinned to `MODEL`.
fn config(server: &Server) -> rawmakase_ai_retouch::Config {
    let mut config = support::config(server, &[]);
    config.lama = rawmakase_ai_retouch::ModelFile {
        url: format!("{}{GOOD_PATH}", server.url),
        sha256: sha256(MODEL),
        bytes: MODEL.len() as u64,
    };
    config
}

fn serving_the_model() -> Server {
    Server::start(|r| match r.path.as_str() {
        GOOD_PATH => Reply::bytes("application/octet-stream", MODEL.to_vec()),
        _ => Reply::json(404, json!({})),
    })
}

fn distance(x: usize, y: usize) -> f64 {
    ((x as f64 - 20.0).powi(2) + (y as f64 - 24.0).powi(2)).sqrt()
}

#[test]
fn local_removal_fills_the_painted_area_keeps_the_rest_and_needs_no_key() {
    let server = serving_the_model();
    // No key anywhere: the local generator needs none.
    let host = support::start(config(&server), &[]);
    let (answer, result) = host
        .generate_retouch(local_request(), &support::image(), &support::coverage())
        .unwrap();
    assert_eq!(answer.model, "lama");
    assert!(
        answer.model_key.starts_with("local|lama|"),
        "{}",
        answer.model_key
    );
    let original = support::image();
    let stride = support::request().image.stride as usize;
    for y in 0..H as usize {
        for x in 0..W as usize {
            let got = support::pixel(&result, x, y);
            if distance(x, y) <= 8.0 {
                for c in got {
                    assert!((i32::from(c) - 128).abs() <= 3, "{x},{y} {got:?}");
                }
            } else if distance(x, y) >= 16.0 {
                let was: [u8; 3] = original[y * stride + x * 3..][..3].try_into().unwrap();
                assert_eq!(got, was, "{x},{y}");
            }
        }
    }
    // One download, with no credentials, and nothing else on the network.
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, GOOD_PATH);
    assert_eq!(requests[0].header("authorization"), None);
    host.finish().unwrap();
}

#[test]
fn the_model_is_downloaded_once_and_kept() {
    let server = serving_the_model();
    let host = support::start(config(&server), &[]);
    for _ in 0..2 {
        host.generate_retouch(local_request(), &support::image(), &support::coverage())
            .unwrap();
    }
    assert_eq!(server.requests().len(), 1);
    host.finish().unwrap();
}

#[test]
fn a_download_that_does_not_match_its_checksum_is_refused_and_not_kept() {
    let hits = std::sync::Arc::new(AtomicUsize::new(0));
    let seen = hits.clone();
    let server = Server::start(move |r| {
        seen.fetch_add(1, Ordering::SeqCst);
        let mut tampered = MODEL.to_vec();
        *tampered.last_mut().unwrap() ^= 1;
        // As a real server answers: a range past the end is a 416. A copy of
        // the bad file left behind would be asked to continue and get one.
        match r.header("range") {
            Some(_) => Reply::json(416, json!({})),
            None => Reply::bytes("application/octet-stream", tampered),
        }
    });
    let host = support::start(config(&server), &[]);
    for attempt in 1..=2 {
        let error = host
            .generate_retouch(local_request(), &support::image(), &support::coverage())
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Unavailable);
        assert!(error.message.contains("checksum"), "{}", error.message);
        // Nothing unverified stays behind to be trusted next time.
        assert_eq!(hits.load(Ordering::SeqCst), attempt);
    }
    host.finish().unwrap();
}

#[test]
fn a_short_download_resumes_where_it_stopped() {
    let cut = MODEL.len() / 3;
    let server = Server::start(move |r| {
        let from = r
            .header("range")
            .and_then(|v| v.strip_prefix("bytes="))
            .and_then(|v| v.strip_suffix('-'))
            .and_then(|v| v.parse::<usize>().ok());
        match from {
            // The first answer ends early, as a dropped connection does.
            None => Reply::bytes("application/octet-stream", MODEL[..cut].to_vec()),
            Some(from) => Reply {
                status: 206,
                ..Reply::bytes("application/octet-stream", MODEL[from..].to_vec())
            },
        }
    });
    let host = support::start(config(&server), &[]);
    let error = host
        .generate_retouch(local_request(), &support::image(), &support::coverage())
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Unavailable);
    assert!(error.message.contains("send it again"), "{}", error.message);
    host.generate_retouch(local_request(), &support::image(), &support::coverage())
        .unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].header("range"),
        Some(format!("bytes={cut}-").as_str())
    );
    host.finish().unwrap();
}

#[test]
fn a_service_error_while_downloading_says_what_to_do() {
    let server = Server::start(|_| Reply::json(503, json!({})));
    let host = support::start(config(&server), &[]);
    let error = host
        .generate_retouch(local_request(), &support::image(), &support::coverage())
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Unavailable);
    assert!(error.message.contains("503"), "{}", error.message);
    host.finish().unwrap();
}
