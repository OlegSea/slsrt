{ lib, rustPlatform }:

let
  cargoToml = builtins.fromTOML (builtins.readFile ../Cargo.toml);
in
rustPlatform.buildRustPackage {
  pname = cargoToml.package.name;
  version = cargoToml.package.version;

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../src
      ../tests
    ];
  };
  cargoLock.lockFile = ../Cargo.lock;

  postInstall = ''
    install -Dm644 ${../examples/main.lua} $out/share/slsrt/examples/main.lua
  '';

  # Fail the build if the binary embeds toolchain paths, which would pull
  # rustc and cargo into the runtime closure.
  disallowedReferences = [
    rustPlatform.rust.rustc
    rustPlatform.rust.cargo
  ];

  meta = {
    description = "Sort files into directories according to a Lua ruleset";
    mainProgram = "slsrt";
    platforms = [
      "x86_64-linux"
      "aarch64-linux"
      "aarch64-darwin"
    ];
  };
}
