{
  description = "AZ — assistant personnel local-first";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, fenix }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        rustToolchain = fenix.packages.${system}.stable.withComponents [
          "cargo"
          "clippy"
          "rust-src"
          "rust-std"
          "rustc"
          "rustfmt"
        ];
      in
      {
        devShells.default = pkgs.mkShell {
          packages = [
            rustToolchain
            fenix.packages.${system}.rust-analyzer
            pkgs.pkg-config
            pkgs.cmake
            pkgs.clang
            pkgs.alsa-lib
            pkgs.stdenv.cc.cc.lib

            # Tauri 2 — système (WebKit + GTK + libsoup-3)
            pkgs.webkitgtk_4_1
            pkgs.gtk3
            pkgs.libsoup_3
            pkgs.librsvg
            pkgs.glib
            pkgs.cairo
            pkgs.pango
            pkgs.gdk-pixbuf
            pkgs.atk
            pkgs.harfbuzz
            pkgs.openssl
            pkgs.libayatana-appindicator
            pkgs.dbus
            pkgs.patchelf

            # Tauri 2 — frontend
            pkgs.nodejs_20
            pkgs.pnpm
          ];

          env = {
            RUST_BACKTRACE = "1";
            RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
            LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          };

          shellHook = ''
            export LD_LIBRARY_PATH=${pkgs.lib.makeLibraryPath [
              pkgs.stdenv.cc.cc.lib
              pkgs.alsa-lib
              pkgs.webkitgtk_4_1
              pkgs.gtk3
              pkgs.libsoup_3
              pkgs.glib
              pkgs.cairo
              pkgs.pango
              pkgs.gdk-pixbuf
              pkgs.atk
              pkgs.harfbuzz
              pkgs.librsvg
              pkgs.openssl
              pkgs.libayatana-appindicator
              pkgs.dbus
            ]}:''${LD_LIBRARY_PATH:-}

            # GTK : ressources (icônes, schémas) pour que la fenêtre WebKit s'affiche correctement
            export XDG_DATA_DIRS=${pkgs.gtk3}/share/gsettings-schemas/${pkgs.gtk3.name}:${pkgs.glib}/share/gsettings-schemas/${pkgs.glib.name}:''${XDG_DATA_DIRS:-}
          '';
        };

        formatter = pkgs.nixpkgs-fmt;
      });
}
