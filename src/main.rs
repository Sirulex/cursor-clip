use clap::Parser;
use d2b_clip_picker::placement::PickerPlacement;
use d2b_clip_picker::protocol::{IpcPeer, OpenRequest};
use d2b_clip_picker::ui;
use log::{debug, error, info, warn};
use rustix::io::{FdFlags, fcntl_getfd, fcntl_setfd};
use std::env;
use std::os::fd::FromRawFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Parser)]
#[command(
    name = "d2b-clip-picker",
    version,
    about = "UI-only d2b clipboard picker"
)]
struct Args {
    /// Inherited socketpair file descriptor from d2b-clipd.
    #[arg(long = "ipc-fd")]
    ipc_fd: i32,
    /// Test harness only: activate the first rendered row after the UI maps.
    #[arg(long = "test-select-first", hide = true)]
    test_select_first: bool,

    /// Optional generic JSON palette for UI shell colors.
    #[arg(long = "theme-json", value_name = "PATH")]
    theme_json: Option<PathBuf>,
}

fn main() {
    force_headless_safe_gtk_defaults();
    if let Err(err) = run() {
        error!("picker failed: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .format_timestamp_secs()
        .try_init()
        .ok();

    let args = Args::parse();
    if args.ipc_fd <= 2 {
        return Err("--ipc-fd must be greater than 2".into());
    }

    let stream = unsafe { UnixStream::from_raw_fd(args.ipc_fd) };
    let flags = fcntl_getfd(&stream).map_err(|error| {
        format!(
            "--ipc-fd {} is not an open inherited file descriptor: {error}",
            args.ipc_fd,
        )
    })?;
    fcntl_setfd(&stream, flags | FdFlags::CLOEXEC)
        .map_err(|error| format!("set --ipc-fd close-on-exec: {error}"))?;
    let mut peer = IpcPeer::new(stream)?;
    peer.send_client_hello()?;
    let request = match peer.read_clipd_frame()? {
        d2b_clip_picker::protocol::ClipdFrame::OpenRequest(request) => *request,
        d2b_clip_picker::protocol::ClipdFrame::Close { .. } => return Ok(()),
        d2b_clip_picker::protocol::ClipdFrame::Error { code, .. } => {
            return Err(format!("clipd rejected picker request: {code}").into());
        }
    };

    let placement = choose_placement(&request);
    debug!("starting picker UI");
    let theme = match args.theme_json.as_deref() {
        Some(path) => ui::ThemePalette::from_json_file(path)?,
        None => ui::ThemePalette::default(),
    };

    ui::run_picker(request, peer, placement, args.test_select_first, theme)?;
    Ok(())
}

fn choose_placement(request: &OpenRequest) -> PickerPlacement {
    if let Some(hints) = request.placement_hints.as_ref() {
        if hints.pointer_x.is_some() && hints.pointer_y.is_some() {
            return PickerPlacement::from_hints(hints);
        }
        if hints.output.is_some() {
            return PickerPlacement::from_hints(hints);
        }
        match d2b_clip_picker::placement::PointerCapture::capture_picker_timeout(
            Duration::from_millis(500),
        ) {
            Ok(mut captured) => {
                if let Some(width) = hints.overlay_width {
                    captured.geometry.overlay_width = width;
                }
                if let Some(height) = hints.overlay_height {
                    captured.geometry.overlay_height = height;
                }
                apply_hint_output_if_missing(&mut captured, hints.output.clone());
                info!(
                    "picker pointer capture placement x={} y={} output_width={} output_height={} output={:?}",
                    captured.geometry.x,
                    captured.geometry.y,
                    captured.geometry.output_width,
                    captured.geometry.output_height,
                    captured.output
                );
                return captured;
            }
            Err(error) => {
                warn!("picker pointer capture failed, using placement hints: {error}");
            }
        }
        return PickerPlacement::from_hints(hints);
    }

    match d2b_clip_picker::placement::PointerCapture::capture_picker_timeout(Duration::from_millis(
        500,
    )) {
        Ok(placement) => {
            info!(
                "picker pointer capture placement x={} y={} output_width={} output_height={} output={:?}",
                placement.geometry.x,
                placement.geometry.y,
                placement.geometry.output_width,
                placement.geometry.output_height,
                placement.output
            );
            placement
        }
        Err(error) => {
            warn!("picker pointer capture failed, using default placement: {error}");
            PickerPlacement::default()
        }
    }
}

fn apply_hint_output_if_missing(placement: &mut PickerPlacement, hint_output: Option<String>) {
    if placement.output.is_none() {
        placement.output = hint_output;
    }
}

fn force_headless_safe_gtk_defaults() {
    if env::var_os("GDK_BACKEND").is_none() {
        unsafe {
            env::set_var("GDK_BACKEND", "wayland");
        }
    }
    if env::var_os("GSK_RENDERER").is_none() {
        unsafe {
            env::set_var("GSK_RENDERER", "cairo");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use d2b_clip_picker::placement::Placement;

    #[test]
    fn pointer_capture_uses_destination_output_hint_when_capture_has_no_output() {
        let mut placement = PickerPlacement {
            geometry: Placement {
                x: 10.0,
                y: 20.0,
                overlay_width: 420,
                overlay_height: 520,
                output_width: 2560,
                output_height: 1440,
            },
            output: None,
        };

        apply_hint_output_if_missing(&mut placement, Some("DP-3".to_owned()));

        assert_eq!(placement.output.as_deref(), Some("DP-3"));
        assert_eq!(placement.geometry.x, 10.0);
        assert_eq!(placement.geometry.y, 20.0);
    }

    #[test]
    fn pointer_capture_keeps_exact_output_when_available() {
        let mut placement = PickerPlacement {
            geometry: Placement::default(),
            output: Some("HDMI-A-1".to_owned()),
        };

        apply_hint_output_if_missing(&mut placement, Some("DP-3".to_owned()));

        assert_eq!(placement.output.as_deref(), Some("HDMI-A-1"));
    }

    #[test]
    fn choose_placement_prefers_destination_output_hint_without_pointer() {
        let request = OpenRequest {
            selected_protocol_version: 1,
            clipd_version: "test".to_owned(),
            picker_version: "test".to_owned(),
            request_id: "req".to_owned(),
            destination: d2b_clip_picker::protocol::DestinationMetadata {
                realm: "personal-dev".to_owned(),
                realm_kind: d2b_clip_picker::protocol::RealmKind::Vm,
                application: Some("Firefox".to_owned()),
                app_id: Some("d2b.personal-dev.firefox".to_owned()),
                title: None,
                workspace: None,
                output: Some("DP-3".to_owned()),
                attribution: Some(d2b_clip_picker::protocol::AttributionQuality::ExactClient),
            },
            requested_mime_type: "text/plain".to_owned(),
            expires_at_unix_ms: Some(1),
            placement_hints: Some(d2b_clip_picker::protocol::PlacementHints {
                pointer_x: None,
                pointer_y: None,
                output_width: None,
                output_height: None,
                overlay_width: Some(420),
                overlay_height: Some(520),
                output: Some("DP-3".to_owned()),
            }),
            candidates: Vec::new(),
        };

        let placement = choose_placement(&request);

        assert_eq!(placement.output.as_deref(), Some("DP-3"));
        assert_eq!(placement.geometry.x, 24.0);
        assert_eq!(placement.geometry.y, 24.0);
    }
}
