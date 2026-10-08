-- Example slsrt config. Copy to ~/.config/slsrt/main.lua (Linux) or pass with --config.
--
-- Rules live in tables registered with slsrt.rule. Relative `paths` are resolved against
-- this file's directory; relative action targets against the file's current directory.

slsrt.rule {
  name = "screenshots",
  description = "File screenshots into monthly folders",
  priority = 50,
  paths = { "~/Pictures", "~/Desktop" },
  filter = function(f)
    return f:matches("Screenshot*") and f:has_ext("png", "jpg")
  end,
  action = function(f)
    f:move_to("~/Pictures/Screenshots/" .. os.date("%Y-%m", f:modified()))
  end,
}

slsrt.rule {
  name = "large-archives",
  paths = { "~/Downloads" },
  recursive = true,
  filter = function(f)
    return f:has_ext("zip", "tar", "gz", "7z") and f:size() > 500 * slsrt.MB
  end,
  action = function(f)
    f:move_to("~/Archives", { on_conflict = "skip" })
  end,
}

slsrt.rule {
  name = "old-installers",
  description = "Trash installers not touched for a month",
  paths = { "~/Downloads" },
  filter = function(f)
    return f:has_ext("deb", "rpm", "AppImage", "exe", "dmg") and f:age() > slsrt.days(30)
  end,
  action = function(f)
    slsrt.log.info("trashing old installer " .. f.name)
    f:trash()
  end,
}
