use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};

pub const PROTOCOL_MIN: u16 = 1;
pub const PROTOCOL_MAX: u16 = 1;
pub const MAX_PICKER_FRAME_BYTES: usize = 4 * 1024;
pub const MAX_OPEN_REQUEST_BYTES: usize = 23_553_408;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolVersionRange {
    pub min: u16,
    pub max: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PickerFrame {
    ClientHello {
        protocol_version_range: ProtocolVersionRange,
        picker_version: String,
    },
    Select {
        selected_protocol_version: u16,
        request_id: String,
        entry_id: String,
    },
    Cancel {
        selected_protocol_version: u16,
        request_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClipdFrame {
    OpenRequest(Box<OpenRequest>),
    Error {
        selected_protocol_version: Option<u16>,
        request_id: Option<String>,
        code: String,
    },
    Close {
        selected_protocol_version: Option<u16>,
        request_id: Option<String>,
        code: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenRequest {
    pub selected_protocol_version: u16,
    pub clipd_version: String,
    pub picker_version: String,
    pub request_id: String,
    pub destination: DestinationMetadata,
    pub requested_mime_type: String,
    pub expires_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub placement_hints: Option<PlacementHints>,
    pub candidates: Vec<Candidate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct DestinationMetadata {
    pub realm: String,
    pub realm_kind: RealmKind,
    #[serde(default)]
    pub application: Option<String>,
    #[serde(default)]
    pub app_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub attribution: Option<AttributionQuality>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RealmKind {
    Host,
    Vm,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttributionQuality {
    ExactClient,
    FocusedWindowGuess,
    CacheStaleFocusedWindowGuess,
    BrokerInjectedDebug,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementHints {
    #[serde(default)]
    pub pointer_x: Option<f64>,
    #[serde(default)]
    pub pointer_y: Option<f64>,
    #[serde(default)]
    pub output_width: Option<i32>,
    #[serde(default)]
    pub output_height: Option<i32>,
    #[serde(default)]
    pub overlay_width: Option<i32>,
    #[serde(default)]
    pub overlay_height: Option<i32>,
    #[serde(default)]
    pub output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub entry_id: String,
    pub source_realm: String,
    pub source_realm_kind: RealmKind,
    #[serde(default)]
    pub source_app: Option<String>,
    #[serde(default)]
    pub source_app_id: Option<String>,
    pub source_attribution: AttributionQuality,
    #[serde(default)]
    pub preview_text: Option<String>,
    pub content_type: String,
    #[serde(default)]
    pub timestamp_unix_ms: Option<u64>,
    #[serde(default)]
    pub thumbnail_png_base64: Option<String>,
    #[serde(default)]
    pub byte_count: Option<u64>,
    #[serde(default)]
    pub confirmation_required: bool,
}

#[derive(Clone)]
pub struct PickerTx {
    stream: Arc<Mutex<UnixStream>>,
    selected_protocol_version: u16,
    request_id: String,
}

impl PickerTx {
    pub fn select(&self, entry_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.send(PickerFrame::Select {
            selected_protocol_version: self.selected_protocol_version,
            request_id: self.request_id.clone(),
            entry_id: entry_id.to_owned(),
        })
    }

    pub fn cancel(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.send(PickerFrame::Cancel {
            selected_protocol_version: self.selected_protocol_version,
            request_id: self.request_id.clone(),
        })
    }

    fn send(&self, frame: PickerFrame) -> Result<(), Box<dyn std::error::Error>> {
        let encoded = serde_json::to_vec(&frame)?;
        if encoded.len() > MAX_PICKER_FRAME_BYTES {
            return Err("picker frame exceeded size cap".into());
        }
        let mut stream = self
            .stream
            .lock()
            .map_err(|_| "picker socket lock poisoned")?;
        stream.write_all(&encoded)?;
        stream.write_all(b"\n")?;
        stream.flush()?;
        Ok(())
    }
}

pub struct IpcPeer {
    reader: BufReader<UnixStream>,
    writer: Arc<Mutex<UnixStream>>,
}

impl IpcPeer {
    pub fn new(stream: UnixStream) -> Result<Self, Box<dyn std::error::Error>> {
        stream.set_nonblocking(false)?;
        let writer_stream = stream.try_clone()?;
        Ok(Self {
            reader: BufReader::new(stream),
            writer: Arc::new(Mutex::new(writer_stream)),
        })
    }

    pub fn send_client_hello(&self) -> Result<(), Box<dyn std::error::Error>> {
        PickerTx {
            stream: self.writer.clone(),
            selected_protocol_version: PROTOCOL_MAX,
            request_id: String::new(),
        }
        .send(PickerFrame::ClientHello {
            protocol_version_range: ProtocolVersionRange {
                min: PROTOCOL_MIN,
                max: PROTOCOL_MAX,
            },
            picker_version: crate::VERSION.to_owned(),
        })
    }

    pub fn read_clipd_frame(&mut self) -> Result<ClipdFrame, Box<dyn std::error::Error>> {
        let line = read_bounded_line(&mut self.reader, MAX_OPEN_REQUEST_BYTES)?;
        Ok(serde_json::from_str(line.trim_end())?)
    }

    pub fn tx_for_request(&self, request: &OpenRequest) -> PickerTx {
        PickerTx {
            stream: self.writer.clone(),
            selected_protocol_version: request.selected_protocol_version,
            request_id: request.request_id.clone(),
        }
    }

    pub fn into_reader(self) -> BufReader<UnixStream> {
        self.reader
    }
}

pub fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut buf = Vec::new();
    let read = reader
        .take(max_bytes.saturating_add(1) as u64)
        .read_until(b'\n', &mut buf)?;
    if read == 0 {
        return Err("d2b-clipd closed picker socket".into());
    }
    if !buf.ends_with(b"\n") {
        return Err("clipd frame exceeded size cap".into());
    }
    buf.pop();
    Ok(String::from_utf8(buf)?)
}

pub fn sanitize_preview(input: &str, max_chars: usize) -> String {
    let mut output = String::new();
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if output.chars().count() >= max_chars {
            output.push('…');
            break;
        }
        if ch == '\u{1b}' {
            while let Some(next) = chars.peek().copied() {
                chars.next();
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        match ch {
            '\n' | '\r' | '\t' => output.push(' '),
            c if c.is_control() => output.push('�'),
            c => output.push(c),
        }
    }
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn display_content_kind(content_type: &str) -> (&'static str, &'static str) {
    match content_type.split(';').next().unwrap_or(content_type) {
        "text/html" => ("HTML text", "📝"),
        "text/plain" => ("Text", "📝"),
        "image/png" => ("Image", "🖼️"),
        _ => ("Item", "📄"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_reader_rejects_overlong_line() {
        let input = vec![b'a'; MAX_OPEN_REQUEST_BYTES + 1];
        let mut cursor = std::io::Cursor::new(input);
        let err = read_bounded_line(&mut cursor, MAX_OPEN_REQUEST_BYTES).unwrap_err();
        assert!(err.to_string().contains("size cap"));
    }

    #[test]
    fn sanitize_preview_strips_ansi_and_controls() {
        assert_eq!(
            sanitize_preview("hello\u{1b}[31m red\nworld", 80),
            "hello red world"
        );
    }
}
