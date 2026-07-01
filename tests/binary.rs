//! Binary integration tests for d2b-clip-picker.
//!
//! These tests spawn the compiled picker binary directly and exercise the
//! ADR 0042 protocol contract at the process boundary, verifying that the
//! binary exits non-zero on malformed or unexpected input rather than
//! silently succeeding.

use std::io::{BufRead, BufReader, Write};
use std::os::fd::BorrowedFd;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};

fn spawn_picker() -> (UnixStream, Child) {
    let (parent, child) = UnixStream::pair().expect("socketpair");
    let child_fd = child.as_raw_fd();

    let mut command = Command::new(env!("CARGO_BIN_EXE_d2b-clip-picker"));
    command.arg("--ipc-fd").arg(child_fd.to_string());
    unsafe {
        command.pre_exec(move || {
            let borrowed = BorrowedFd::borrow_raw(child_fd);
            let flags = rustix::io::fcntl_getfd(borrowed)
                .map_err(|err| std::io::Error::from_raw_os_error(err.raw_os_error()))?;
            rustix::io::fcntl_setfd(borrowed, flags - rustix::io::FdFlags::CLOEXEC)
                .map_err(|err| std::io::Error::from_raw_os_error(err.raw_os_error()))
        });
    }

    let child_proc = command.spawn().expect("spawn d2b-clip-picker");

    drop(child);
    (parent, child_proc)
}

fn read_client_hello(parent: &UnixStream) {
    parent
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .expect("set timeout");
    let mut reader = BufReader::new(parent.try_clone().expect("clone socket"));
    let mut hello = String::new();
    reader.read_line(&mut hello).expect("read client_hello");
    assert!(
        hello.contains("client_hello"),
        "picker must send client_hello first, got: {hello}"
    );
}

/// Picker must exit non-zero when it receives a malformed / unrecognised
/// clipd frame type.
///
/// The picker uses `serde(deny_unknown_fields)` on `ClipdFrame` so any frame
/// that cannot be deserialized as a known variant propagates an error through
/// `run()` which calls `process::exit(1)`.
#[test]
fn picker_exits_nonzero_on_malformed_clipd_frame() {
    let (parent, mut child_proc) = spawn_picker();
    read_client_hello(&parent);

    // Send a frame with an unknown `type` field. ClipdFrame uses
    // `deny_unknown_fields` + `serde(tag = "type")`, so this will fail
    // deserialization and the picker must exit non-zero.
    let mut writer = parent.try_clone().expect("clone socket for write");
    writeln!(
        writer,
        r#"{{"type":"totally_bogus_type_that_does_not_exist_in_clipd_frame"}}"#
    )
    .expect("write malformed frame");
    drop(writer);
    drop(parent);

    let status = child_proc.wait().expect("wait for picker");
    assert!(
        !status.success(),
        "picker must exit non-zero on receiving an unknown/malformed clipd frame type; \
         exit code was: {:?}",
        status.code()
    );
}

#[test]
fn picker_exits_nonzero_on_clipd_error_frame() {
    let (parent, mut child_proc) = spawn_picker();
    read_client_hello(&parent);

    let mut writer = parent.try_clone().expect("clone socket for write");
    writeln!(
        writer,
        r#"{{"type":"error","selected_protocol_version":1,"request_id":"req","code":"picker_not_configured"}}"#
    )
    .expect("write error frame");
    drop(writer);
    drop(parent);

    let status = child_proc.wait().expect("wait for picker");
    assert!(
        !status.success(),
        "picker must exit non-zero when clipd rejects the request; exit code was: {:?}",
        status.code()
    );
}

#[test]
fn picker_exits_zero_on_clipd_close_frame() {
    let (parent, mut child_proc) = spawn_picker();
    read_client_hello(&parent);

    let mut writer = parent.try_clone().expect("clone socket for write");
    writeln!(
        writer,
        r#"{{"type":"close","selected_protocol_version":1,"request_id":"req","code":"request_expired"}}"#
    )
    .expect("write close frame");
    drop(writer);
    drop(parent);

    let status = child_proc.wait().expect("wait for picker");
    assert!(
        status.success(),
        "picker must exit cleanly on an explicit clipd close frame; exit code was: {:?}",
        status.code()
    );
}

/// Picker must exit non-zero when clipd closes the socket immediately without
/// sending an open_request (e.g., a too-old-version rejection path).
#[test]
fn picker_exits_nonzero_on_immediate_socket_close() {
    let (parent, mut child_proc) = spawn_picker();
    read_client_hello(&parent);

    // Close the parent socket immediately — simulates clipd hanging up.
    drop(parent);

    let status = child_proc.wait().expect("wait");
    assert!(
        !status.success(),
        "picker must exit non-zero when clipd disconnects without a Close frame; exit code was: {:?}",
        status.code()
    );
}
