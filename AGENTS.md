# AGENTS.md

Operating manual for AI coding agents and human contributors working on
**`vicondoa/d2b-clip-picker`**. If you are installing the picker, start at
[README.md](./README.md) instead.

This project is GPL-3.0-only because it is forked from
[`Sirulex/cursor-clip`](https://github.com/Sirulex/cursor-clip). Keep that
license boundary intact.

## What this is

`d2b-clip-picker` is the UI-only GTK4/Libadwaita/Layer Shell picker client for
[d2b](https://github.com/vicondoa/d2b) clipboard flows. It is launched by
`d2b-clipd` with an inherited anonymous Unix socket and returns only `Select` or
`Cancel` for the current request.

It is not a clipboard manager, Wayland proxy, policy engine, history store, or
privileged clipboard owner.

## Trust boundary

The picker may:

- render destination/source metadata supplied by `d2b-clipd`;
- filter/search candidates;
- render safe text previews and allowed host thumbnails;
- handle keyboard, mouse, Escape, and window-close cancellation;
- send protocol `Select` or `Cancel` over the inherited socketpair.

The picker MUST NEVER:

- use `ext-data-control-v1`, `zwlr-data-control-manager-v1`, primary selection,
  `wl-copy`, `wl-paste`, virtual-keyboard injection, or `ydotool`;
- receive or write clipboard transfer file descriptors;
- connect to `$NIRI_SOCKET` or other compositor IPC for authority or labels;
- persist clipboard history or payloads;
- evaluate d2b policy or infer VM identity from app-id prefixes;
- import d2b internal Rust crates.

## Repo layout

```
.
├── README.md                 <- user-facing entry point
├── AGENTS.md                 <- this file
├── CHANGELOG.md              <- Keep a Changelog, entries under [Unreleased]
├── LICENSE                   <- GPL-3.0-only
├── flake.nix / flake.lock    <- source tarball, binary package, app, devShell
├── Cargo.toml / Cargo.lock   <- Rust package
├── rust-toolchain.toml       <- pinned Rust toolchain
├── src/
│   ├── protocol.rs           <- independent picker/clipd NDJSON DTOs
│   ├── ui.rs                 <- GTK/Libadwaita UI
│   ├── placement.rs          <- Layer Shell pointer/output placement
│   └── main.rs               <- IPC entry point + process harness
└── tests/                    <- protocol/policy/binary integration tests
```

## Build & validate

Prefer the Nix dev shell so GTK/Layer Shell dependencies match CI:

```bash
nix develop --command cargo fmt --all -- --check
nix develop --command cargo clippy --all-targets -- -D warnings
nix develop --command cargo test
nix flake check --no-build --all-systems
nix build .#d2b-clip-picker
nix build .#source
```

Zero compiler and clippy warnings are required.

## Development workflow

- Keep changes UI-only unless the protocol DTOs are explicitly being updated.
- Protocol changes must remain small, versioned, newline-delimited JSON, and
  independently implementable without d2b crates.
- Update `CHANGELOG.md` for every code or behavior change.
- Commit before `nix flake check`; untracked files are invisible to flake source
  capture.
- If docs and code disagree, committed passing code is canon; update docs in the
  same change.

## Panel review

For non-trivial changes, run the same unanimous panel-gate style used by d2b.
Prompts must include validation evidence and must tell reviewers not to rerun
long tests. Suggested roster:

| Reviewer | Focus |
| --- | --- |
| `rust` | ownership, lifetimes, protocol DTOs, errors, warnings |
| `wayland` | Layer Shell, pointer/output placement, no clipboard authority |
| `security` | UI-only boundary, no FDs/persistence/compositor authority |
| `test` | protocol/binary/policy coverage and CI sufficiency |
| `nix-packaging` | flake package/app/source tarball/dev shell |
| `product` | picker wording, navigation, cancellation, provenance clarity |
| `docs` | README, changelog, fork acknowledgement, Diataxis where applicable |

## Versioning and releases

Follow Semantic Versioning and Keep a Changelog. The crate version in
`Cargo.toml` is the package version used by the flake source tarball and binary
package. Release notes should be user-facing and should not contain internal
wave/finding/process markers.

## Don'ts

- Don't add clipboard ownership or data-control support.
- Don't add a d2b internal crate dependency.
- Don't receive, retain, or write transfer FDs.
- Don't connect to Niri IPC.
- Don't add shell-command construction; use argv if a command is ever needed.
- Don't commit generated `result*` symlinks, build artifacts, secrets, or
  payload samples from real clipboards.

## References

- [README.md](./README.md) — user-facing overview and install.
- [d2b](https://github.com/vicondoa/d2b) — clipboard authority (`d2b-clipd`)
  and Wayland proxy implementation.
- [Cursor Clip upstream](https://github.com/Sirulex/cursor-clip) — original
  GPL-3.0 project whose UI interaction model this fork adapts.
