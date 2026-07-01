use d2b_clip_picker::protocol::{ClipdFrame, DestinationMetadata};

#[test]
fn close_frame_accepts_optional_request_id_without_protocol_version() {
    let json = r#"{"type":"close","request_id":"abc","code":"request_expired"}"#;
    let frame: ClipdFrame = serde_json::from_str(json).expect("close frame");
    assert!(matches!(frame, ClipdFrame::Close { .. }));
}

#[test]
fn destination_metadata_accepts_workspace() {
    let json = r#"{"realm":"Host","realm_kind":"host","workspace":"work","attribution":"focused_window_guess"}"#;
    let destination: DestinationMetadata = serde_json::from_str(json).expect("destination");
    assert_eq!(destination.workspace.as_deref(), Some("work"));
}
