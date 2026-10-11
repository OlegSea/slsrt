# slsrt

Sort files into directories according to a Lua ruleset.

You write rules in Lua. Each rule names the directories it covers, a filter
function that decides whether a file matches, and an action function that
moves, renames, copies, trashes or deletes it. `slsrt run` applies the rules to
everything currently in those directories; `slsrt watch` stays resident and
applies them to files as they arrive.

```lua
slsrt.rule {
  name = "screenshots",
  paths = { "~/Pictures", "~/Desktop" },
  filter = function(f)
    return f:matches("Screenshot*") and f:has_ext("png", "jpg")
  end,
  action = function(f)
    f:move_to("~/Pictures/Screenshots/" .. os.date("%Y-%m", f:modified()))
  end,
}
```

Filters and actions are ordinary Lua functions, so anything Lua can express
(string patterns, date math, lookup tables, helper modules) is available inside
a rule.

> [!WARNING]
> This project has been entirely vibe-coded. Even though we did read the code to make sure nothing bad is there, we cannot assure you of its quality.
> Use at your own risk, and probably don't use it in an environment where safety matters.

> [!WARNING]
> This project allows you to run unsandboxed Lua code. Please don't run code you found on the internet without checking it first. Also be careful when using AI to write your configs. And just be careful, in general. Life's scary out there.

## Contents

- [Install](#install)
- [Quick start](#quick-start)
- [Command line](#command-line)
- [Writing rules](#writing-rules)
- [The `file` object](#the-file-object)
- [The `slsrt` table](#the-slsrt-table)
- [How rules are evaluated](#how-rules-are-evaluated)
- [Watch mode](#watch-mode)
- [Nix, NixOS and Home Manager](#nix-nixos-and-home-manager)
- [Development](#development)

## Install

With Cargo (Lua 5.4 is vendored, so no system Lua is needed):

```sh
cargo install --git https://github.com/OlegSea/slsrt
```

With Nix:

```sh
nix run github:OlegSea/slsrt -- --help
nix profile install github:OlegSea/slsrt
```

For declarative setups, see [Nix, NixOS and Home Manager](#nix-nixos-and-home-manager).

## Quick start

1. Copy [`examples/main.lua`](examples/main.lua) to `~/.config/slsrt/main.lua`
   and edit the rules.
2. Check that the config loads and the rules point where you expect:

   ```sh
   slsrt check
   ```

3. See what a run would do without touching anything:

   ```sh
   slsrt --dry-run run
   ```

4. Run it for real, or leave it watching:

   ```sh
   slsrt run
   slsrt watch
   ```

## Command line

```
slsrt [OPTIONS] <COMMAND>
```

| Command | What it does |
| ------- | ------------ |
| `run`   | Scan every directory the rules reference once and apply the rules to each file found. |
| `watch` | Watch those directories and process files as they are created or moved in. |
| `check` | Load and validate the config, then print the rules in evaluation order. |

| Option | Meaning |
| ------ | ------- |
| `-c, --config <MAIN_LUA>` | Use this config file instead of the default. |
| `-n, --dry-run` | Log what each action would do without changing any files. |
| `-v`, `-vv` | Debug, then trace logging. |
| `-q`, `-qq` | Warnings only, then errors only. |
| `--settle-ms <MS>` | `watch` only. How long a file must stay unchanged before it is processed. Default `2000`. |

`slsrt run` exits with a non-zero status if any filter or action failed, so it
can gate scripts and timers.

### Config location

The config file is looked up in this order:

1. `--config <path>`
2. `$SLSRT_CONFIG`
3. `main.lua` in the OS config directory: `~/.config/slsrt/main.lua` on Linux
   (respecting `$XDG_CONFIG_HOME`), `~/Library/Application Support/slsrt/main.lua`
   on macOS.

If the file is missing, the error message prints the path slsrt looked for.

### Logging

Logs go to stderr through `env_logger`, so `RUST_LOG` works on top of `-v` and
`-q`. Messages from `slsrt.log.*` in your config use the target `lua`, which
lets you turn them up on their own:

```sh
RUST_LOG=info,lua=debug slsrt run
```

## Writing rules

A config is a Lua script that calls `slsrt.rule { ... }` once per rule.

| Field         | Type     | Default | Meaning |
| ------------- | -------- | ------- | ------- |
| `name`        | string   | required | Unique name, used in logs and `slsrt check`. |
| `paths`       | list of strings | required | Directories the rule applies to. At least one. |
| `action`      | function | required | Called with the `file` object when the rule matches. |
| `filter`      | function | none    | Called with the `file` object. Any return value other than `nil` or `false` counts as a match. Without a filter, every file in `paths` matches. |
| `priority`    | integer  | `0`     | Higher priorities are evaluated first. |
| `recursive`   | boolean  | `false` | Also apply to files in subdirectories of `paths`. |
| `continue`    | boolean  | `false` | Keep evaluating lower-priority rules after this one fires. |
| `enabled`     | boolean  | `true`  | Set to `false` to keep a rule in the config but skip it. |
| `description` | string   | none    | Shown by `slsrt check`. |

Unknown fields, wrong types and duplicate names are errors at load time, so a
typo like `recursve = true` fails loudly instead of being ignored.

### Paths

- `~` at the start of a path and `$VAR` / `${VAR}` anywhere in it are expanded.
  Referencing an unset variable is an error.
- Relative entries in `paths` are resolved against the directory containing
  `main.lua`.
- Relative targets in actions (`f:move_to("sorted")`) are resolved against the
  file's current directory.
- Without `recursive`, a rule only sees files directly inside its directories.
  With it, files at any depth. Symlinked directories are not descended into.

### Splitting the config

The config directory is on Lua's `package.path`, so you can keep rules in
separate files and `require` them:

```lua
-- ~/.config/slsrt/main.lua
require("downloads")
require("media")       -- loads media.lua or media/init.lua
```

## The `file` object

Filters and actions receive one argument, the file being processed. Fields are
read with `.`, methods are called with `:`.

### Fields

| Field    | Example value |
| -------- | ------------- |
| `f.path` | `/home/me/Downloads/Report.PDF` |
| `f.name` | `Report.PDF` |
| `f.stem` | `Report` |
| `f.ext`  | `pdf` (lowercased, `""` if there is none) |
| `f.dir`  | `/home/me/Downloads` |

### Metadata

| Method | Returns |
| ------ | ------- |
| `f:size()` | Size in bytes. |
| `f:modified()`, `f:created()`, `f:accessed()` | Unix timestamp in seconds, or `nil` if the platform doesn't provide it. |
| `f:age()` | Seconds since the last modification. |
| `f:is_hidden()` | `true` for dotfiles (and, on Windows, files with the hidden attribute). |
| `f:is_symlink()` | Whether the path itself is a symlink. |
| `f:is_readonly()` | Whether the file is read-only. |

Metadata is read once per file and cached, so calling several of these in a
filter costs one `stat`.

### Matching

`f:has_ext("pdf", "djvu")` compares the extension case-insensitively. A
leading dot is optional.

`f:matches(glob)` tests a glob pattern. A pattern without `/` is matched
against the file name; a pattern containing `/` is matched against the full
path, and `*` does not cross directory separators.

```lua
f:matches("IMG_*.jpg")
f:matches("/home/*/Downloads/*.torrent")
```

### Actions

| Method | Effect |
| ------ | ------ |
| `f:move(path)` | Move to an exact path (new directory and name). |
| `f:move_to(dir)` | Move into `dir`, keeping the name. |
| `f:rename(name)` | Rename in place. The name may not contain a path separator. |
| `f:copy_to(dir)` | Copy into `dir`, keeping the name. The `file` object stays on the original. |
| `f:trash()` | Move to the system trash. |
| `f:delete()` | Delete permanently. |

Missing target directories are created. Moves across filesystems fall back to
copy and remove.

`move`, `move_to`, `rename` and `copy_to` take an optional second argument
that controls what happens when the target already exists:

```lua
f:move_to("~/Archives", { on_conflict = "skip" })
```

| `on_conflict` | Behaviour |
| ------------- | --------- |
| `"rename"` (default) | Pick a free name: `report (1).pdf`, `report (2).pdf`, ... |
| `"skip"` | Leave the file where it is. The method returns `nil`. |
| `"overwrite"` | Replace the existing file. |

These four methods return the path they wrote to, or `nil` when skipped.

After a move or rename, the `file` object points at the new location, so
actions can be chained:

```lua
action = function(f)
  f:rename(os.date("%Y-%m-%d ", f:modified()) .. f.name)
  f:move_to("~/Documents/Scans")
end
```

After `trash()` or `delete()`, any further use of the object that needs the
file on disk raises an error.

## The `slsrt` table

| Name | Meaning |
| ---- | ------- |
| `slsrt.rule { ... }` | Register a rule. |
| `slsrt.KB`, `MB`, `GB`, `TB` | Byte counts (powers of 1024), for `f:size() > 500 * slsrt.MB`. |
| `slsrt.minutes(n)`, `hours(n)`, `days(n)` | Seconds, for `f:age() > slsrt.days(30)`. |
| `slsrt.expand(path)` | Expand `~` and environment variables in a string. |
| `slsrt.path.join(a, b, ...)` | Join path components. |
| `slsrt.path.exists(path)` | Whether anything exists at `path` (after expansion). |
| `slsrt.log.debug/info/warn/error(msg)` | Write to slsrt's log. |
| `slsrt.dry_run` | `true` when running with `--dry-run`. |

## How rules are evaluated

Rules are sorted by `priority`, highest first. Rules with equal priority keep
the order they were registered in. `slsrt check` prints this final order.

For each file, slsrt walks that list and considers every enabled rule whose
`paths` cover the file:

1. If the rule has a filter and it returns `nil` or `false`, move on to the next rule.
2. Otherwise run the action.
3. Stop, unless the rule has `continue = true`.

So by default the first matching rule wins. With `continue`, a high-priority
rule can do something to every file (log it, rename it) and let a later rule
decide where it goes. Each later rule is checked against the file's current
path, so a rule that moved the file out of a directory no longer triggers the
rules for that directory.

Errors are contained per file. A filter that raises an error is logged and that
rule is skipped. An action that raises an error is logged and processing of
that file stops, since its state is no longer known. Other files are
unaffected.

`slsrt run` lists all files before acting, so a file moved into another rule's
directory during a run is not processed a second time in the same run.

### Dry runs

With `--dry-run`, actions log `[dry-run] would move ...` and leave the
filesystem alone. The `file` object still tracks the pretend location after a
move, so a `continue` rule that reads metadata after a pretend move will fail,
because nothing exists at that path yet. Check `slsrt.dry_run` in the config
if a rule needs to behave differently.

## Watch mode

`slsrt watch` subscribes to every directory referenced by an enabled rule
(recursively where any rule on that directory is recursive). It reacts to files
being created or moved into those directories, and waits until a file has been
quiet for `--settle-ms` before processing it, so partially downloaded files are
left alone.

A few things to know:

- Files that are already present when `watch` starts are not processed. Run
  `slsrt run` once first, or use both.
- In-place edits of an existing file do not trigger the rules. New files and
  renames do.
- Files written by slsrt's own actions are ignored for a short window, so a
  rule that moves files into another watched directory doesn't trigger a
  second pass on them.
- Directories that don't exist at startup are skipped with a warning. If none
  exist, `watch` exits with an error.
- The config is read once at startup. Restart `watch` after editing it.

## Nix, NixOS and Home Manager

The flake provides:

| Output | Contents |
| ------ | -------- |
| `packages.<system>.default` | The `slsrt` binary, plus the example config at `share/slsrt/examples/main.lua`. |
| `overlays.default` | Adds `pkgs.slsrt`. |
| `homeManagerModules.default` | `services.slsrt`: per-user ruleset and systemd user service. |
| `nixosModules.default` | `programs.slsrt`: installs the package system-wide. |
| `devShells.<system>.default` | Development shell (see below). |

Supported systems: `x86_64-linux`, `aarch64-linux`, `aarch64-darwin`.

Both modules take their default package from `pkgs.slsrt`, which is not in
nixpkgs. Apply the overlay, or set `package` explicitly:

```nix
{
  inputs.slsrt.url = "github:OlegSea/slsrt";

  # in your NixOS or Home Manager configuration:
  nixpkgs.overlays = [ inputs.slsrt.overlays.default ];
}
```

### Home Manager

```nix
imports = [ inputs.slsrt.homeManagerModules.default ];

services.slsrt = {
  enable = true;
  mode = "watch";        # or "timer"
  settleMs = 2000;       # watch mode only
  interval = "*:0/15";   # timer mode only, systemd OnCalendar syntax
  settings = ''
    slsrt.rule {
      name = "screenshots",
      paths = { "$HOME/Pictures", "$HOME/Desktop" },
      filter = function(f) return f:matches("Screenshot*") end,
      action = function(f) f:move_to("$HOME/Pictures/Screenshots") end,
    }
  '';
};
```

| Option | Default | Meaning |
| ------ | ------- | ------- |
| `enable` | `false` | Turn the service on. |
| `package` | `pkgs.slsrt` | Package to run. |
| `settings` | `""` | Lua source written to `~/.config/slsrt/main.lua`. Must not be empty when enabled. |
| `mode` | `"watch"` | `"watch"` runs `slsrt watch` as a resident service. `"timer"` runs `slsrt run` on a timer. |
| `settleMs` | `2000` | Passed to `--settle-ms` in watch mode. |
| `interval` | `"*:0/15"` | `OnCalendar` expression for timer mode (every 15 minutes by default). |

In watch mode the module creates the user service `slsrt-watch`, tied to
`graphical-session.target` and restarted on failure. In timer mode it creates
`slsrt-run.service` and `slsrt-run.timer`; the timer is persistent, so if a run
was missed while the machine was off, systemd starts it as soon as the timer
is active again.

Remember that `settings` is a Nix string: `${...}` inside it is Nix
interpolation. Use `$VAR` in rule paths, or escape it as `''${VAR}`.

### NixOS

```nix
imports = [ inputs.slsrt.nixosModules.default ];
programs.slsrt.enable = true;
```

This only installs the binary. Rules stay per-user in each user's config
directory.

## Development

```sh
nix develop          # or `direnv allow`; the repo has an .envrc
cargo test
cargo build --release
nix fmt              # formats the Nix files with nixfmt
```

The dev shell provides a stable Rust toolchain from fenix (with clippy,
rustfmt and rust-src), rust-analyzer, cargo-watch, cargo-edit and cargo-deny.

Integration tests in [`tests/engine.rs`](tests/engine.rs) build configs in a
temporary directory and run the engine against real files.

### Layout

| Path | Contents |
| ---- | -------- |
| `src/main.rs`, `src/cli.rs` | Argument parsing and command dispatch. |
| `src/config.rs` | Config file lookup. |
| `src/lua/` | Lua state setup, the `slsrt` table and the `file` userdata. |
| `src/rules.rs` | Rule parsing, validation and ordering. |
| `src/engine.rs` | Runs the rules against one file. |
| `src/scan.rs` | Collects the directories and files to process. |
| `src/modes/` | `run` and `watch`. |
| `src/modules/` | Everything exposed on the `file` object. |
| `src/actions/` | Conflict policies and the real and dry-run executors. |

### Adding file capabilities

The `file` object has no built-in fields or methods. Everything on it is
registered by a module in `src/modules/`: `meta.rs` provides names, metadata
and matching, `actions.rs` provides the file operations. To add something like
EXIF or media info:

1. Create a type implementing `Module` and register fields and methods through
   `ModuleCtx::file_field` / `file_method` (or `api_fn` for functions on the
   `slsrt` table).
2. Use `FileHandle::cached` for anything expensive, so it is computed only when
   a filter asks for it and at most once per file.
3. Add the module to `modules::all()`.

Name clashes between modules are caught when the Lua state is built.
