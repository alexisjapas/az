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
          ];

          env = {
            RUST_BACKTRACE = "1";
            RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
          };
        };

        formatter = pkgs.nixpkgs-fmt;
      });
}
