{
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
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };

        craneLib = crane.mkLib pkgs;

        commonArgs = {
          src = craneLib.cleanCargoSource ./.;
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

        cursor-clip = craneLib.buildPackage (
          commonArgs
          // {
            inherit cargoArtifacts;
          }
        );
      in
      {
        packages.default = cursor-clip;

        devShells.default = craneLib.devShell {
          inputsFrom = [ cursor-clip ];

          packages = with pkgs; [
            rust-analyzer
            clippy
          ];
        };
      }
    );
}
