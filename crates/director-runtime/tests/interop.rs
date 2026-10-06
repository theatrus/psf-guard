use psf_guard_director_runtime::interop::astrocollab::{decode_tonight, Source};

#[test]
fn runtime_consumes_the_shared_read_only_adapter_without_new_ipc() {
    let source = Source::new("https://collab.example", "000000000001", false).unwrap();
    let bytes = include_bytes!("../../director-interop/tests/fixtures/starfront-tonight.json");
    let nightly = decode_tonight(bytes, &source, "2026-10-05").unwrap();
    let shared =
        psf_guard_director_interop::astrocollab::decode_tonight(bytes, &source, "2026-10-05")
            .unwrap();
    assert_eq!(nightly, shared);
    assert_eq!(nightly.shares[0].demands.len(), 6);
    assert!(nightly.shares[0]
        .demands
        .iter()
        .all(|v| v.exposure_ms == 300_000 && v.requested_frames == 11));
}
