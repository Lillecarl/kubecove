use super::{parse_binary_frame, valid_id, MAX_TUNNEL_PAYLOAD_BYTES};

// EOF must release the upload sender without cancelling response delivery.
// Stale IDs must not affect any other connection, and repeated EOF is harmless.
#[tokio::test]
async fn upload_eof_preserves_connection_until_response_finishes() {
    let registry = super::TunnelRegistry::default();
    let id = "123e4567-e89b-12d3-a456-426614174000";
    let (incoming, mut receiver) = tokio::sync::mpsc::channel(1);
    let task = tokio::spawn(std::future::pending());
    registry.state.lock().unwrap().connections.insert(
        id.to_string(),
        super::TunnelConnection {
            forward_id: id.to_string(),
            incoming: Some(incoming),
            task,
        },
    );
    assert!(!registry.finish_upload("123e4567-e89b-12d3-a456-426614174001"));
    assert!(registry.finish_upload(id));
    assert!(registry.finish_upload(id));
    assert_eq!(receiver.recv().await, None);
    assert!(!registry.state.lock().unwrap().connections[id]
        .task
        .is_finished());
    assert!(registry.close_connection(id));
    assert!(registry.state.lock().unwrap().connections.is_empty());
}

#[test]
fn accepts_only_canonical_lowercase_uuid_ids() {
    assert!(valid_id("123e4567-e89b-12d3-a456-426614174000"));
    for value in [
        "",
        "123e4567e89b12d3a456426614174000",
        "123E4567-E89B-12D3-A456-426614174000",
        "123e4567-e89b-12d3-a456-42661417400g",
        "123e4567-e89b-12d3-a456-426614174000x",
    ] {
        assert!(!valid_id(value), "accepted {value}");
    }
}

#[test]
fn parses_payload_without_copying_and_preserves_every_byte() {
    let id = b"123e4567-e89b-12d3-a456-426614174000";
    let mut frame = Vec::new();
    frame.extend_from_slice(&u16::try_from(id.len()).unwrap().to_be_bytes());
    frame.extend_from_slice(id);
    frame.extend_from_slice(&[0, 255, 4, 0, 13]);

    let (parsed_id, payload) = parse_binary_frame(&frame).expect("valid frame");
    assert_eq!(parsed_id, "123e4567-e89b-12d3-a456-426614174000");
    assert_eq!(payload, [0, 255, 4, 0, 13]);
}

#[test]
fn rejects_truncated_unknown_and_oversized_frames_before_copying_payload() {
    assert!(parse_binary_frame(&[]).is_none());
    assert!(parse_binary_frame(&[0, 0]).is_none());

    let mut unknown_id = Vec::from(2_u16.to_be_bytes());
    unknown_id.extend_from_slice(b"xx");
    assert!(parse_binary_frame(&unknown_id).is_none());

    let id = b"123e4567-e89b-12d3-a456-426614174000";
    let mut oversized = Vec::new();
    oversized.extend_from_slice(&u16::try_from(id.len()).unwrap().to_be_bytes());
    oversized.extend_from_slice(id);
    oversized.resize(oversized.len() + MAX_TUNNEL_PAYLOAD_BYTES + 1, 0);
    assert!(parse_binary_frame(&oversized).is_none());
}
