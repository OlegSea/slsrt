//! AVI (RIFF). Stream headers live in `LIST hdrl`, tags in `LIST INFO`; `LIST movi` is skipped.

use std::io::{Read, Seek, SeekFrom};

use super::codec;
use super::model::{Kind, MediaInfo, Track, round_fps};
use super::reader::{Bytes, fourcc_str, read_at, text};

const LIST_CAP: u64 = 4 << 20;

pub fn sniff(magic: &[u8]) -> bool {
    magic.len() >= 12 && &magic[..4] == b"RIFF" && &magic[8..12] == b"AVI "
}

/// Chunks inside an in-memory LIST body. Chunk bodies are padded to an even length.
fn chunks(data: &[u8]) -> impl Iterator<Item = ([u8; 4], &[u8])> {
    let mut b = Bytes::new(data);
    std::iter::from_fn(move || {
        let id = b.fourcc()?;
        let size = b.u32le()? as usize;
        let body = b.take(size.min(b.remaining()))?;
        if size % 2 == 1 {
            b.skip(1);
        }
        Some((id, body))
    })
}

/// `LIST` bodies start with their list type.
fn list<'a>(id: &[u8; 4], body: &'a [u8], want: &[u8; 4]) -> Option<&'a [u8]> {
    (id == b"LIST" && body.get(..4) == Some(&want[..])).then(|| &body[4..])
}

pub fn parse<R: Read + Seek>(r: &mut R, len: u64) -> Option<MediaInfo> {
    let mut head = [0u8; 12];
    r.seek(SeekFrom::Start(0)).ok()?;
    r.read_exact(&mut head).ok()?;
    let riff_end = (8 + u32::from_le_bytes(head[4..8].try_into().ok()?) as u64).min(len);

    let mut info = MediaInfo::new("avi");
    let mut header = None;
    let mut pos = 12u64;
    while pos + 12 <= riff_end {
        r.seek(SeekFrom::Start(pos)).ok()?;
        let mut chunk = [0u8; 12];
        r.read_exact(&mut chunk).ok()?;
        let size = u32::from_le_bytes(chunk[4..8].try_into().ok()?) as u64;
        if &chunk[..4] == b"LIST"
            && matches!(&chunk[8..12], b"hdrl" | b"INFO")
            && let Some(body) = read_at(r, pos + 12, size.saturating_sub(4), LIST_CAP)
        {
            if &chunk[8..12] == b"hdrl" {
                header = hdrl(&mut info, &body);
            } else {
                tags(&mut info, &body);
            }
        }
        pos += 8 + size + size % 2;
    }

    let main = header?;
    info.duration = info.tracks.iter().filter_map(|t| t.duration).reduce(f64::max).or_else(|| {
        let secs = main.total_frames as f64 * main.usec_per_frame as f64 / 1e6;
        (secs > 0.0).then_some(secs)
    });
    Some(info)
}

/// The fields of `avih` used as a fallback when the stream headers lack lengths.
struct MainHeader {
    usec_per_frame: u32,
    total_frames: u32,
}

fn hdrl(info: &mut MediaInfo, data: &[u8]) -> Option<MainHeader> {
    let mut main = None;
    let mut odml_frames = None;
    for (id, body) in chunks(data) {
        if &id == b"avih" {
            let mut b = Bytes::new(body);
            let usec_per_frame = b.u32le()?;
            b.skip(12)?;
            let total_frames = b.u32le()?;
            main = Some(MainHeader { usec_per_frame, total_frames });
        } else if let Some(strl) = list(&id, body, b"strl") {
            if let Some(mut track) = stream(strl) {
                track.id = info.tracks.len() as u64;
                info.tracks.push(track);
            }
        } else if let Some(odml) = list(&id, body, b"odml") {
            // OpenDML (files over 1 GB): `avih` only counts frames in the first RIFF chunk.
            odml_frames = chunks(odml).find(|(id, _)| id == b"dmlh").and_then(|(_, b)| Bytes::new(b).u32le());
        }
    }
    let mut main = main?;
    if let Some(frames) = odml_frames.filter(|&f| f > 0) {
        main.total_frames = frames;
    }
    for track in info.tracks.iter_mut().filter(|t| t.kind == Kind::Video) {
        track.frame_count = track.frame_count.max(Some(main.total_frames as u64).filter(|&f| f > 0));
    }
    let mut seen = std::collections::HashSet::new();
    for track in &mut info.tracks {
        track.default = seen.insert(track.kind);
    }
    Some(main)
}

fn stream(data: &[u8]) -> Option<Track> {
    let (_, strh) = chunks(data).find(|(id, _)| id == b"strh")?;
    let mut b = Bytes::new(strh);
    let kind = match &b.fourcc()? {
        b"vids" => Kind::Video,
        b"auds" => Kind::Audio,
        b"txts" => Kind::Subtitle,
        _ => Kind::Other,
    };
    let handler = b.fourcc()?;
    b.skip(4 + 2 + 2 + 4)?;
    let scale = b.u32le()?;
    let rate = b.u32le()?;
    b.skip(4)?;
    let length = b.u32le()?;

    let mut track = Track::new(kind);
    if scale > 0 && rate > 0 {
        let secs = length as f64 * scale as f64 / rate as f64;
        track.duration = (secs > 0.0).then_some(secs);
        if kind == Kind::Video {
            track.fps = round_fps(rate as f64 / scale as f64);
            track.frame_count = (length > 0).then_some(length as u64);
        }
    }
    track.name = chunks(data).find(|(id, _)| id == b"strn").and_then(|(_, n)| text(n));

    let strf = chunks(data).find(|(id, _)| id == b"strf").map(|(_, f)| f).unwrap_or_default();
    let mut f = Bytes::new(strf);
    match kind {
        Kind::Video => {
            f.skip(4);
            let (w, h) = (f.i32le(), f.i32le());
            f.skip(4);
            let compression = f.fourcc();
            track.width = w.map(|w| w.unsigned_abs()).filter(|&w| w > 0);
            // Negative height marks a top-down bitmap.
            track.height = h.map(|h| h.unsigned_abs()).filter(|&h| h > 0);
            let tag = compression.filter(|c| c != &[0; 4]).unwrap_or(handler);
            track.set_codec(fourcc_str(&tag), codec::vfw_codec(&tag));
        }
        Kind::Audio => {
            let format = f.u16le().unwrap_or_default();
            track.channels = f.u16le().map(u32::from).filter(|&c| c > 0);
            track.sample_rate = f.u32le().filter(|&r| r > 0);
            let bytes_per_sec = f.u32le().unwrap_or_default();
            f.skip(2);
            let bits = f.u16le().unwrap_or_default();
            track.set_codec(format!("0x{format:04x}"), codec::wave_codec(format));
            track.bitrate = (bytes_per_sec > 0).then_some(bytes_per_sec as u64 * 8);
            if track.codec == "pcm" && bits > 0 {
                track.bit_depth = Some(bits as u32);
            }
        }
        _ => track.set_codec(fourcc_str(&handler), None),
    }
    Some(track)
}

fn tags(info: &mut MediaInfo, data: &[u8]) {
    for (id, body) in chunks(data) {
        let key = match &id {
            b"INAM" => "title",
            b"IART" => "artist",
            b"ICRD" => "date",
            b"ICMT" => "comment",
            b"IGNR" => "genre",
            b"ISFT" => "encoder",
            b"ICOP" => "copyright",
            b"IPRD" => "album",
            b"ISBJ" => "subject",
            b"IENG" => "engineer",
            b"ILNG" => "language",
            _ => continue,
        };
        if let Some(value) = text(body) {
            info.add_tag(key, &value);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Cursor;

    use super::*;

    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = id.to_vec();
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
        if body.len() % 2 == 1 {
            out.push(0);
        }
        out
    }

    fn list(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        chunk(b"LIST", &[&kind[..], body].concat())
    }

    /// 640×480 XviD at 25 fps for 100 frames, plus MP3 audio and a title.
    pub fn sample_avi() -> Vec<u8> {
        let le = |v: u32| v.to_le_bytes();
        let avih = [le(40_000), le(0), le(0), le(0), le(100), le(0), le(2), le(0), le(640), le(480)].concat();
        let vstrh = [&b"vids"[..], b"xvid", &[0; 12], &le(1), &le(25), &le(0), &le(100), &[0; 20]].concat();
        let vstrf = [le(40), le(640), le(480), le(0x0018_0001), *b"XVID", [0; 4]].concat();
        let astrh = [&b"auds"[..], &[0; 4], &[0; 12], &le(1152), &le(44_100), &le(0), &le(383), &[0; 20]].concat();
        let astrf = [&0x55u16.to_le_bytes()[..], &2u16.to_le_bytes(), &le(44_100), &le(16_000), &[1, 0, 0, 0]].concat();
        let hdrl = list(
            b"hdrl",
            &[
                chunk(b"avih", &avih),
                list(b"strl", &[chunk(b"strh", &vstrh), chunk(b"strf", &vstrf)].concat()),
                list(b"strl", &[chunk(b"strh", &astrh), chunk(b"strf", &astrf)].concat()),
            ]
            .concat(),
        );
        let movi = list(b"movi", &[0; 64]);
        let info = list(b"INFO", &chunk(b"INAM", b"Old clip\0"));
        let body = [&b"AVI "[..], &hdrl, &movi, &info].concat();
        chunk(b"RIFF", &body)
    }

    #[test]
    fn parses_sample() {
        let data = sample_avi();
        let info = parse(&mut Cursor::new(&data), data.len() as u64).unwrap();
        assert_eq!(info.container, "avi");
        assert_eq!(info.tags.get("title").map(String::as_str), Some("Old clip"));
        assert_eq!(info.tracks.len(), 2);

        let video = &info.tracks[0];
        assert_eq!((video.codec.as_str(), video.codec_tag.as_str()), ("mpeg4", "XVID"));
        assert_eq!(video.resolution(), Some((640, 480)));
        assert_eq!(video.fps, Some(25.0));
        assert_eq!(video.duration, Some(4.0));

        let audio = &info.tracks[1];
        assert_eq!((audio.codec.as_str(), audio.channels, audio.bitrate), ("mp3", Some(2), Some(128_000)));
    }

    #[test]
    fn truncated_input_does_not_panic() {
        let data = sample_avi();
        for cut in 0..data.len() {
            let _ = parse(&mut Cursor::new(&data[..cut]), cut as u64);
        }
    }
}
