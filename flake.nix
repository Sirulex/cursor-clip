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

          nativeBuildInputs = with pkgs; [
            pkg-config
            wrapGAppsHook4
          ];
          buildInputs = with pkgs; [
            gtk4
            dbus
            glib
            gtk4-layer-shell
            libadwaita
            adwaita-icon-theme
          ];
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        cursor-clip = craneLib.buildPackage (
          commonArgs
          // {
            inherit cargoArtifacts;
            meta.mainProgram = "cursor-clip-toggle";
            postInstall = ''
              install -Dm644 ${./assets/io.github.sirulex.cursor-clip.desktop} \
                "$out/share/applications/io.github.sirulex.cursor-clip.desktop"
              install -Dm644 ${./assets/io.github.sirulex.cursor-clip.svg} \
                "$out/share/icons/hicolor/scalable/apps/io.github.sirulex.cursor-clip.svg"
              substituteInPlace "$out/share/applications/io.github.sirulex.cursor-clip.desktop" \
                --replace-fail "Exec=cursor-clip-toggle" "Exec=$out/bin/cursor-clip-toggle"
            '';
          }
        );
      in
      {
        packages.default = cursor-clip;

        devShells.default = craneLib.devShell {
          inputsFrom = [ cursor-clip ];

          # Crane supplies Cargo, rustc, Clippy, and rustfmt.
          packages = with pkgs; [
            rust-analyzer
            adwaita-icon-theme
          ];

          RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;

          # Make GTK schemas and icons discoverable when running through Cargo.
          shellHook = ''
            export XDG_DATA_DIRS="${pkgs.adwaita-icon-theme}/share''${GSETTINGS_SCHEMAS_PATH:+:$GSETTINGS_SCHEMAS_PATH}''${XDG_DATA_DIRS:+:$XDG_DATA_DIRS}"
          '';
        };
      }
    );
}
