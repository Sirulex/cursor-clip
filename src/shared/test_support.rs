// Run tests using the normal config, store and socket paths in their own process.
// Changing those environment variables inside the parallel test harness is unsafe.
pub(crate) fn isolated_test(run: impl FnOnce()) {
    use std::{env, fs, process::Command};
    let thread = std::thread::current();
    let name = thread.name().expect("Rust test harness thread name");
    if env::var("CURSOR_CLIP_ISOLATED_TEST").as_deref() == Ok(name) {
        run();
        return;
    }
    let dir = env::temp_dir().join(format!("cc-test-{:x}", rand::random::<u64>()));
    fs::create_dir(&dir).unwrap();
    let mut command = Command::new(env::current_exe().unwrap());
    if let (Some(runtime), Some(display)) = (
        env::var_os("XDG_RUNTIME_DIR"),
        env::var_os("WAYLAND_DISPLAY"),
    ) {
        command.env(
            "WAYLAND_DISPLAY",
            std::path::PathBuf::from(runtime).join(display),
        );
    }
    let output = command
        .args(["--exact", name, "--include-ignored", "--nocapture"])
        .env("CURSOR_CLIP_ISOLATED_TEST", name)
        .env("HOME", &dir)
        .env("XDG_RUNTIME_DIR", &dir)
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .output()
        .unwrap();
    fs::remove_dir_all(dir).unwrap();
    assert!(
        output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
