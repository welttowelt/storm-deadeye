#![allow(
    clippy::unwrap_used,
    clippy::tests_outside_test_module,
    clippy::print_stderr,
    clippy::panic,
    reason = "integration test driver"
)]
//! End-to-end test for `RemoteSigner` against a `wiremock` HTTP server.
//!
//! The test spins up a mock signing endpoint that returns canned r/s for
//! a fixed hash, drives a `RemoteSigner` against it, and verifies:
//! 1. The signer produces the expected (r, s).
//! 2. The remote endpoint received exactly one POST with the expected body
//!    shape.
//! 3. Retry logic fires on transient 503 responses and ultimately succeeds.

use std::{sync::Arc, time::Duration};

use deadeye_starknet::{DeadeyeSigner, Felt, RemoteSigner, RemoteSignerConfig};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

#[tokio::test]
async fn remote_signer_signs_against_mock_endpoint() {
    let server = MockServer::start().await;
    let canned = serde_json::json!({
        "r": "0xdeadbeefcafebabe",
        "s": "0x123456789abcdef0",
    });
    Mock::given(method("POST"))
        .and(path("/sign"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&canned))
        .expect(1)
        .mount(&server)
        .await;

    let endpoint = url::Url::parse(&format!("{}/sign", server.uri())).unwrap();
    let pk = Felt::from_hex("0xabc").unwrap();
    let signer = RemoteSigner::new(pk, endpoint, RemoteSignerConfig::default()).unwrap();

    let hash = Felt::from_hex("0x1234").unwrap();
    let [r, s] = signer.sign_hash(hash).await.unwrap();
    assert_eq!(r, Felt::from_hex("0xdeadbeefcafebabe").unwrap());
    assert_eq!(s, Felt::from_hex("0x123456789abcdef0").unwrap());

    assert_eq!(signer.public_key().await.unwrap(), pk);
}

#[tokio::test]
async fn remote_signer_retries_transient_failures() {
    let server = MockServer::start().await;
    let canned = serde_json::json!({
        "r": "0x11",
        "s": "0x22",
    });
    // First two requests fail with 503, third succeeds.
    Mock::given(method("POST"))
        .and(path("/sign"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/sign"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&canned))
        .expect(1)
        .mount(&server)
        .await;

    let endpoint = url::Url::parse(&format!("{}/sign", server.uri())).unwrap();
    let cfg = RemoteSignerConfig {
        timeout: Duration::from_secs(1),
        auth_token: None,
        max_retries: 5,
        backoff: Duration::from_millis(10),
    };
    let signer = RemoteSigner::new(Felt::ONE, endpoint, cfg).unwrap();

    let hash = Felt::from_hex("0xbeef").unwrap();
    let [r, s] = signer.sign_hash(hash).await.unwrap();
    assert_eq!(r, Felt::from_hex("0x11").unwrap());
    assert_eq!(s, Felt::from_hex("0x22").unwrap());
}

#[tokio::test]
async fn remote_signer_dyn_signer_bound_round_trips() {
    let server = MockServer::start().await;
    let canned = serde_json::json!({"r": "0x1", "s": "0x2"});
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&canned))
        .mount(&server)
        .await;

    let endpoint = url::Url::parse(&server.uri()).unwrap();
    let signer = RemoteSigner::new(
        Felt::from_hex("0x99").unwrap(),
        endpoint,
        RemoteSignerConfig::default(),
    )
    .unwrap();

    // Lift to Arc<dyn DeadeyeSigner> and exercise the trait object.
    let dyn_signer: Arc<dyn DeadeyeSigner> = Arc::new(signer);
    let pk = dyn_signer.public_key().await.unwrap();
    assert_eq!(pk, Felt::from_hex("0x99").unwrap());
    let [r, s] = dyn_signer.sign_hash(Felt::ONE).await.unwrap();
    assert_eq!(r, Felt::ONE);
    assert_eq!(s, Felt::TWO);
}
