{
  description = "A Nix-flake-based Rust development environment";

  inputs = {
    nixpkgs.url = "https://flakehub.com/f/NixOS/nixpkgs/0.1"; # unstable Nixpkgs
    fenix = {
      url = "https://flakehub.com/f/nix-community/fenix/0.1";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { self, ... }@inputs:

    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forEachSupportedSystem =
        f:
        inputs.nixpkgs.lib.genAttrs supportedSystems (
          system:
          f {
            inherit system;
            pkgs = import inputs.nixpkgs {
              inherit system;
              overlays = [
                inputs.self.overlays.default
              ];
            };
          }
        );
    in
    {
      overlays.default = final: prev: {
        slsrt = final.callPackage ./nix/package.nix { };
      };

      packages = forEachSupportedSystem (
        { pkgs, ... }:
        {
          inherit (pkgs) slsrt;
          default = pkgs.slsrt;
        }
      );

      nixosModules.default = import ./nix/nixos.nix;
      homeManagerModules.default = import ./nix/home-manager.nix;

      apps = forEachSupportedSystem (
        { pkgs, ... }:
        {
          default = {
            type = "app";
            program = pkgs.lib.getExe pkgs.slsrt;
          };
        }
      );

      devShells = forEachSupportedSystem (
        { pkgs, system }:
        let
          rustToolchain =
            with inputs.fenix.packages.${system};
            combine (
              with stable;
              [
                clippy
                rustc
                cargo
                rustfmt
                rust-src
              ]
            );
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              rustToolchain
              openssl
              pkg-config
              cargo-deny
              cargo-edit
              cargo-watch
              rust-analyzer
              self.formatter.${system}
            ];

            env = {
              # Required by rust-analyzer
              RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
            };
          };
        }
      );

      formatter = forEachSupportedSystem ({ pkgs, ... }: pkgs.nixfmt);
    };
}
