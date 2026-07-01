use std::fs;
use std::path::{Path, PathBuf};

fn source_files() -> Vec<PathBuf> {
    let mut files = vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("flake.nix"),
    ];
    collect_rs(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    files.into_iter().filter(|path| path.exists()).collect()
}

fn collect_rs(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read source dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_rs(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

#[test]
fn no_forbidden_clipboard_authority_dependencies_or_uses() {
    let forbidden = [
        concat!("data", "_control"),
        concat!("ext_", "data", "_control"),
        concat!("zwlr_", "data", "_control"),
        concat!("virtual", "_keyboard"),
        "ydotool",
        concat!("wl", "-copy"),
        concat!("wl", "-paste"),
        "keyring",
        "stoolap",
        "aes-gcm",
        "SetClipboard",
        "ClearHistory",
        "DeleteItem",
        "SetPinned",
        "Persistence",
        "recvmsg",
        "sendmsg",
        "SCM_RIGHTS",
    ];
    for path in source_files() {
        let content = fs::read_to_string(&path).expect("read file");
        for needle in forbidden {
            assert!(
                !content.contains(needle),
                "{} contains forbidden picker authority marker {needle}",
                path.display()
            );
        }
    }
}

#[test]
fn picker_does_not_depend_on_compositor_ipc_environment() {
    let env_name = concat!("NIRI", "_SOCKET");
    for path in source_files() {
        let content = fs::read_to_string(&path).expect("read file");
        assert!(
            !content.contains(env_name),
            "{} must not depend on compositor IPC env vars",
            path.display()
        );
    }
}
