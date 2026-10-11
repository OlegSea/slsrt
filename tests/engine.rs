use std::fs;
use std::path::{Path, PathBuf};

use slsrt::engine::Engine;
use slsrt::modes::run;
use tempfile::TempDir;

/// Temp workspace with an `in/` directory and a config whose `$IN`/`$ROOT` placeholders
/// are substituted with real paths.
struct Fixture {
    root: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("in")).unwrap();
        Fixture { root }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.path().canonicalize().unwrap().join(rel)
    }

    fn write(&self, rel: &str, contents: &str) -> PathBuf {
        let path = self.path(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path
    }

    fn config(&self, lua: &str) -> PathBuf {
        let root = self.path("");
        let lua = lua.replace("$ROOT", root.to_str().unwrap()).replace("$IN", self.path("in").to_str().unwrap());
        self.write("main.lua", &lua)
    }

    fn engine(&self, lua: &str, dry_run: bool) -> Engine {
        Engine::load(&self.config(lua), dry_run).unwrap()
    }

    fn load_err(&self, lua: &str) -> String {
        match Engine::load(&self.config(lua), false) {
            Ok(_) => panic!("config unexpectedly loaded"),
            Err(e) => format!("{e:#}"),
        }
    }
}

fn exists(p: impl AsRef<Path>) -> bool {
    p.as_ref().exists()
}

#[test]
fn moves_matching_files_only() {
    let fx = Fixture::new();
    fx.write("in/a.txt", "hello");
    fx.write("in/b.png", "png");
    let engine = fx.engine(
        r#"
        slsrt.rule {
          name = "text", paths = { "$IN" },
          filter = function(f) return f:has_ext("TXT") and f:size() == 5 end,
          action = function(f) f:move_to("$ROOT/out") end,
        }
        "#,
        false,
    );
    let summary = run::run(&engine).unwrap();
    assert_eq!((summary.scanned, summary.matched, summary.errors), (2, 1, 0));
    assert!(exists(fx.path("out/a.txt")));
    assert!(exists(fx.path("in/b.png")));
}

#[test]
fn dry_run_leaves_files_untouched() {
    let fx = Fixture::new();
    fx.write("in/a.txt", "x");
    let engine = fx.engine(
        r#"slsrt.rule { name = "r", paths = { "$IN" }, action = function(f) f:move_to("$ROOT/out") end }"#,
        true,
    );
    let summary = run::run(&engine).unwrap();
    assert_eq!(summary.matched, 1);
    assert!(exists(fx.path("in/a.txt")));
    assert!(!exists(fx.path("out")));
}

#[test]
fn priority_and_continue() {
    let fx = Fixture::new();
    let file = fx.write("in/a.txt", "x");
    let engine = fx.engine(
        r#"
        slsrt.rule { name = "low", paths = { "$IN" }, priority = 1, action = function(f) end }
        slsrt.rule { name = "high", paths = { "$IN" }, priority = 10, continue = true, action = function(f) end }
        slsrt.rule { name = "mid", paths = { "$IN" }, priority = 5, action = function(f) end }
        slsrt.rule { name = "mid2", paths = { "$IN" }, priority = 5, action = function(f) end }
        "#,
        false,
    );
    let names: Vec<_> = engine.rules().iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["high", "mid", "mid2", "low"]);
    assert_eq!(engine.process(&file).fired, ["high", "mid"]);
}

#[test]
fn chained_rename_and_move_with_conflicts() {
    let fx = Fixture::new();
    fx.write("in/a.txt", "new");
    fx.write("out/b.txt", "existing");
    let engine = fx.engine(
        r#"
        slsrt.rule {
          name = "r", paths = { "$IN" },
          action = function(f)
            f:rename("b.txt")
            assert(f.name == "b.txt")
            f:move_to("$ROOT/out")
            assert(f.name == "b (1).txt", f.name)
          end,
        }
        "#,
        false,
    );
    assert_eq!(run::run(&engine).unwrap().errors, 0);
    assert_eq!(fs::read_to_string(fx.path("out/b (1).txt")).unwrap(), "new");
    assert_eq!(fs::read_to_string(fx.path("out/b.txt")).unwrap(), "existing");
}

#[test]
fn skip_and_overwrite_policies() {
    let fx = Fixture::new();
    fx.write("in/a.txt", "new");
    fx.write("in/c.txt", "new");
    fx.write("out/a.txt", "old");
    fx.write("out/c.txt", "old");
    let engine = fx.engine(
        r#"
        slsrt.rule {
          name = "r", paths = { "$IN" },
          action = function(f)
            local policy = f.stem == "a" and "skip" or "overwrite"
            local result = f:move_to("$ROOT/out", { on_conflict = policy })
            if policy == "skip" then assert(result == nil) end
          end,
        }
        "#,
        false,
    );
    assert_eq!(run::run(&engine).unwrap().errors, 0);
    assert_eq!(fs::read_to_string(fx.path("in/a.txt")).unwrap(), "new");
    assert_eq!(fs::read_to_string(fx.path("out/a.txt")).unwrap(), "old");
    assert!(!exists(fx.path("in/c.txt")));
    assert_eq!(fs::read_to_string(fx.path("out/c.txt")).unwrap(), "new");
}

#[test]
fn erroring_rule_does_not_stop_others() {
    let fx = Fixture::new();
    fx.write("in/a.txt", "x");
    fx.write("in/b.txt", "x");
    let engine = fx.engine(
        r#"
        slsrt.rule { name = "bad", paths = { "$IN" }, priority = 1,
                     filter = function(f) error("boom") end, action = function(f) end }
        slsrt.rule { name = "good", paths = { "$IN" }, action = function(f) f:move_to("$ROOT/out") end }
        "#,
        false,
    );
    let summary = run::run(&engine).unwrap();
    assert_eq!((summary.matched, summary.errors), (2, 2));
    assert!(exists(fx.path("out/a.txt")) && exists(fx.path("out/b.txt")));
}

#[test]
fn recursion_is_per_rule_and_overlaps_are_deduped() {
    let fx = Fixture::new();
    fx.write("in/top.txt", "x");
    fx.write("in/sub/deep.txt", "x");
    let engine = fx.engine(
        r#"
        count = 0
        slsrt.rule { name = "flat", paths = { "$IN" }, continue = true,
                     filter = function(f) return f.name == "deep.txt" end, action = function(f) error("flat saw deep") end }
        slsrt.rule { name = "deep", paths = { "$IN", "$IN/sub" }, recursive = true,
                     action = function(f) count = count + 1 end }
        "#,
        false,
    );
    let summary = run::run(&engine).unwrap();
    assert_eq!((summary.scanned, summary.matched, summary.errors), (2, 2, 0));
}

#[test]
fn files_moved_during_run_are_not_reprocessed() {
    let fx = Fixture::new();
    fx.write("in/a.txt", "x");
    fs::create_dir(fx.path("in2")).unwrap();
    let engine = fx.engine(
        r#"
        slsrt.rule { name = "one", paths = { "$IN" }, action = function(f) f:move_to("$ROOT/in2") end }
        slsrt.rule { name = "two", paths = { "$ROOT/in2" }, action = function(f) f:move_to("$ROOT/out") end }
        "#,
        false,
    );
    let summary = run::run(&engine).unwrap();
    assert_eq!(summary.matched, 1);
    assert!(exists(fx.path("in2/a.txt")));
}

#[test]
fn trash_marks_file_removed() {
    let fx = Fixture::new();
    let file = fx.write("in/a.txt", "x");
    let engine = fx.engine(
        r#"
        slsrt.rule { name = "r", paths = { "$IN" }, action = function(f)
          f:delete()
          local ok = pcall(function() return f:size() end)
          assert(not ok, "size() after delete should fail")
        end }
        "#,
        false,
    );
    assert_eq!(engine.process(&file).errors, 0);
    assert!(!exists(&file));
}

#[test]
fn meta_helpers() {
    let fx = Fixture::new();
    let file = fx.write("in/.Report.Final.PDF", "12345");
    let engine = fx.engine(
        r#"
        slsrt.rule { name = "r", paths = { "$IN" }, action = function(f)
          assert(f.name == ".Report.Final.PDF")
          assert(f.stem == ".Report.Final")
          assert(f.ext == "pdf")
          assert(f.dir == "$IN")
          assert(f:size() == 5)
          assert(f:is_hidden())
          assert(not f:is_symlink())
          assert(f:age() < 60)
          assert(math.abs(f:modified() - os.time()) < 60)
          assert(f:has_ext(".pdf", "doc"))
          assert(not f:has_ext("txt"))
          assert(f:matches(".Report*"))
          assert(f:matches("**/in/*.PDF"))
          assert(not f:matches("*.txt"))
          assert(slsrt.minutes(2) == 120 and slsrt.KB == 1024)
          assert(slsrt.path.join("a", "b") == "a/b")
          assert(tostring(f):find("Report"))
        end }
        "#,
        false,
    );
    assert_eq!(engine.process(&file).errors, 0);
}

#[test]
fn require_loads_sibling_modules() {
    let fx = Fixture::new();
    fx.write("helpers.lua", "return { is_txt = function(f) return f.ext == 'txt' end }");
    let file = fx.write("in/a.txt", "x");
    let engine = fx.engine(
        r#"
        local h = require("helpers")
        slsrt.rule { name = "r", paths = { "in" }, filter = h.is_txt, action = function(f) end }
        "#,
        false,
    );
    assert_eq!(engine.rules()[0].paths, [fx.path("in")]);
    assert_eq!(engine.process(&file).fired, ["r"]);
}

#[test]
fn invalid_rules_are_rejected() {
    let fx = Fixture::new();
    let cases = [
        (r#"slsrt.rule { paths = { "$IN" }, action = function() end }"#, "missing required field `name`"),
        (r#"slsrt.rule { name = "r", action = function() end }"#, "missing required field `paths`"),
        (r#"slsrt.rule { name = "r", paths = { "$IN" } }"#, "missing required field `action`"),
        (r#"slsrt.rule { name = "r", paths = {}, action = function() end }"#, "at least one directory"),
        (r#"slsrt.rule { name = "r", paths = { "$IN" }, action = function() end, on_conflict = "skip" }"#, "unknown field `on_conflict`"),
        (r#"slsrt.rule { name = "r", paths = { "$IN" }, action = function() end, priority = "high" }"#, "`priority` must be of type integer"),
        (
            r#"slsrt.rule { name = "r", paths = { "$IN" }, action = function() end }
               slsrt.rule { name = "r", paths = { "$IN" }, action = function() end }"#,
            "already registered",
        ),
    ];
    for (lua, expected) in cases {
        let err = fx.load_err(lua);
        assert!(err.contains(expected), "expected `{expected}` in error, got: {err}");
    }
}

#[test]
fn invalid_action_option_is_an_error() {
    let fx = Fixture::new();
    let file = fx.write("in/a.txt", "x");
    let engine = fx.engine(
        r#"slsrt.rule { name = "r", paths = { "$IN" },
                        action = function(f) f:move_to("$ROOT/out", { on_conflict = "replace" }) end }"#,
        false,
    );
    assert_eq!(engine.process(&file).errors, 1);
    assert!(exists(&file));
}

/// Minimal AVI: one 1920×1080 XviD stream at 25 fps, 50 frames, titled "clip".
fn tiny_avi() -> Vec<u8> {
    fn chunk(id: &[u8], body: &[u8]) -> Vec<u8> {
        [id, &(body.len() as u32).to_le_bytes(), body].concat()
    }
    let le = |v: u32| v.to_le_bytes();
    let avih = [le(40_000), le(0), le(0), le(0), le(50), le(0), le(1), le(0), le(1920), le(1080)].concat();
    let strh = [&b"vids"[..], b"XVID", &[0; 12], &le(1), &le(25), &le(0), &le(50), &[0; 20]].concat();
    let strf = [le(40), le(1920), le(1080), le(0x0018_0001), *b"XVID"].concat();
    let strl = chunk(b"LIST", &[&b"strl"[..], &chunk(b"strh", &strh), &chunk(b"strf", &strf)].concat());
    let hdrl = chunk(b"LIST", &[&b"hdrl"[..], &chunk(b"avih", &avih), &strl].concat());
    let info = chunk(b"LIST", &[&b"INFO"[..], &chunk(b"INAM", b"clip")].concat());
    chunk(b"RIFF", &[&b"AVI "[..], &hdrl, &info].concat())
}

#[test]
fn video_helpers() {
    let fx = Fixture::new();
    fs::write(fx.path("in/movie.avi"), tiny_avi()).unwrap();
    fx.write("in/notes.mp4", "not really a video");
    let engine = fx.engine(
        r#"
        slsrt.rule {
          name = "hd", paths = { "$IN" },
          filter = function(f)
            local w, h = f:resolution()
            return w == 1920 and h == 1080 and f:duration() == 2 and f:framerate() == 25
              and f:video_codec() == "mpeg4" and f:audio_codec() == nil and f:tag("title") == "clip"
              and #f:tracks() == 1 and #f:tracks("video") == 1 and #f:subtitles() == 0
              and f:media().container == "avi" and f:media().tracks[1].codec_tag == "XVID"
          end,
          action = function(f) f:move_to("$ROOT/hd") end,
        }
        slsrt.rule {
          name = "not video", paths = { "$IN" },
          filter = function(f) return f:media() == nil and f:resolution() == nil and #f:tracks() == 0 end,
          action = function(f) f:move_to("$ROOT/other") end,
        }
        "#,
        false,
    );
    let summary = run::run(&engine).unwrap();
    assert_eq!((summary.matched, summary.errors), (2, 0));
    assert!(exists(fx.path("hd/movie.avi")));
    assert!(exists(fx.path("other/notes.mp4")));
}

#[test]
fn unknown_track_kind_is_an_error() {
    let fx = Fixture::new();
    let file = fx.write("in/a.txt", "x");
    let engine = fx.engine(
        r#"slsrt.rule { name = "r", paths = { "$IN" }, filter = function(f) return f:tracks("subs") end,
                        action = function() end }"#,
        false,
    );
    assert_eq!(engine.process(&file).errors, 1);
}
