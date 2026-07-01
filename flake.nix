{
  description = "d2b-clip-picker — UI-only picker client for d2b clipboard flows";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      nixpkgs,
      crane,
      flake-utils,
      ...
    }:
    flake-utils.lib.eachSystem
      [
        "x86_64-linux"
        "aarch64-linux"
      ]
      (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          craneLib = crane.mkLib pkgs;
          manifest = builtins.fromTOML (builtins.readFile ./Cargo.toml);
          version = manifest.package.version;
          cleanSrc = craneLib.cleanCargoSource ./.;
          commonArgs = {
            pname = "d2b-clip-picker";
            inherit version;
            src = cleanSrc;
            strictDeps = true;
            nativeBuildInputs = with pkgs; [ pkg-config ];
            buildInputs = with pkgs; [
              gtk4
              dbus
              glib
              gtk4-layer-shell
              libadwaita
            ];
          };
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;
          d2b-clip-picker = craneLib.buildPackage (
            commonArgs
            // {
              inherit cargoArtifacts;
            }
          );
          sourceTarball = pkgs.runCommand "d2b-clip-picker-${version}-source" { } ''
            mkdir -p "$out"
            tar \
              --sort=name \
              --mtime='@0' \
              --owner=0 \
              --group=0 \
              --numeric-owner \
              --transform 'flags=r;s,^\.$,d2b-clip-picker-${version},' \
              --transform 'flags=r;s,^\./,d2b-clip-picker-${version}/,' \
              -czf "$out/d2b-clip-picker-${version}-source.tar.gz" \
              -C ${cleanSrc} .
          '';
        in
        {
          packages.default = d2b-clip-picker;
          packages.d2b-clip-picker = d2b-clip-picker;
          packages.binary = d2b-clip-picker;
          packages.source = sourceTarball;
          apps.default = flake-utils.lib.mkApp { drv = d2b-clip-picker; };
          devShells.default = craneLib.devShell {
            inputsFrom = [ d2b-clip-picker ];
            packages = with pkgs; [
              cargo
              rustc
              rustfmt
              rust-analyzer
              clippy
            ];
          };
          checks = {
            inherit d2b-clip-picker;
          };
          formatter = pkgs.nixfmt-rfc-style;
        }
      );
}
