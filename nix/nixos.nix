{ config, lib, pkgs, ... }:

let
  cfg = config.programs.slsrt;
in
{
  options.programs.slsrt = {
    enable = lib.mkEnableOption "slsrt, a file sorter driven by a Lua ruleset";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.slsrt or (pkgs.callPackage ../nix/package.nix { });
      defaultText = lib.literalExpression "pkgs.slsrt or (pkgs.callPackage <slsrt>/nix/package.nix { })";
      description = "The slsrt package to use. Built from your nixpkgs unless the overlay provides `pkgs.slsrt`.";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];
  };
}
