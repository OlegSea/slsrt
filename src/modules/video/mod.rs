//! Video container inspection: duration, resolution, framerate, codecs, tags and tracks.
//!
//! Parsed from container headers only (MP4/MOV/M4V/3GP, MKV/WebM, AVI); no frames are decoded
//! and no external tools are run. Files in other formats report `nil` instead of raising.

mod codec;
mod isobmff;
mod matroska;
mod model;
mod reader;
mod riff;

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::rc::Rc;

use mlua::prelude::*;

use super::{Module, ModuleCtx};
use crate::lua::file::FileHandle;
pub use model::{Kind, MediaInfo, Track};

pub struct Video;

/// Cached probe result; `None` when the file isn't a recognised container.
struct Probe(Option<MediaInfo>);

pub fn probe(path: &Path) -> io::Result<Option<MediaInfo>> {
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut r = BufReader::new(file);
    let mut magic = Vec::with_capacity(12);
    r.by_ref().take(12).read_to_end(&mut magic)?;
    r.seek(SeekFrom::Start(0))?;

    let info = if isobmff::sniff(&magic) {
        isobmff::parse(&mut r, len)
    } else if matroska::sniff(&magic) {
        matroska::parse(&mut r, len)
    } else if riff::sniff(&magic) {
        riff::parse(&mut r, len)
    } else {
        None
    };
    Ok(info.map(|mut info| {
        if let Some(secs) = info.duration.filter(|&d| d > 0.0) {
            info.bitrate = Some((len as f64 * 8.0 / secs) as u64);
        }
        info
    }))
}

fn media(file: &FileHandle) -> LuaResult<Rc<Probe>> {
    file.cached(|path| {
        probe(path)
            .map(Probe)
            .map_err(|e| LuaError::runtime(format!("cannot read {}: {e}", path.display())))
    })
}

fn first(file: &FileHandle, kind: Kind) -> LuaResult<Option<Track>> {
    Ok(media(file)?.0.as_ref().and_then(|m| m.first(kind)).cloned())
}

fn parse_kind(kind: &str) -> LuaResult<Kind> {
    match kind {
        "video" => Ok(Kind::Video),
        "audio" => Ok(Kind::Audio),
        "subtitle" => Ok(Kind::Subtitle),
        "other" => Ok(Kind::Other),
        _ => Err(LuaError::runtime(format!("unknown track kind `{kind}`, expected one of {}", Kind::NAMES.join(", ")))),
    }
}

impl Module for Video {
    fn name(&self) -> &'static str {
        "video"
    }

    fn register(&self, ctx: &ModuleCtx) -> LuaResult<()> {
        ctx.file_method("media", |lua, f, ()| media(f)?.0.as_ref().map(|m| m.to_lua(lua)).transpose())?;
        ctx.file_method("duration", |_, f, ()| Ok(media(f)?.0.as_ref().and_then(|m| m.duration)))?;
        ctx.file_method("resolution", |_, f, ()| {
            let size = first(f, Kind::Video)?.and_then(|t| t.resolution());
            Ok((size.map(|s| s.0), size.map(|s| s.1)))
        })?;
        ctx.file_method("framerate", |_, f, ()| Ok(first(f, Kind::Video)?.and_then(|t| t.fps)))?;
        ctx.file_method("video_codec", |_, f, ()| Ok(first(f, Kind::Video)?.map(|t| t.codec)))?;
        ctx.file_method("audio_codec", |_, f, ()| Ok(first(f, Kind::Audio)?.map(|t| t.codec)))?;
        ctx.file_method("tracks", |lua, f, kind: Option<String>| {
            let kind = kind.as_deref().map(parse_kind).transpose()?;
            match &media(f)?.0 {
                Some(m) => m.tracks_lua(lua, kind),
                None => lua.create_table(),
            }
        })?;
        ctx.file_method("subtitles", |lua, f, ()| match &media(f)?.0 {
            Some(m) => m.tracks_lua(lua, Some(Kind::Subtitle)),
            None => lua.create_table(),
        })?;
        ctx.file_method("tag", |_, f, name: String| {
            Ok(media(f)?.0.as_ref().and_then(|m| m.tags.get(&model::tag_key(&name)).cloned()))
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probes_by_content_not_extension() {
        let dir = tempfile::tempdir().unwrap();
        let mp4 = dir.path().join("clip.bin");
        std::fs::write(&mp4, isobmff::tests::sample_mp4()).unwrap();
        let avi = dir.path().join("clip.mp4");
        std::fs::write(&avi, riff::tests::sample_avi()).unwrap();
        let text = dir.path().join("notes.mkv");
        std::fs::write(&text, "not a video").unwrap();

        let info = probe(&mp4).unwrap().unwrap();
        assert_eq!(info.container, "mp4");
        assert!(info.bitrate.is_some());
        assert_eq!(probe(&avi).unwrap().unwrap().container, "avi");
        assert!(probe(&text).unwrap().is_none());
    }
}
