//! Matroska and WebM (EBML).
//!
//! The header elements (`Info`, `Tracks`, `Tags`) are read into memory; clusters are never read.
//! `Tags` usually sits after the clusters, so it is found through the `SeekHead` index.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};

use super::codec::{self, apply_color};
use super::model::{Kind, MediaInfo, Track, iso8601, language, round_fps};
use super::reader::{Bytes, fourcc_str, read_at, text};

const MAGIC: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];
const ELEMENT_CAP: u64 = 16 << 20;

const SEGMENT: u32 = 0x1853_8067;
const SEEK_HEAD: u32 = 0x114D_9B74;
const INFO: u32 = 0x1549_A966;
const TRACKS: u32 = 0x1654_AE6B;
const TAGS: u32 = 0x1254_C367;
const CLUSTER: u32 = 0x1F43_B675;

/// Seconds between the Matroska epoch (2001-01-01) and the Unix epoch.
const MKV_EPOCH_OFFSET: i64 = 978_307_200;

pub fn sniff(magic: &[u8]) -> bool {
    magic.starts_with(&MAGIC)
}

/// Variable-length integer: the count of leading zero bits gives the length.
/// IDs keep the marker bit, sizes drop it. A size with every value bit set means "unknown".
fn vint(first: u8, rest: &[u8], keep_marker: bool) -> Option<(u64, bool)> {
    let len = first.leading_zeros() as usize + 1;
    if len > 8 || rest.len() < len - 1 {
        return None;
    }
    let mask = match len {
        _ if keep_marker => 0xFF,
        8 => 0,
        _ => 0xFFu8 >> len,
    };
    let mut value = (first & mask) as u64;
    for &b in &rest[..len - 1] {
        value = (value << 8) | b as u64;
    }
    let unknown = !keep_marker && value == (1u64 << (7 * len)) - 1;
    Some((value, unknown))
}

fn vint_len(first: u8) -> usize {
    first.leading_zeros() as usize + 1
}

fn read_vint(b: &mut Bytes, keep_marker: bool) -> Option<(u64, bool)> {
    let first = b.u8()?;
    let rest = b.take(vint_len(first).checked_sub(1)?)?;
    vint(first, rest, keep_marker)
}

/// Elements inside an in-memory buffer.
fn elements(data: &[u8]) -> impl Iterator<Item = (u32, &[u8])> {
    let mut b = Bytes::new(data);
    std::iter::from_fn(move || {
        let (id, _) = read_vint(&mut b, true)?;
        let (size, unknown) = read_vint(&mut b, false)?;
        let size = if unknown { b.remaining() } else { usize::try_from(size).ok()? };
        Some((id as u32, b.take(size)?))
    })
}

fn uint(data: &[u8]) -> Option<u64> {
    (data.len() <= 8).then(|| data.iter().fold(0u64, |n, &b| (n << 8) | b as u64))
}

fn float(data: &[u8]) -> Option<f64> {
    match data.len() {
        4 => Some(f32::from_be_bytes(data.try_into().ok()?) as f64),
        8 => Some(f64::from_be_bytes(data.try_into().ok()?)),
        _ => None,
    }
}

struct Header {
    id: u32,
    size: Option<u64>,
    data_pos: u64,
}

fn header<R: Read + Seek>(r: &mut R, pos: u64) -> Option<Header> {
    r.seek(SeekFrom::Start(pos)).ok()?;
    let mut buf = [0u8; 12];
    r.read_exact(&mut buf[..1]).ok()?;
    let id_len = vint_len(buf[0]);
    if id_len > 4 {
        return None;
    }
    r.read_exact(&mut buf[1..id_len + 1]).ok()?;
    let size_len = vint_len(buf[id_len]);
    if size_len > 8 {
        return None;
    }
    r.read_exact(&mut buf[id_len + 1..id_len + size_len]).ok()?;
    let (id, _) = vint(buf[0], &buf[1..id_len], true)?;
    let (size, unknown) = vint(buf[id_len], &buf[id_len + 1..id_len + size_len], false)?;
    Some(Header { id: id as u32, size: (!unknown).then_some(size), data_pos: pos + (id_len + size_len) as u64 })
}

fn body<R: Read + Seek>(r: &mut R, h: &Header) -> Option<Vec<u8>> {
    read_at(r, h.data_pos, h.size?, ELEMENT_CAP)
}

pub fn parse<R: Read + Seek>(r: &mut R, len: u64) -> Option<MediaInfo> {
    let ebml = header(r, 0)?;
    let doc_type = elements(&body(r, &ebml)?).find(|(id, _)| *id == 0x4282).and_then(|(_, v)| text(v));
    let container = match doc_type.as_deref() {
        Some("webm") => "webm",
        Some("matroska") => "mkv",
        _ => return None,
    };

    let mut pos = ebml.data_pos + ebml.size?;
    let segment = loop {
        let h = header(r, pos)?;
        if h.id == SEGMENT {
            break h;
        }
        pos = h.data_pos + h.size?;
    };
    let seg_start = segment.data_pos;
    let seg_end = segment.size.map_or(len, |s| (seg_start + s).min(len));

    let mut parser = Parser { info: MediaInfo::new(container), ..Default::default() };
    let mut seeks: Vec<(u32, u64)> = Vec::new();

    let mut pos = seg_start;
    while pos < seg_end {
        let Some(h) = header(r, pos) else { break };
        if h.id == CLUSTER || h.size.is_none() {
            break;
        }
        if h.id == SEEK_HEAD {
            if let Some(data) = body(r, &h) {
                seeks.extend(seek_entries(&data, seg_start));
            }
        } else if let Some(data) = matches!(h.id, INFO | TRACKS | TAGS).then(|| body(r, &h)).flatten() {
            parser.element(h.id, &data);
        }
        pos = h.data_pos + h.size.unwrap_or_default();
    }

    // Elements past the first cluster, reachable only through the SeekHead index. A SeekHead
    // can point at a second SeekHead at the end of the file.
    let mut i = 0;
    while i < seeks.len() {
        let (id, at) = seeks[i];
        i += 1;
        if parser.done.contains(&id) || at >= len {
            continue;
        }
        let Some(h) = header(r, at).filter(|h| h.id == id) else { continue };
        let Some(data) = body(r, &h) else { continue };
        if id == SEEK_HEAD {
            parser.done.push(SEEK_HEAD);
            seeks.extend(seek_entries(&data, seg_start));
        } else {
            parser.element(id, &data);
        }
    }

    let mut info = parser.info;
    if info.duration.is_none() {
        info.duration = info.tracks.iter().filter_map(|t| t.duration).reduce(f64::max);
    }
    Some(info)
}

fn seek_entries(data: &[u8], seg_start: u64) -> Vec<(u32, u64)> {
    elements(data)
        .filter(|(id, _)| *id == 0x4DBB)
        .filter_map(|(_, seek)| {
            let mut id = None;
            let mut at = None;
            for (child, value) in elements(seek) {
                match child {
                    0x53AB => id = uint(value),
                    0x53AC => at = uint(value),
                    _ => {}
                }
            }
            Some((id? as u32, seg_start.checked_add(at?)?))
        })
        .filter(|(id, _)| matches!(*id, INFO | TRACKS | TAGS | SEEK_HEAD))
        .collect()
}

#[derive(Default)]
struct Parser {
    info: MediaInfo,
    done: Vec<u32>,
    track_uids: HashMap<u64, usize>,
}

impl Parser {
    fn element(&mut self, id: u32, data: &[u8]) {
        if self.done.contains(&id) {
            return;
        }
        self.done.push(id);
        match id {
            INFO => self.segment_info(data),
            TRACKS => self.tracks(data),
            TAGS => self.tags(data),
            _ => {}
        }
    }

    fn segment_info(&mut self, data: &[u8]) {
        let mut scale = 1_000_000u64;
        let mut duration = None;
        for (id, value) in elements(data) {
            match id {
                0x2AD7B1 => scale = uint(value).filter(|&s| s > 0).unwrap_or(scale),
                0x4489 => duration = float(value),
                0x7BA9 | 0x5741 => {
                    if let Some(v) = text(value) {
                        self.info.add_tag(if id == 0x7BA9 { "title" } else { "encoder" }, &v);
                    }
                }
                0x4461 => {
                    if let Some(ns) = uint(value) {
                        let secs = (ns as i64).div_euclid(1_000_000_000) + MKV_EPOCH_OFFSET;
                        self.info.add_tag("creation_time", &iso8601(secs));
                    }
                }
                _ => {}
            }
        }
        self.info.duration = duration.filter(|d| *d > 0.0).map(|d| d * scale as f64 / 1e9);
    }

    fn tracks(&mut self, data: &[u8]) {
        for (_, entry) in elements(data).filter(|(id, _)| *id == 0xAE) {
            let (track, uid) = track_entry(entry);
            if let Some(uid) = uid {
                self.track_uids.insert(uid, self.info.tracks.len());
            }
            self.info.tracks.push(track);
        }
    }

    fn tags(&mut self, data: &[u8]) {
        for (_, tag) in elements(data).filter(|(id, _)| *id == 0x7373) {
            let mut track_uids = Vec::new();
            let mut simple = Vec::new();
            for (id, value) in elements(tag) {
                match id {
                    0x63C0 => track_uids.extend(
                        elements(value)
                            .filter(|(id, _)| *id == 0x63C5)
                            .filter_map(|(_, v)| uint(v))
                            .filter(|&u| u != 0),
                    ),
                    0x67C8 => {
                        let mut name = None;
                        let mut string = None;
                        for (id, value) in elements(value) {
                            match id {
                                0x45A3 => name = text(value),
                                0x4487 => string = text(value),
                                _ => {}
                            }
                        }
                        if let (Some(n), Some(s)) = (name, string) {
                            simple.push((n, s));
                        }
                    }
                    _ => {}
                }
            }
            if track_uids.is_empty() {
                for (name, value) in &simple {
                    self.info.add_tag(name, value);
                }
                continue;
            }
            for uid in track_uids {
                let Some(track) = self.track_uids.get(&uid).and_then(|&i| self.info.tracks.get_mut(i)) else {
                    continue;
                };
                for (name, value) in &simple {
                    statistics_tag(track, name, value);
                }
            }
        }
    }
}

/// Per-track statistics written by mkvmerge and ffmpeg.
fn statistics_tag(track: &mut Track, name: &str, value: &str) {
    match name.to_ascii_uppercase().as_str() {
        "BPS" => track.bitrate = track.bitrate.or(value.parse().ok()),
        "NUMBER_OF_FRAMES" => track.frame_count = track.frame_count.or(value.parse().ok()),
        "DURATION" => track.duration = track.duration.or(parse_timestamp(value)),
        "TITLE" => track.name = track.name.take().or(Some(value.to_string())),
        _ => {}
    }
    if track.kind == Kind::Video
        && track.fps.is_none()
        && let (Some(frames), Some(secs)) = (track.frame_count, track.duration)
        && secs > 0.0
    {
        track.fps = round_fps(frames as f64 / secs);
    }
}

/// `HH:MM:SS.nnnnnnnnn`.
fn parse_timestamp(s: &str) -> Option<f64> {
    let mut parts = s.split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let sec: f64 = parts.next()?.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + sec)
}

fn track_entry(data: &[u8]) -> (Track, Option<u64>) {
    let mut track = Track { default: true, language: Some("eng".into()), ..Default::default() };
    let mut uid = None;
    let mut codec_id = String::new();
    let mut private: &[u8] = &[];
    let mut bcp47 = None;
    let mut default_duration = None;
    for (id, value) in elements(data) {
        match id {
            0xD7 => track.id = uint(value).unwrap_or_default(),
            0x73C5 => uid = uint(value),
            0x83 => {
                track.kind = match uint(value) {
                    Some(1) => Kind::Video,
                    Some(2) => Kind::Audio,
                    Some(17) => Kind::Subtitle,
                    _ => Kind::Other,
                }
            }
            0x88 => track.default = uint(value) != Some(0),
            0x55AA => track.forced = uint(value).is_some_and(|v| v != 0),
            0x536E => track.name = text(value),
            0x22B59C => track.language = text(value).and_then(|l| language(&l)),
            0x22B59D => bcp47 = text(value).and_then(|l| language(&l)),
            0x86 => codec_id = text(value).unwrap_or_default(),
            0x63A2 => private = value,
            0x23E383 => default_duration = uint(value).filter(|&ns| ns > 0),
            0xE0 => video(&mut track, value),
            0xE1 => audio(&mut track, value),
            _ => {}
        }
    }
    track.language = bcp47.or(track.language);
    // Audio tracks use DefaultDuration for codec frame length, not a frame rate.
    if track.kind == Kind::Video {
        track.fps = default_duration.and_then(|ns| round_fps(1e9 / ns as f64));
    }

    let known = codec::mkv_codec(&codec_id);
    track.set_codec(codec_id.clone(), known);
    if track.kind == Kind::Audio && !matches!(track.codec.as_str(), "pcm" | "flac" | "alac" | "truehd") {
        track.bit_depth = None;
    }
    match codec_id.as_str() {
        "V_MPEG4/ISO/AVC" => _ = codec::apply_avcc(&mut track, private),
        "V_MPEGH/ISO/HEVC" => _ = codec::apply_hvcc(&mut track, private),
        "V_AV1" => _ = codec::apply_av1c(&mut track, private),
        "V_VP9" => _ = codec::apply_mkv_vp9(&mut track, private),
        "A_AAC" => _ = codec::apply_aac(&mut track, private),
        // BITMAPINFOHEADER: biCompression at offset 16.
        "V_MS/VFW/FOURCC" => {
            if let Some(tag) = private.get(16..20).and_then(|t| <[u8; 4]>::try_from(t).ok()) {
                let known = codec::vfw_codec(&tag);
                track.codec = known.map_or_else(|| fourcc_str(&tag).to_ascii_lowercase(), str::to_string);
            }
        }
        // WAVEFORMATEX: wFormatTag first.
        "A_MS/ACM" => {
            if let Some(tag) = Bytes::new(private).u16le() {
                track.codec = codec::wave_codec(tag).map_or_else(|| format!("0x{tag:04x}"), str::to_string);
            }
        }
        id if id.starts_with("A_AAC/") => {
            track.profile = match id.rsplit('/').next() {
                Some("LC") => Some("LC".into()),
                Some("MAIN") => Some("Main".into()),
                Some("SBR") => Some("HE-AAC".into()),
                Some("LTP") => Some("LTP".into()),
                _ => None,
            }
        }
        _ => {}
    }
    (track, uid)
}

fn video(track: &mut Track, data: &[u8]) {
    let (mut display_w, mut display_h, mut unit) = (None, None, 0);
    for (id, value) in elements(data) {
        match id {
            0xB0 => track.width = uint(value).map(|v| v as u32),
            0xBA => track.height = uint(value).map(|v| v as u32),
            0x54B0 => display_w = uint(value).map(|v| v as u32),
            0x54BA => display_h = uint(value).map(|v| v as u32),
            0x54B2 => unit = uint(value).unwrap_or_default(),
            0x55B0 => colour(track, value),
            _ => {}
        }
    }
    let (Some(w), Some(h)) = (track.width, track.height) else { return };
    let (dw, dh) = match (display_w.filter(|&v| v > 0), display_h.filter(|&v| v > 0)) {
        (Some(dw), Some(dh)) if unit == 0 => (dw, dh),
        // DisplayUnit 3 means the display values are only an aspect ratio.
        (Some(dw), Some(dh)) if unit == 3 => ((h as u64 * dw as u64 / dh as u64) as u32, h),
        _ => (w, h),
    };
    track.display_width = Some(dw);
    track.display_height = Some(dh);
    track.rotation = Some(0);
}

fn colour(track: &mut Track, data: &[u8]) {
    let (mut primaries, mut transfer, mut matrix) = (2, 2, 2);
    let (mut horz, mut vert) = (None, None);
    for (id, value) in elements(data) {
        let v = uint(value);
        match id {
            0x55B1 => matrix = v.unwrap_or(2),
            0x55BA => transfer = v.unwrap_or(2),
            0x55BB => primaries = v.unwrap_or(2),
            0x55B2 => track.bit_depth = v.filter(|&b| b > 0).map(|b| b as u32).or(track.bit_depth),
            0x55B3 => horz = v,
            0x55B4 => vert = v,
            _ => {}
        }
    }
    apply_color(track, primaries, transfer, matrix);
    if let (Some(h), Some(v)) = (horz, vert) {
        track.chroma = match (h, v) {
            (1, 1) => Some("4:2:0".into()),
            (1, 0) => Some("4:2:2".into()),
            (0, 0) => Some("4:4:4".into()),
            _ => track.chroma.take(),
        };
    }
}

fn audio(track: &mut Track, data: &[u8]) {
    let mut rate = None;
    let mut output_rate = None;
    track.channels = Some(1);
    for (id, value) in elements(data) {
        match id {
            0xB5 => rate = float(value),
            0x78B5 => output_rate = float(value),
            0x9F => track.channels = uint(value).map(|c| c as u32),
            0x6264 => track.bit_depth = uint(value).map(|b| b as u32),
            _ => {}
        }
    }
    track.sample_rate = output_rate.or(rate).filter(|r| *r >= 1.0).map(|r| r as u32);
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn el(id: u32, body: &[u8]) -> Vec<u8> {
        let mut out: Vec<u8> = id.to_be_bytes().into_iter().skip_while(|&b| b == 0).collect();
        // 8-byte size: 0x01 marker then 7 bytes of length.
        out.push(0x01);
        out.extend_from_slice(&(body.len() as u64).to_be_bytes()[1..]);
        out.extend_from_slice(body);
        out
    }

    fn sample() -> Vec<u8> {
        let ebml = el(0x1A45_DFA3, &el(0x4282, b"webm"));
        let info = el(INFO, &[el(0x2AD7B1, &[0x0F, 0x42, 0x40]), el(0x4489, &90_500f64.to_be_bytes())].concat());
        let video = el(
            0xAE,
            &[
                el(0xD7, &[1]),
                el(0x73C5, &[7]),
                el(0x83, &[1]),
                el(0x86, b"V_VP9"),
                el(0x23E383, &33_333_333u32.to_be_bytes()),
                el(0xE0, &[el(0xB0, &[0x0F, 0x00]), el(0xBA, &[0x08, 0x70])].concat()),
            ]
            .concat(),
        );
        let subs = el(
            0xAE,
            &[
                el(0xD7, &[2]),
                el(0x83, &[17]),
                el(0x86, b"S_TEXT/WEBVTT"),
                el(0x22B59C, b"rus"),
                el(0x55AA, &[1]),
                el(0x88, &[0]),
            ]
            .concat(),
        );
        let tracks = el(TRACKS, &[video, subs].concat());
        let cluster = el(CLUSTER, &[0; 32]);
        let tags = el(TAGS, &el(0x7373, &el(0x67C8, &[el(0x45A3, b"ARTIST"), el(0x4487, b"Someone")].concat())));
        // The SeekHead entry for Tags is filled in once the layout is known.
        let seek = |at: u64| {
            let entry = [el(0x53AB, &TAGS.to_be_bytes()), el(0x53AC, &at.to_be_bytes())].concat();
            el(SEEK_HEAD, &el(0x4DBB, &entry))
        };
        let head_len = seek(0).len() + info.len() + tracks.len() + cluster.len();
        let segment_body = [seek(head_len as u64), info, tracks, cluster, tags].concat();
        [ebml, el(SEGMENT, &segment_body)].concat()
    }

    #[test]
    fn parses_sample() {
        let data = sample();
        let info = parse(&mut Cursor::new(&data), data.len() as u64).unwrap();
        assert_eq!(info.container, "webm");
        assert_eq!(info.duration, Some(90.5));
        assert_eq!(info.tags.get("artist").map(String::as_str), Some("Someone"));

        let video = &info.tracks[0];
        assert_eq!((video.kind, video.codec.as_str()), (Kind::Video, "vp9"));
        assert_eq!(video.resolution(), Some((3840, 2160)));
        assert_eq!(video.fps, Some(30.0));

        let sub = &info.tracks[1];
        assert_eq!((sub.kind, sub.codec.as_str()), (Kind::Subtitle, "webvtt"));
        assert_eq!(sub.language.as_deref(), Some("rus"));
        assert!(sub.forced && !sub.default);
    }

    #[test]
    fn truncated_input_does_not_panic() {
        let data = sample();
        for cut in 0..data.len() {
            let _ = parse(&mut Cursor::new(&data[..cut]), cut as u64);
        }
    }

    #[test]
    fn timestamps() {
        assert_eq!(parse_timestamp("01:02:03.500000000"), Some(3723.5));
    }
}
