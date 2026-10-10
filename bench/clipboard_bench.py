#!/usr/bin/env python3
"""Compare two cursor-clip builds on Hyprland.

For each history size the script starts the daemon in an isolated HOME,
fills the history through the real Wayland clipboard (wl-copy), restarts the
daemon so the history is loaded from disk, and measures:

  * daemon RSS after loading the history
  * time until a new copy shows up in the history
  * round trip of the SetHistoryPersistence call the overlay makes on open
  * time from launching the overlay until Hyprland maps its layer surface

Requirements: Hyprland, wl-clipboard, a running Secret Service (keyring),
no other cursor-clip daemon or overlay running. The overlay opens and closes
repeatedly and grabs the keyboard while open, and the clipboard is
overwritten; do not use the machine while it runs.

Usage:
  bench/clipboard_bench.py --label upstream --bin /usr/bin/cursor-clip \
      --sizes 0 10 100 --out upstream.json
  bench/clipboard_bench.py --label fork --bin target/release/cursor-clip \
      --open-cmd target/release/cursor-clip-toggle --out fork.json
"""

import argparse
import json
import os
import random
import shutil
import socket
import statistics
import struct
import subprocess
import tempfile
import time
import zlib
from pathlib import Path

RUNTIME = Path(os.environ["XDG_RUNTIME_DIR"])
SOCKET = RUNTIME / "cursor-clip" / "cursor-clip.sock"
IMAGE_EVERY = 5  # every 5th item is a screenshot-sized PNG


def make_png(seed: int, width: int = 1920, height: int | None = None) -> bytes:
    """Screenshot-like PNG (~2 MB): flat UI areas plus noisy content.

    The height varies per seed so every image has a distinct byte size;
    v1.0.2 deduplicates by a preview string that only contains the size."""
    if height is None:
        height = 1080 - (seed % 200)
    rng = random.Random(seed)
    rows = []
    for y in range(height):
        if (y // 40) % 3 == 0:
            row = os.urandom(width * 3)
        else:
            shade = bytes([(y + seed * 17) % 256, (seed * 31) % 256, 200]) * width
            row = shade
        rows.append(b"\x00" + row)
    rng.shuffle(rows[: height // 8])
    raw = b"".join(rows)

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(raw, 6))
        + chunk(b"IEND", b"")
    )


def make_text(seed: int) -> str:
    rng = random.Random(seed)
    words = "clipboard history wayland overlay rust gtk layer shell daemon paste copy".split()
    length = rng.choice([3, 12, 40, 150, 400])
    return f"[{seed}] " + " ".join(rng.choice(words) for _ in range(length))


def ipc(message) -> tuple[float, dict]:
    with socket.socket(socket.AF_UNIX) as sock:
        sock.connect(str(SOCKET))
        start = time.perf_counter()
        sock.sendall((json.dumps(message) + "\n").encode())
        buffer = b""
        while not buffer.endswith(b"\n"):
            chunk = sock.recv(1 << 20)
            if not chunk:
                break
            buffer += chunk
        return time.perf_counter() - start, json.loads(buffer)


def history_len() -> int:
    return len(ipc("GetHistory")[1]["History"]["items"])


def rss_kib(pid: int) -> int:
    for line in Path(f"/proc/{pid}/status").read_text().splitlines():
        if line.startswith("VmRSS:"):
            return int(line.split()[1])
    return 0


def overlay_mapped() -> bool:
    layers = json.loads(subprocess.check_output(["hyprctl", "layers", "-j"]))
    return any(
        layer["namespace"] == "cursor-clip"
        for monitor in layers.values()
        for level in monitor["levels"].values()
        for layer in level
    )


def wait_for(predicate, timeout: float, interval: float = 0.003) -> float | None:
    start = time.perf_counter()
    while time.perf_counter() - start < timeout:
        if predicate():
            return time.perf_counter() - start
        time.sleep(interval)
    return None


class Daemon:
    def __init__(self, binary: str, env: dict):
        self.binary = binary
        self.env = env
        self.proc = None

    def start(self):
        self.proc = subprocess.Popen(
            [self.binary, "--daemon"],
            env=self.env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        if wait_for(lambda: SOCKET.exists() and self._answers(), 120, 0.05) is None:
            raise RuntimeError("daemon did not come up")

    def _answers(self) -> bool:
        try:
            history_len()
            return True
        except OSError:
            return False

    def stop(self):
        if self.proc:
            self.proc.terminate()
            self.proc.wait(timeout=30)
            self.proc = None


def payload(index: int, images: dict[int, bytes]) -> tuple[list[str], bytes]:
    """wl-copy arguments and data for item `index` (images are cached)."""
    if index % IMAGE_EVERY == IMAGE_EVERY - 1:
        if index not in images:
            images[index] = make_png(index)
        return ["wl-copy", "-t", "image/png"], images[index]
    return ["wl-copy"], make_text(index).encode()


def copy_item(index: int, images: dict[int, bytes]):
    args, data = payload(index, images)
    subprocess.run(args, input=data, check=True)


def fill_history(count: int, images: dict[int, bytes]) -> tuple[list[float], int]:
    """Copy `count` items; returns per-copy latencies and how many copies had
    to be repeated because the daemon did not record them."""
    latencies = []
    missed = 0
    for index in range(count):
        before = history_len()
        args, data = payload(index, images)  # prepared outside the timed part
        for _attempt in range(3):
            start = time.perf_counter()
            subprocess.run(args, input=data, check=True)
            if wait_for(lambda: history_len() > before, 20, 0.01) is not None:
                break
            missed += 1
        else:
            raise RuntimeError(f"copy {index} was not recorded after 3 attempts")
        latencies.append(time.perf_counter() - start)
        time.sleep(0.15)
    return latencies, missed


def measure_overlay(open_cmd: str, env: dict, runs: int) -> dict:
    """Cold opens for every run; for a resident overlay also warm re-opens."""
    def launch():
        return subprocess.Popen(
            [open_cmd], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
        )

    def close_with_toggle():
        launch().wait(timeout=10)
        if wait_for(lambda: not overlay_mapped(), 10) is None:
            raise RuntimeError("overlay did not close")

    cold, warm = [], []
    resident = None
    for _ in range(runs):
        start = time.perf_counter()
        proc = launch()
        if wait_for(overlay_mapped, 15) is None:
            raise RuntimeError("overlay did not open")
        cold.append(time.perf_counter() - start)
        time.sleep(0.4)
        close_with_toggle()
        time.sleep(0.3)
        resident = find_resident_overlay(open_cmd)
        if resident:
            break
    if resident:
        # The first cold open is representative; the rest are warm re-opens.
        for _ in range(runs):
            start = time.perf_counter()
            launch().wait(timeout=10)
            if wait_for(overlay_mapped, 15) is None:
                raise RuntimeError("overlay did not re-open")
            warm.append(time.perf_counter() - start)
            time.sleep(0.4)
            close_with_toggle()
            time.sleep(0.3)
    result = {"cold_ms": [round(v * 1000, 1) for v in cold]}
    if resident:
        result["warm_ms"] = [round(v * 1000, 1) for v in warm]
        result["overlay_rss_kib"] = rss_kib(resident)
        os.kill(resident, 15)
        wait_for(lambda: not Path(f"/proc/{resident}").exists(), 10)
    return result


def find_resident_overlay(open_cmd: str) -> int | None:
    """PID of an overlay process that kept running after being closed."""
    binary = str(Path(open_cmd).resolve().parent / "cursor-clip")
    for proc in Path("/proc").iterdir():
        if not proc.name.isdigit():
            continue
        try:
            argv = (proc / "cmdline").read_bytes().split(b"\0")
        except OSError:
            continue
        if argv and argv[0].decode(errors="ignore") == binary and b"--daemon" not in argv:
            return int(proc.name)
    return None


def disk_usage_kib(path: Path) -> int:
    total = 0
    for entry in path.rglob("*"):
        try:
            if entry.is_file() and not entry.is_symlink():
                total += entry.stat().st_blocks * 512
        except OSError:
            pass
    return total // 1024


def run(
    binary: str,
    open_cmd: str,
    size: int,
    runs: int,
    images: dict[int, bytes],
    workdir: Path,
    skip_overlay: bool = False,
) -> dict:
    # Not /tmp: that is usually RAM-backed tmpfs, and v1.0.2's database can
    # grow to gigabytes while the history is being filled.
    workdir.mkdir(parents=True, exist_ok=True)
    home = Path(tempfile.mkdtemp(prefix="cursor-clip-bench-", dir=workdir))
    config = home / ".config" / "cursor-clip"
    config.mkdir(parents=True)
    (config / "config.toml").write_text(
        "show_trash = true\nshow_pin = true\npersistence_enabled = true\ninstant_paste = false\n"
    )
    env = dict(
        os.environ,
        HOME=str(home),
        XDG_CONFIG_HOME=str(home / ".config"),
        XDG_DATA_HOME=str(home / ".local" / "share"),
        XDG_CACHE_HOME=str(home / ".cache"),
        RUST_LOG="error",
    )
    daemon = Daemon(binary, env)
    try:
        daemon.start()
        copy_latencies, missed_copies = fill_history(size, images)
        disk_after_fill = disk_usage_kib(home)
        daemon.stop()

        # Restart so the history is loaded from disk, like after a login.
        daemon.start()
        time.sleep(3)
        assert history_len() == size, history_len()
        rss = rss_kib(daemon.proc.pid)
        sync = [ipc({"SetHistoryPersistence": {"enabled": True}})[0] for _ in range(5)]
        overlay = {} if skip_overlay else measure_overlay(open_cmd, env, runs)
        rss_after_open = rss_kib(daemon.proc.pid)
    finally:
        daemon.stop()
        shutil.rmtree(home, ignore_errors=True)

    tail = copy_latencies[-10:]
    return {
        "items": size,
        "daemon_rss_kib": rss,
        "daemon_rss_after_opens_kib": rss_after_open,
        "missed_copies": missed_copies,
        "disk_after_fill_kib": disk_after_fill,
        "copy_record_ms_median_last10": round(statistics.median(tail) * 1000, 1) if tail else None,
        "persistence_sync_ms_median": round(statistics.median(sync) * 1000, 2),
        **overlay,
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--label", required=True)
    parser.add_argument("--bin", required=True, help="cursor-clip binary (daemon and overlay)")
    parser.add_argument(
        "--open-cmd", help="command bound to the shortcut (default: --bin)"
    )
    parser.add_argument("--sizes", type=int, nargs="+", default=[0, 10, 100])
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--out", required=True)
    parser.add_argument(
        "--skip-overlay", action="store_true", help="only measure the daemon side"
    )
    parser.add_argument(
        "--workdir",
        type=Path,
        default=Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "cursor-clip-bench",
        help="where the isolated HOME directories are created (disk-backed)",
    )
    args = parser.parse_args()

    if SOCKET.exists():
        try:
            history_len()
            raise SystemExit("another cursor-clip daemon is running; stop it first")
        except OSError:
            pass

    images: dict[int, bytes] = {}
    results = []
    for size in args.sizes:
        print(f"[{args.label}] {size} items...", flush=True)
        result = run(
            args.bin,
            args.open_cmd or args.bin,
            size,
            args.runs,
            images,
            args.workdir,
            args.skip_overlay,
        )
        print(json.dumps(result), flush=True)
        results.append(result)
    Path(args.out).write_text(json.dumps({"label": args.label, "results": results}, indent=2))


if __name__ == "__main__":
    main()
