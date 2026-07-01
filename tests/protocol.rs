use d2b_clip_picker::protocol::{
    AttributionQuality, Candidate, ClipdFrame, DestinationMetadata, IpcPeer, OpenRequest,
    PlacementHints, RealmKind, sanitize_preview,
};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

fn sample_request() -> OpenRequest {
    OpenRequest {
        selected_protocol_version: 1,
        clipd_version: "0.1.0".to_owned(),
        picker_version: "0.1.0".to_owned(),
        request_id: "req-1".to_owned(),
        destination: DestinationMetadata {
            realm: "personal".to_owned(),
            realm_kind: RealmKind::Vm,
            application: Some("Firefox".to_owned()),
            app_id: Some("firefox".to_owned()),
            title: None,
            workspace: Some("1".to_owned()),
            output: Some("DP-1".to_owned()),
            attribution: Some(AttributionQuality::ExactClient),
        },
        requested_mime_type: "text/plain".to_owned(),
        expires_at_unix_ms: Some(1_800_000_000_000),
        placement_hints: Some(PlacementHints {
            pointer_x: Some(20.0),
            pointer_y: Some(30.0),
            output_width: Some(1920),
            output_height: Some(1080),
            overlay_width: Some(420),
            overlay_height: Some(520),
            output: Some("DP-1".to_owned()),
        }),
        candidates: vec![Candidate {
            entry_id: "entry-1".to_owned(),
            source_realm: "host".to_owned(),
            source_realm_kind: RealmKind::Host,
            source_app: Some("Terminal".to_owned()),
            source_app_id: Some("org.example.Terminal".to_owned()),
            source_attribution: AttributionQuality::FocusedWindowGuess,
            preview_text: Some("hello".to_owned()),
            content_type: "text/plain".to_owned(),
            timestamp_unix_ms: Some(1_700_000_000_000),
            thumbnail_png_base64: None,
            byte_count: Some(5),
            confirmation_required: false,
        }],
    }
}

#[test]
fn client_hello_contains_only_protocol_range_and_picker_version() {
    let (client, server) = UnixStream::pair().expect("socketpair");
    let peer = IpcPeer::new(client).expect("peer");
    peer.send_client_hello().expect("hello");

    let mut line = String::new();
    BufReader::new(server).read_line(&mut line).expect("line");
    let value: serde_json::Value = serde_json::from_str(line.trim_end()).expect("json");
    assert_eq!(value["type"], "client_hello");
    assert!(value.get("protocol_version_range").is_some());
    assert!(value.get("picker_version").is_some());
    assert!(value.get("request_id").is_none());
}

#[test]
fn fake_clipd_select_roundtrip() {
    let (client, mut server) = UnixStream::pair().expect("socketpair");
    let mut peer = IpcPeer::new(client).expect("peer");
    peer.send_client_hello().expect("hello");

    let mut hello = String::new();
    let mut server_reader = BufReader::new(server.try_clone().expect("clone"));
    server_reader.read_line(&mut hello).expect("hello line");

    let request = sample_request();
    let frame = ClipdFrame::OpenRequest(Box::new(request.clone()));
    writeln!(server, "{}", serde_json::to_string(&frame).expect("encode")).expect("write");
    let decoded = peer.read_clipd_frame().expect("open request");
    assert!(matches!(decoded, ClipdFrame::OpenRequest(_)));

    peer.tx_for_request(&request)
        .select("entry-1")
        .expect("select");
    let mut select = String::new();
    server_reader.read_line(&mut select).expect("select line");
    let value: serde_json::Value = serde_json::from_str(select.trim_end()).expect("json");
    assert_eq!(value["type"], "select");
    assert_eq!(value["request_id"], "req-1");
    assert_eq!(value["entry_id"], "entry-1");
}

#[test]
fn fake_clipd_cancel_roundtrip() {
    let (client, mut server) = UnixStream::pair().expect("socketpair");
    let mut peer = IpcPeer::new(client).expect("peer");
    peer.send_client_hello().expect("hello");

    let mut server_reader = BufReader::new(server.try_clone().expect("clone"));
    let mut hello = String::new();
    server_reader.read_line(&mut hello).expect("hello line");

    let request = sample_request();
    writeln!(
        server,
        "{}",
        serde_json::to_string(&ClipdFrame::OpenRequest(Box::new(request.clone()))).expect("encode")
    )
    .expect("write");
    peer.read_clipd_frame().expect("open request");
    peer.tx_for_request(&request).cancel().expect("cancel");

    let mut cancel = String::new();
    server_reader.read_line(&mut cancel).expect("cancel line");
    let value: serde_json::Value = serde_json::from_str(cancel.trim_end()).expect("json");
    assert_eq!(value["type"], "cancel");
    assert_eq!(value["request_id"], "req-1");
    assert!(value.get("entry_id").is_none());
}

#[test]
fn previews_are_inert_plain_text() {
    let sanitized = sanitize_preview("<b>x</b>\u{1b}[31m\nnext\u{0007}", 128);
    assert_eq!(sanitized, "<b>x</b> next�");
}
