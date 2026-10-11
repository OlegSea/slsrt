{ config, lib, pkgs, ... }:

let
  cfg = config.programs.slsrt;
in
{
  options.programs.slsrt = {
    enable = lib.mkEnableOption "slsrt, a file sorter driven by a Lua ruleset";

    package = lib.mkPackageOption pkgs "slsrt" { };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];
  };
}
