{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.slsrt;

  settleMs = toString cfg.settleMs;

  settingsType = lib.types.addCheck (lib.types.str // {
    description = "Lua source for the slsrt ruleset (body of main.lua)";
  }) (_: true);

  watchService = execStart: {
    Unit = {
      Description = "slsrt: sort files according to the Lua ruleset";
      Documentation = "man:slsrt(1)";
      After = [ "graphical-session.target" ];
      PartOf = [ "graphical-session.target" ];
    };

    Service = {
      ExecStart = execStart;
      Restart = "on-failure";
      RestartSec = "5s";
      Environment = [ "SLSRT_CONFIG=%h/.config/slsrt/main.lua" ];
    };

    Install = {
      WantedBy = [ "graphical-session.target" ];
    };
  };

  timerUnit = {
    Unit = {
      Description = "Periodic slsrt run (sort files according to the Lua ruleset)";
    };

    Timer = {
      OnCalendar = cfg.interval;
      Persistent = true;
      Unit = "slsrt-run.service";
    };

    Install = {
      WantedBy = [ "timers.target" ];
    };
  };

  runService = {
    Unit = {
      Description = "One-shot slsrt run (sort files according to the Lua ruleset)";
    };

    Service = {
      Type = "oneshot";
      ExecStart = "${lib.getExe cfg.package} run";
      Environment = [ "SLSRT_CONFIG=%h/.config/slsrt/main.lua" ];
    };
  };
in
{
  options.services.slsrt = {
    enable = lib.mkEnableOption "slsrt, a file sorter driven by a Lua ruleset";

    package = lib.mkPackageOption pkgs "slsrt" { };

    settings = lib.mkOption {
      type = settingsType;
      default = "";
      example = ''
        slsrt.rule {
          name = "screenshots",
          paths = { "$HOME/Pictures", "$HOME/Desktop" },
          filter = function(f) return f:matches("Screenshot*") end,
          action = function(f) f:move_to("$HOME/Pictures/Screenshots") end,
        }
      '';
      description = ''
        Lua source of the ruleset, written to
        {file}`$XDG_CONFIG_HOME/slsrt/main.lua`. Use `slsrt.rule { ... }`
        to register rules; `~` and `$VAR`/`''${VAR}` are expanded in rule paths
        and action targets.

        See the shipped example at
        {file}`$out/share/slsrt/examples/main.lua` in the package.
      '';
    };

    mode = lib.mkOption {
      type = lib.types.enum [
        "watch"
        "timer"
      ];
      default = "watch";
      description = ''
        `watch` runs a resident systemd service that reacts to filesystem
        events (after a settle period). `timer` runs `slsrt run` periodically
        on a systemd timer.
      '';
    };

    settleMs = lib.mkOption {
      type = lib.types.ints.positive;
      default = 2000;
      example = 5000;
      description = ''
        How long a file must stay unchanged before it is processed, in
        milliseconds. Only used when `mode` is `watch`.
      '';
    };

    interval = lib.mkOption {
      type = lib.types.str;
      default = "*:0/15";
      example = "hourly";
      description = ''
        OnCalendar expression for periodic runs. Only used when `mode` is
        `timer`.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.settings != "";
        message = "services.slsrt.settings must contain a Lua ruleset (`slsrt.rule { ... }`)";
      }
    ];

    xdg.configFile."slsrt/main.lua".text = cfg.settings;

    systemd.user.services.slsrt-watch = lib.mkIf (cfg.mode == "watch") (
      watchService "${lib.getExe cfg.package} watch --settle-ms ${settleMs}"
    );

    systemd.user.services.slsrt-run = lib.mkIf (cfg.mode == "timer") runService;
    systemd.user.timers.slsrt-run = lib.mkIf (cfg.mode == "timer") timerUnit;
  };
}
