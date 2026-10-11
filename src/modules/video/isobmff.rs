//! ISO base media file format: MP4, M4V, MOV (QuickTime), 3GP.
//!
//! Everything comes from the `moov` box, which is read into memory once. Sample data (`mdat`,
//! `moof`) is skipped by seeking.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};

use super::codec::{self, apply_color};
use super::model::{Kind, MediaInfo, Track, iso8601, language, round_fps};
use super::reader::{Bytes, fourcc_str, read_at, text};

const MOOV_CAP: u64 = 64 << 20;
/// Seconds between the MP4 epoch (1904) and the Unix epoch.
const MP4_EPOCH_OFFSET: u64 = 2_082_844_800;

pub fn sniff(magic: &[u8]) -> bool {
    magic.len() >= 8 && matches!(&magic[4..8], b"ftyp" | b"moov" | b"mdat" | b"free" | b"skip" | b"wide" | b"pnot")
}

struct Atom<'a> {
    kind: [u8; 4],
    body: &'a [u8],
}

fn atoms(data: &[u8]) -> impl Iterator<Item = Atom<'_>> {
    let mut b = Bytes::new(data);
    std::iter::from_fn(move || {
        let size = b.u32be()? as u64;
        let kind = b.fourcc()?;
        let body_len = match size {
            0 => b.remaining() as u64,
            1 => b.u64be()?.checked_sub(16)?,
            n => n.checked_sub(8)?,
        };
        let body = b.take(usize::try_from(body_len).ok()?)?;
        Some(Atom { kind, body })
    })
}

fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    atoms(data).find(|a| &a.kind == kind).map(|a| a.body)
}

fn path<'a>(data: &'a [u8], kinds: &[&[u8; 4]]) -> Option<&'a [u8]> {
    kinds.iter().try_fold(data, |d, k| child(d, k))
}

#[derive(Default)]
struct TopLevel {
    brand: Option<[u8; 4]>,
    moov: Option<Vec<u8>>,
    sidx_duration: Option<f64>,
    fragments: HashMap<u32, Fragments>,
}

/// Per-track defaults from `mvex/trex`, used by fragments that don't list every sample.
#[derive(Default, Clone, Copy)]
struct Trex {
    duration: u32,
    size: u32,
}

/// Sample totals summed over all `moof` fragments of one track.
#[derive(Default)]
struct Fragments {
    samples: u64,
    ticks: u64,
    bytes: u64,
    first_offset: Option<i64>,
}

/// Walk the top-level boxes by seeking from header to header.
fn scan<R: Read + Seek>(r: &mut R, len: u64) -> Option<TopLevel> {
    let mut top = TopLevel::default();
    let mut trex = HashMap::new();
    let mut pos = 0u64;
    while pos + 8 <= len {
        r.seek(SeekFrom::Start(pos)).ok()?;
        let mut head = [0u8; 8];
        r.read_exact(&mut head).ok()?;
        let size = u32::from_be_bytes(head[..4].try_into().ok()?) as u64;
        let kind: [u8; 4] = head[4..].try_into().ok()?;
        let (header, size) = match size {
            0 => (8, len - pos),
            1 => {
                let mut large = [0u8; 8];
                r.read_exact(&mut large).ok()?;
                (16, u64::from_be_bytes(large))
            }
            n => (8, n),
        };
        if size < header {
            break;
        }
        let body_len = size - header;
        match &kind {
            b"ftyp" => top.brand = read_at(r, pos + header, 4, 4).and_then(|b| b.try_into().ok()),
            b"moov" => {
                top.moov = read_at(r, pos + header, body_len, MOOV_CAP);
                trex = top.moov.as_deref().map(trex_defaults).unwrap_or_default();
            }
            b"sidx" => {
                let secs = read_at(r, pos + header, body_len, 1 << 20).and_then(|b| sidx_duration(&b));
                top.sidx_duration = top.sidx_duration.into_iter().chain(secs).reduce(f64::max);
            }
            b"moof" => {
                if let Some(body) = read_at(r, pos + header, body_len, 4 << 20) {
                    movie_fragment(&body, &trex, &mut top.fragments);
                }
            }
            _ => {}
        }
        pos = match pos.checked_add(size) {
            Some(p) => p,
            None => break,
        };
    }
    Some(top)
}

fn trex_defaults(moov: &[u8]) -> HashMap<u32, Trex> {
    let mut out = HashMap::new();
    for atom in child(moov, b"mvex").into_iter().flat_map(atoms).filter(|a| &a.kind == b"trex") {
        let mut b = Bytes::new(atom.body);
        if let (Some(_), Some(id), Some(_), Some(duration), Some(size)) =
            (b.u32be(), b.u32be(), b.u32be(), b.u32be(), b.u32be())
        {
            out.insert(id, Trex { duration, size });
        }
    }
    out
}

/// Add up sample durations and sizes from each `traf/trun` of a `moof`.
fn movie_fragment(moof: &[u8], trex: &HashMap<u32, Trex>, out: &mut HashMap<u32, Fragments>) -> Option<()> {
    for traf in atoms(moof).filter(|a| &a.kind == b"traf") {
        let mut b = Bytes::new(child(traf.body, b"tfhd")?);
        let flags = b.u32be()? & 0x00FF_FFFF;
        let id = b.u32be()?;
        let mut defaults = trex.get(&id).copied().unwrap_or_default();
        if flags & 0x01 != 0 {
            b.skip(8)?;
        }
        if flags & 0x02 != 0 {
            b.skip(4)?;
        }
        if flags & 0x08 != 0 {
            defaults.duration = b.u32be()?;
        }
        if flags & 0x10 != 0 {
            defaults.size = b.u32be()?;
        }

        let totals = out.entry(id).or_default();
        for trun in atoms(traf.body).filter(|a| &a.kind == b"trun") {
            let mut b = Bytes::new(trun.body);
            let flags = b.u32be()? & 0x00FF_FFFF;
            let count = b.u32be()?;
            b.skip(if flags & 0x01 != 0 { 4 } else { 0 } + if flags & 0x04 != 0 { 4 } else { 0 })?;
            for _ in 0..count {
                let duration = if flags & 0x100 != 0 { b.u32be()? } else { defaults.duration };
                let size = if flags & 0x200 != 0 { b.u32be()? } else { defaults.size };
                b.skip(if flags & 0x400 != 0 { 4 } else { 0 })?;
                if flags & 0x800 != 0 {
                    let offset = b.i32be()? as i64;
                    if totals.samples == 0 {
                        totals.first_offset = Some(offset);
                    }
                }
                totals.samples += 1;
                totals.ticks += duration as u64;
                totals.bytes += size as u64;
            }
        }
    }
    Some(())
}

fn sidx_duration(body: &[u8]) -> Option<f64> {
    let mut b = Bytes::new(body);
    let version = b.u8()?;
    b.skip(3 + 4)?;
    let timescale = b.u32be()?;
    b.skip(if version == 0 { 8 } else { 16 })?;
    b.skip(2)?;
    let count = b.u16be()?;
    let mut total = 0u64;
    for _ in 0..count {
        b.skip(4)?;
        total += b.u32be()? as u64;
        b.skip(4)?;
    }
    (timescale > 0 && total > 0).then(|| total as f64 / timescale as f64)
}

fn mehd_duration(body: &[u8]) -> Option<u64> {
    let mut b = Bytes::new(body);
    let version = b.u8()?;
    b.skip(3)?;
    if version == 1 { b.u64be() } else { b.u32be().map(u64::from) }
}

/// Shared prefix of `mvhd` and `mdhd`.
struct Times {
    created: u64,
    timescale: u32,
    duration: Option<u64>,
}

impl Times {
    fn secs(&self) -> Option<f64> {
        let duration = self.duration?;
        (self.timescale > 0).then(|| duration as f64 / self.timescale as f64)
    }
}

fn times(body: &[u8]) -> Option<(Times, Bytes<'_>)> {
    let mut b = Bytes::new(body);
    let version = b.u8()?;
    b.skip(3)?;
    let (created, timescale, duration) = if version == 1 {
        let created = b.u64be()?;
        b.skip(8)?;
        (created, b.u32be()?, b.u64be()?)
    } else {
        let created = b.u32be()? as u64;
        b.skip(4)?;
        let timescale = b.u32be()?;
        let duration = match b.u32be()? {
            u32::MAX => u64::MAX,
            d => d as u64,
        };
        (created, timescale, duration)
    };
    let duration = (duration != 0 && duration != u64::MAX).then_some(duration);
    Some((Times { created, timescale, duration }, b))
}

pub fn parse<R: Read + Seek>(r: &mut R, len: u64) -> Option<MediaInfo> {
    let top = scan(r, len)?;
    let moov = top.moov?;
    let container = match top.brand.as_ref() {
        Some(b"qt  ") => "mov",
        Some(b"M4V " | b"M4VH" | b"M4VP") => "m4v",
        Some(b"M4A " | b"M4B ") => "m4a",
        Some(b) if b.starts_with(b"3g") => "3gp",
        _ => "mp4",
    };
    let mut info = MediaInfo::new(container);

    if let Some(udta) = child(&moov, b"udta") {
        user_data(&mut info, udta);
    }
    if let Some(meta_box) = child(&moov, b"meta") {
        meta(&mut info, meta_box);
    }

    let movie = child(&moov, b"mvhd").and_then(times).map(|(t, _)| t);
    if let Some(movie) = &movie {
        info.duration = movie.secs();
        if movie.created > MP4_EPOCH_OFFSET {
            info.add_tag("creation_time", &iso8601((movie.created - MP4_EPOCH_OFFSET) as i64));
        }
    }

    if info.duration.is_none()
        && let Some(movie) = &movie
        && let Some(mehd) = path(&moov, &[b"mvex", b"mehd"])
    {
        info.duration = mehd_duration(mehd)
            .filter(|&d| d > 0 && movie.timescale > 0)
            .map(|d| d as f64 / movie.timescale as f64);
    }

    let mut chapter_tracks = HashSet::new();
    for atom in atoms(&moov).filter(|a| &a.kind == b"trak") {
        if let Some(chap) = path(atom.body, &[b"tref", b"chap"]) {
            let mut b = Bytes::new(chap);
            while let Some(id) = b.u32be() {
                chapter_tracks.insert(id as u64);
            }
        }
        if let Some(track) = trak(atom.body, movie.as_ref().map_or(0, |m| m.timescale), &top.fragments) {
            info.tracks.push(track);
        }
    }

    // MP4 has no "default track" flag; treat the first enabled track of each kind as default.
    let mut seen = HashSet::new();
    for track in &mut info.tracks {
        if chapter_tracks.contains(&track.id) {
            track.kind = Kind::Other;
        }
        track.default = track.default && seen.insert(track.kind);
    }

    // The longest track as stored, like ffprobe. `mvhd` is only the muxer's claim and can overstate it.
    let longest = info.tracks.iter().filter_map(|t| t.duration).reduce(f64::max);
    info.duration = longest.or(info.duration).or(top.sidx_duration);
    Some(info)
}

fn trak(body: &[u8], movie_timescale: u32, fragments: &HashMap<u32, Fragments>) -> Option<Track> {
    let mdia = child(body, b"mdia")?;
    let kind = match child(mdia, b"hdlr").and_then(|h| h.get(8..12)) {
        Some(b"vide") => Kind::Video,
        Some(b"soun") => Kind::Audio,
        Some(b"sbtl" | b"subt" | b"text" | b"clcp") => Kind::Subtitle,
        _ => Kind::Other,
    };
    let mut track = Track::new(kind);

    let mut b = Bytes::new(child(body, b"tkhd")?);
    let version = b.u8()?;
    let flags = b.u24be()?;
    b.skip(if version == 1 { 16 } else { 8 })?;
    track.id = b.u32be()? as u64;
    b.skip(4 + if version == 1 { 8 } else { 4 })?;
    b.skip(8 + 2 + 2 + 2 + 2)?;
    let (a, rot_b) = (b.i32be()?, b.i32be()?);
    b.skip(7 * 4)?;
    let tkhd_width = b.u32be()? >> 16;
    let tkhd_height = b.u32be()? >> 16;
    track.default = flags & 1 != 0;

    let (media, mut rest) = times(child(mdia, b"mdhd")?)?;
    let fragments = fragments.get(&(track.id as u32)).filter(|f| f.samples > 0);
    track.duration = media.secs().or_else(|| {
        let ticks = fragments?.ticks;
        (ticks > 0 && media.timescale > 0).then(|| ticks as f64 / media.timescale as f64)
    });
    track.language = rest.u16be().and_then(mp4_language);
    if let Some(elng) = child(mdia, b"elng") {
        track.language = elng.get(4..).and_then(text).and_then(|l| language(&l)).or(track.language);
    }
    track.name = path(body, &[b"udta", b"name"]).and_then(text);

    let stbl = path(mdia, &[b"minf", b"stbl"])?;
    let extras = child(stbl, b"stsd").and_then(|stsd| sample_entry(&mut track, stsd)).unwrap_or_default();
    if track.codec.is_empty() {
        return Some(track);
    }

    let mut runs = child(stbl, b"stts").map(stts).unwrap_or_default();
    if runs.is_empty()
        && let Some(f) = fragments
    {
        // Fragments only give totals, so assume evenly spaced samples.
        runs.push((f.samples, f.ticks / f.samples));
    }
    let frames: u64 = runs.iter().map(|r| r.0).sum();
    let ticks: u64 = runs.iter().map(|r| r.0 * r.1).sum();
    if runs.is_empty()
        && let Some(duration) = media.duration
    {
        // No sample timing at all: treat the media as continuous for the edit list.
        runs.push((duration, 1));
    }
    let shift = child(stbl, b"ctts")
        .and_then(first_composition_offset)
        .or(fragments.and_then(|f| f.first_offset))
        .unwrap_or(0);
    if let Some(elst) = path(body, &[b"edts", b"elst"])
        && let Some(secs) = edited_duration(elst, movie_timescale, media.timescale, &runs, shift)
    {
        track.duration = Some(secs);
    } else if ticks > 0 && media.timescale > 0 {
        // Without an edit list the B-frame delay is a gap before the first frame, like an empty edit.
        track.duration = Some((ticks as i64 + shift.max(0)) as f64 / media.timescale as f64);
    }

    if kind == Kind::Video {
        if let (Some(w), Some(h)) = (track.width, track.height) {
            let (dw, dh) = if tkhd_width > 0 && tkhd_height > 0 {
                (tkhd_width, tkhd_height)
            } else if let Some((hs, vs)) = extras.pasp {
                ((w as u64 * hs as u64 / vs as u64) as u32, h)
            } else {
                (w, h)
            };
            let rotation = if a == 0 && rot_b == 0 {
                0
            } else {
                let degrees = (rot_b as f64).atan2(a as f64).to_degrees();
                ((degrees / 90.0).round() as i32 * 90).rem_euclid(360) as u32
            };
            let (dw, dh) = if rotation % 180 == 90 { (dh, dw) } else { (dw, dh) };
            track.display_width = Some(dw);
            track.display_height = Some(dh);
            track.rotation = Some(rotation);
        }

        if frames > 0 && ticks > 0 {
            track.frame_count = Some(frames);
            track.fps = round_fps(frames as f64 * media.timescale as f64 / ticks as f64);
        }
    }

    let bytes = child(stbl, b"stsz").and_then(stsz).filter(|&b| b > 0).or(fragments.map(|f| f.bytes)).unwrap_or(0);
    track.bitrate = match track.duration {
        Some(secs) if bytes > 0 && secs > 0.0 => Some((bytes as f64 * 8.0 / secs) as u64),
        _ => extras.avg_bitrate.filter(|&b| b > 0).map(u64::from),
    };
    Some(track)
}

/// Presentation length of a track with an edit list, as ffmpeg measures it: each edit runs from
/// its start to the end of the sample its end falls in, and never past the last sample. Empty
/// edits (a delay before the track starts) count towards the length.
///
/// `runs` are the `stts` (count, duration) pairs; `shift` is the first sample's composition
/// offset, the B-frame delay that edit start times are expressed in.
fn edited_duration(
    elst: &[u8],
    movie_timescale: u32,
    media_timescale: u32,
    runs: &[(u64, u64)],
    shift: i64,
) -> Option<f64> {
    if movie_timescale == 0 || media_timescale == 0 {
        return None;
    }
    let scale = media_timescale as f64 / movie_timescale as f64;
    let mut b = Bytes::new(elst);
    let version = b.u8()?;
    b.skip(3)?;
    let count = b.u32be()?;
    let mut ticks = 0.0;
    for _ in 0..count {
        let (segment, start) = if version == 1 {
            (b.u64be()?, b.u64be()? as i64)
        } else {
            (b.u32be()? as u64, b.i32be()? as i64)
        };
        b.skip(4)?;
        let segment = segment as f64 * scale;
        if start < 0 {
            ticks += segment;
            continue;
        }
        let from = start as f64;
        // A zero-length edit in a fragmented file means "until the end of the media".
        let to = if segment > 0.0 { from + segment } else { f64::INFINITY };
        let mut end = shift as f64;
        for &(count, delta) in runs {
            let run_end = end + (count * delta) as f64;
            if to < run_end && delta > 0 {
                end += ((to - end) / delta as f64).ceil().max(0.0) * delta as f64;
                break;
            }
            end = run_end;
        }
        ticks += (end - from).max(0.0);
    }
    let secs = ticks / media_timescale as f64;
    (secs > 0.0).then_some(secs)
}

fn first_composition_offset(ctts: &[u8]) -> Option<i64> {
    let mut b = Bytes::new(ctts);
    b.skip(4 + 4 + 4)?;
    b.i32be().map(i64::from)
}

/// `mdhd` language: three 5-bit letters, or a Macintosh language code below 0x400 in QuickTime.
fn mp4_language(code: u16) -> Option<String> {
    if code < 0x400 {
        const MAC: [&str; 12] = ["eng", "fra", "deu", "ita", "nld", "swe", "spa", "dan", "por", "nor", "heb", "jpn"];
        return MAC.get(code as usize).map(|s| s.to_string());
    }
    let letters: String = [10, 5, 0].iter().map(|s| (((code >> s) & 0x1F) as u8 + 0x60) as char).collect();
    if letters.chars().all(|c| c.is_ascii_lowercase()) { language(&letters) } else { None }
}

#[derive(Default)]
struct EntryExtras {
    pasp: Option<(u32, u32)>,
    avg_bitrate: Option<u32>,
}

/// The first sample description: codec, coded size or audio format, and codec config boxes.
fn sample_entry(track: &mut Track, stsd: &[u8]) -> Option<EntryExtras> {
    let entry = atoms(stsd.get(8..)?).next()?;
    let mut b = Bytes::new(entry.body);
    b.skip(8)?;
    let mut sample_bits = None;
    let children = match track.kind {
        Kind::Video => {
            b.skip(16)?;
            let (w, h) = (b.u16be()?, b.u16be()?);
            b.skip(50)?;
            track.width = (w > 0).then_some(w as u32);
            track.height = (h > 0).then_some(h as u32);
            b.rest()
        }
        Kind::Audio => {
            let version = b.u16be()?;
            b.skip(6)?;
            let mut channels = b.u16be()? as u32;
            let mut bits = b.u16be()? as u32;
            b.skip(4)?;
            let mut rate = (b.u32be()? >> 16) as f64;
            match version {
                1 => b.skip(16)?,
                2 => {
                    b.skip(4)?;
                    rate = b.f64be()?;
                    channels = b.u32be()?;
                    b.skip(4)?;
                    bits = b.u32be()?;
                    b.skip(12)?;
                }
                _ => {}
            }
            track.channels = (channels > 0).then_some(channels);
            track.sample_rate = (rate >= 1.0).then_some(rate as u32);
            sample_bits = Some(bits);
            b.rest()
        }
        _ => &[],
    };

    let mut tag = entry.kind;
    if matches!(&tag, b"encv" | b"enca")
        && let Some(original) = path(children, &[b"sinf", b"frma"]).and_then(|f| f.get(..4))
    {
        tag.copy_from_slice(original);
    }
    track.set_codec(fourcc_str(&tag), codec::mp4_codec(&tag));
    if matches!(track.codec.as_str(), "pcm" | "alac" | "flac") {
        // QuickTime PCM fourccs carry the depth; the sample entry field then just says 16.
        track.bit_depth = match &tag {
            b"in24" => Some(24),
            b"in32" | b"fl32" => Some(32),
            b"fl64" => Some(64),
            _ => sample_bits.filter(|&b| b > 0),
        };
    }

    let mut extras = EntryExtras::default();
    for atom in atoms(children) {
        match &atom.kind {
            b"avcC" => _ = codec::apply_avcc(track, atom.body),
            b"hvcC" => _ = codec::apply_hvcc(track, atom.body),
            b"av1C" => _ = codec::apply_av1c(track, atom.body),
            b"vpcC" => _ = codec::apply_vpcc(track, atom.body),
            b"colr" => {
                let mut c = Bytes::new(atom.body);
                if matches!(c.fourcc().as_ref(), Some(b"nclx" | b"nclc"))
                    && let (Some(p), Some(t), Some(m)) = (c.u16be(), c.u16be(), c.u16be())
                {
                    apply_color(track, p as u64, t as u64, m as u64);
                }
            }
            b"pasp" => {
                let mut p = Bytes::new(atom.body);
                if let (Some(h), Some(v)) = (p.u32be(), p.u32be()) {
                    extras.pasp = (h > 0 && v > 0).then_some((h, v));
                }
            }
            b"btrt" => extras.avg_bitrate = Bytes::new(atom.body.get(8..).unwrap_or_default()).u32be(),
            b"esds" => apply_esds(track, atom.body, &mut extras),
            // QuickTime wraps the audio `esds` in a `wave` box.
            b"wave" => {
                if let Some(esds) = child(atom.body, b"esds") {
                    apply_esds(track, esds, &mut extras);
                }
            }
            _ => {}
        }
    }
    Some(extras)
}

fn apply_esds(track: &mut Track, body: &[u8], extras: &mut EntryExtras) {
    let Some(esds) = codec::parse_esds(body) else { return };
    if let Some(name) = codec::mp4_object_type(esds.object_type) {
        track.codec = name.to_string();
    }
    if esds.avg_bitrate > 0 {
        extras.avg_bitrate = Some(esds.avg_bitrate);
    }
    if track.codec == "aac" {
        codec::apply_aac(track, esds.specific);
    }
}

/// Runs of (sample count, sample duration in media timescale ticks).
fn stts(body: &[u8]) -> Vec<(u64, u64)> {
    let mut b = Bytes::new(body);
    let count = b.skip(4).and_then(|_| b.u32be()).unwrap_or(0);
    let mut runs = Vec::new();
    for _ in 0..count {
        let (Some(n), Some(delta)) = (b.u32be(), b.u32be()) else { break };
        runs.push((n as u64, delta as u64));
    }
    runs
}

/// Total size of all samples in bytes.
fn stsz(body: &[u8]) -> Option<u64> {
    let mut b = Bytes::new(body);
    b.skip(4)?;
    let size = b.u32be()?;
    let count = b.u32be()?;
    if size != 0 {
        return Some(size as u64 * count as u64);
    }
    let mut total = 0u64;
    for _ in 0..count {
        total += b.u32be()? as u64;
    }
    Some(total)
}

fn user_data(info: &mut MediaInfo, udta: &[u8]) {
    for atom in atoms(udta) {
        match &atom.kind {
            b"meta" => meta(info, atom.body),
            // QuickTime user data text: 16-bit length, 16-bit language, then the text.
            [0xA9, ..] => {
                let mut b = Bytes::new(atom.body);
                if let (Some(len), Some(_)) = (b.u16be(), b.u16be())
                    && let Some(value) = b.take(len as usize).and_then(text)
                {
                    info.add_tag(&ilst_key(&atom.kind), &value);
                }
            }
            _ => {}
        }
    }
}

/// iTunes-style `ilst`, keyed either by fourcc (`©nam`) or, in QuickTime `mdta` metadata, by
/// a 1-based index into the `keys` box.
fn meta(info: &mut MediaInfo, body: &[u8]) {
    // ISO `meta` is a full box with 4 bytes of version and flags; QuickTime's is not.
    let body = if body.starts_with(&[0, 0, 0, 0]) { &body[4..] } else { body };

    let mut keys = Vec::new();
    if let Some(k) = child(body, b"keys") {
        let mut b = Bytes::new(k);
        let count = b.skip(4).and_then(|_| b.u32be()).unwrap_or(0);
        for _ in 0..count {
            let Some(size) = b.u32be() else { break };
            let Some(key) = b.skip(4).and_then(|_| b.take(size.saturating_sub(8) as usize)) else { break };
            keys.push(String::from_utf8_lossy(key).into_owned());
        }
    }

    let Some(ilst) = child(body, b"ilst") else { return };
    for item in atoms(ilst) {
        let index = u32::from_be_bytes(item.kind) as usize;
        let key = if !keys.is_empty() && (1..=keys.len()).contains(&index) {
            mdta_key(&keys[index - 1])
        } else if &item.kind == b"----" {
            match child(item.body, b"name").and_then(|n| n.get(4..)).and_then(text) {
                Some(name) => name,
                None => continue,
            }
        } else {
            ilst_key(&item.kind)
        };
        let Some(data) = child(item.body, b"data") else { continue };
        let mut b = Bytes::new(data);
        let (Some(kind), Some(_)) = (b.u32be(), b.u32be()) else { continue };
        let value = b.rest();
        let value = match kind & 0x00FF_FFFF {
            1 => String::from_utf8_lossy(value).into_owned(),
            21 | 22 if value.len() <= 8 => {
                let mut n = 0i64;
                for &byte in value {
                    n = (n << 8) | byte as i64;
                }
                n.to_string()
            }
            _ => continue,
        };
        info.add_tag(&key, &value);
    }
}

fn ilst_key(kind: &[u8; 4]) -> String {
    let key = match kind {
        b"\xA9nam" => "title",
        b"\xA9ART" | b"\xA9aut" => "artist",
        b"aART" => "album_artist",
        b"\xA9alb" => "album",
        b"\xA9day" => "date",
        b"\xA9cmt" => "comment",
        b"\xA9gen" => "genre",
        b"\xA9too" | b"\xA9swr" | b"\xA9enc" => "encoder",
        b"desc" | b"\xA9des" => "description",
        b"ldes" => "synopsis",
        b"\xA9wrt" => "composer",
        b"cprt" | b"\xA9cpy" => "copyright",
        b"\xA9grp" => "grouping",
        b"\xA9lyr" => "lyrics",
        b"tvsh" => "show",
        b"\xA9xyz" => "location",
        b"\xA9mak" => "make",
        b"\xA9mod" => "model",
        [0xA9, rest @ ..] => return fourcc_str(rest).to_ascii_lowercase(),
        _ => return fourcc_str(kind).to_ascii_lowercase(),
    };
    key.to_string()
}

fn mdta_key(key: &str) -> String {
    let key = key.strip_prefix("com.apple.quicktime.").unwrap_or(key);
    match key {
        "creationdate" => "creation_time".to_string(),
        "location.ISO6709" => "location".to_string(),
        _ => key.to_string(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::io::Cursor;

    use super::*;

    pub fn atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out
    }

    fn full(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        atom(kind, &[&[0, 0, 0, 0][..], body].concat())
    }

    fn trak(id: u32, handler: &[u8; 4], lang: u16, entry: Vec<u8>, extra_stbl: &[u8], edts: &[u8]) -> Vec<u8> {
        let mut tkhd = vec![0; 12];
        tkhd.extend_from_slice(&id.to_be_bytes());
        tkhd.extend_from_slice(&[0; 4 + 4 + 8 + 8]);
        for m in [0x10000i32, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000] {
            tkhd.extend_from_slice(&m.to_be_bytes());
        }
        tkhd.extend_from_slice(&(1920u32 << 16).to_be_bytes());
        tkhd.extend_from_slice(&(1080u32 << 16).to_be_bytes());
        let mut tkhd = atom(b"tkhd", &tkhd);
        tkhd[11] = 1;

        let mut mdhd = vec![0; 8];
        mdhd.extend_from_slice(&1000u32.to_be_bytes());
        mdhd.extend_from_slice(&10_000u32.to_be_bytes());
        mdhd.extend_from_slice(&lang.to_be_bytes());
        mdhd.extend_from_slice(&[0, 0]);
        let hdlr = full(b"hdlr", &[&[0; 4][..], handler, &[0; 13]].concat());
        let stsd = full(b"stsd", &[&1u32.to_be_bytes()[..], &entry].concat());
        let stbl = atom(b"stbl", &[stsd, extra_stbl.to_vec()].concat());
        let minf = atom(b"minf", &stbl);
        let mdia = atom(b"mdia", &[full(b"mdhd", &mdhd), hdlr, minf].concat());
        atom(b"trak", &[tkhd, edts.to_vec(), mdia].concat())
    }

    /// `elst` body from (segment duration, media time) pairs; media time -1 is an empty edit.
    fn elst(edits: &[(u32, i32)]) -> Vec<u8> {
        let mut body = vec![0, 0, 0, 0];
        body.extend_from_slice(&(edits.len() as u32).to_be_bytes());
        for (duration, start) in edits {
            body.extend_from_slice(&duration.to_be_bytes());
            body.extend_from_slice(&start.to_be_bytes());
            body.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        }
        body
    }

    fn edit_list(edits: &[(u32, i32)]) -> Vec<u8> {
        atom(b"edts", &atom(b"elst", &elst(edits)))
    }

    /// 10 s, 1920×1080 H.264 at 25 fps plus an English `tx3g` subtitle track and a title.
    pub fn sample_mp4() -> Vec<u8> {
        sample_mp4_with_edits(&[])
    }

    fn sample_mp4_with_edits(video_edts: &[u8]) -> Vec<u8> {
        let mut visual = vec![0; 8 + 16];
        visual.extend_from_slice(&1920u16.to_be_bytes());
        visual.extend_from_slice(&1080u16.to_be_bytes());
        visual.extend_from_slice(&[0; 50]);
        visual.extend(atom(b"avcC", &[1, 100, 0, 40, 0xFF, 0xE0]));
        let video_entry = atom(b"avc1", &visual);
        let stts = full(b"stts", &[1u32.to_be_bytes(), 250u32.to_be_bytes(), 40u32.to_be_bytes()].concat());

        let eng = (5 << 10) | (14 << 5) | 7;
        let sub_entry = atom(b"tx3g", &[0; 8]);

        let mut mvhd = vec![0; 8];
        mvhd.extend_from_slice(&1000u32.to_be_bytes());
        mvhd.extend_from_slice(&10_000u32.to_be_bytes());
        mvhd.extend_from_slice(&[0; 80]);

        // `data` type indicator 1 is UTF-8.
        let title = atom(b"\xA9nam", &atom(b"data", &[&1u32.to_be_bytes()[..], &[0; 4], b"Holiday"].concat()));
        let meta = full(b"meta", &atom(b"ilst", &title));
        let udta = atom(b"udta", &meta);

        let moov = atom(
            b"moov",
            &[
                full(b"mvhd", &mvhd),
                trak(1, b"vide", 0x55C4, video_entry, &stts, video_edts),
                trak(2, b"sbtl", eng, sub_entry, &[], &[]),
                udta,
            ]
            .concat(),
        );
        [atom(b"ftyp", b"isom\0\0\0\0isom"), moov, atom(b"mdat", &[0; 16])].concat()
    }

    #[test]
    fn parses_sample() {
        let data = sample_mp4();
        let info = parse(&mut Cursor::new(&data), data.len() as u64).unwrap();
        assert_eq!(info.container, "mp4");
        assert_eq!(info.duration, Some(10.0));
        assert_eq!(info.tags.get("title").map(String::as_str), Some("Holiday"));
        assert_eq!(info.tracks.len(), 2);

        let video = &info.tracks[0];
        assert_eq!((video.kind, video.codec.as_str()), (Kind::Video, "h264"));
        assert_eq!(video.resolution(), Some((1920, 1080)));
        assert_eq!(video.fps, Some(25.0));
        assert_eq!(video.frame_count, Some(250));
        assert_eq!(video.profile.as_deref(), Some("High"));
        assert_eq!(video.level.as_deref(), Some("4.0"));
        assert!(video.default);

        let sub = &info.tracks[1];
        assert_eq!((sub.kind, sub.codec.as_str()), (Kind::Subtitle, "mov_text"));
        assert_eq!(sub.language.as_deref(), Some("eng"));
    }

    #[test]
    fn edit_lists_are_capped_at_the_media() {
        // 10 s of video. The edit claims 10.1 s from 80 ms in, but only 9.92 s of samples remain.
        let data = sample_mp4_with_edits(&edit_list(&[(10_100, 80)]));
        let info = parse(&mut Cursor::new(&data), data.len() as u64).unwrap();
        assert_eq!(info.tracks[0].duration, Some(9.92));
        assert_eq!(info.duration, Some(10.0));

        // A 0.5 s empty edit delays the video, so it ends after the 10 s subtitle track.
        let data = sample_mp4_with_edits(&edit_list(&[(500, -1), (10_100, 80)]));
        let info = parse(&mut Cursor::new(&data), data.len() as u64).unwrap();
        let duration = info.duration.unwrap();
        assert!((duration - 10.42).abs() < 1e-9, "{duration}");
    }

    #[test]
    fn edits_follow_ffmpeg_trimming() {
        // AAC priming: the edit starts mid-packet and covers the rest exactly (OBS, 48 kHz).
        let audio = [(5515, 1024), (1, 72)];
        let secs = edited_duration(&elst(&[(117_640, 712)]), 1000, 48_000, &audio, 0).unwrap();
        assert!((secs - 117.64).abs() < 1e-9, "{secs}");

        // B-frames: the edit starts at the composition delay and is rounded down to milliseconds;
        // the last frame still counts in full.
        let video = [(3265, 256)];
        let secs = edited_duration(&elst(&[(54_416, 512)]), 1000, 15_360, &video, 512).unwrap();
        assert!((secs - 3265.0 / 60.0).abs() < 1e-9, "{secs}");
    }

    #[test]
    fn truncated_input_does_not_panic() {
        let data = sample_mp4();
        for cut in 0..data.len() {
            let _ = parse(&mut Cursor::new(&data[..cut]), cut as u64);
        }
    }
}
