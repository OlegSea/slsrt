//! Codec identification tables and parsers for codec configuration records. MP4 and Matroska
//! store the same records (`avcC`, `hvcC`, `av1C`, AAC AudioSpecificConfig), so both share these.

use super::model::Track;
use super::reader::{Bits, Bytes, unescape_rbsp};

/// ISO BMFF / QuickTime sample entry type.
pub fn mp4_codec(tag: &[u8; 4]) -> Option<&'static str> {
    Some(match tag {
        b"avc1" | b"avc2" | b"avc3" | b"avc4" => "h264",
        b"hvc1" | b"hev1" | b"dvh1" | b"dvhe" => "hevc",
        b"av01" => "av1",
        b"vp08" => "vp8",
        b"vp09" => "vp9",
        b"mp4v" => "mpeg4",
        b"s263" | b"h263" => "h263",
        b"apcn" | b"apch" | b"apcs" | b"apco" | b"ap4h" | b"ap4x" => "prores",
        b"jpeg" | b"mjpa" | b"mjpb" => "mjpeg",
        b"mp4a" => "aac",
        b"Opus" => "opus",
        b"fLaC" => "flac",
        b"ac-3" => "ac3",
        b"ec-3" => "eac3",
        b"alac" => "alac",
        b".mp3" => "mp3",
        b"samr" => "amr_nb",
        b"sawb" => "amr_wb",
        b"sowt" | b"twos" | b"in24" | b"in32" | b"fl32" | b"fl64" | b"lpcm" | b"ipcm" | b"raw " => "pcm",
        b"tx3g" | b"text" => "mov_text",
        b"wvtt" => "webvtt",
        b"stpp" => "ttml",
        b"c608" => "eia_608",
        b"c708" => "eia_708",
        b"mp4s" => "dvd_subtitle",
        _ => return None,
    })
}

/// MPEG-4 Systems objectTypeIndication from an `esds` box; refines `mp4a`/`mp4v`.
pub fn mp4_object_type(oti: u8) -> Option<&'static str> {
    Some(match oti {
        0x20 => "mpeg4",
        0x21 => "h264",
        0x23 => "hevc",
        0x40 | 0x66..=0x68 => "aac",
        0x60..=0x65 => "mpeg2video",
        0x6A => "mpeg1video",
        0x69 | 0x6B => "mp3",
        0x6C => "mjpeg",
        0xA5 => "ac3",
        0xA6 => "eac3",
        0xA9..=0xAC => "dts",
        0xAD => "opus",
        0xE0 => "dvd_subtitle",
        _ => return None,
    })
}

/// Matroska CodecID.
pub fn mkv_codec(id: &str) -> Option<&'static str> {
    Some(match id {
        "V_MPEG4/ISO/AVC" => "h264",
        "V_MPEGH/ISO/HEVC" => "hevc",
        "V_AV1" => "av1",
        "V_VP8" => "vp8",
        "V_VP9" => "vp9",
        "V_THEORA" => "theora",
        "V_MPEG1" => "mpeg1video",
        "V_MPEG2" => "mpeg2video",
        "V_MJPEG" => "mjpeg",
        "V_PRORES" => "prores",
        "V_FFV1" => "ffv1",
        "A_OPUS" => "opus",
        "A_VORBIS" => "vorbis",
        "A_FLAC" => "flac",
        "A_AC3" => "ac3",
        "A_EAC3" => "eac3",
        "A_TRUEHD" => "truehd",
        "A_ALAC" => "alac",
        "A_MPEG/L3" => "mp3",
        "A_MPEG/L2" => "mp2",
        "S_TEXT/UTF8" => "subrip",
        "S_TEXT/SSA" => "ssa",
        "S_TEXT/ASS" => "ass",
        "S_TEXT/WEBVTT" | "D_WEBVTT/SUBTITLES" | "D_WEBVTT/CAPTIONS" => "webvtt",
        "S_TEXT/USF" => "usf",
        "S_HDMV/PGS" => "pgs",
        "S_HDMV/TEXTST" => "hdmv_text",
        "S_VOBSUB" => "dvd_subtitle",
        "S_DVBSUB" => "dvb_subtitle",
        "S_KATE" => "kate",
        _ if id.starts_with("V_MPEG4/ISO/") => "mpeg4",
        _ if id.starts_with("A_AAC") => "aac",
        _ if id.starts_with("A_DTS") => "dts",
        _ if id.starts_with("A_PCM/") => "pcm",
        _ => return None,
    })
}

/// Video for Windows fourcc, as found in AVI and Matroska `V_MS/VFW/FOURCC`.
pub fn vfw_codec(tag: &[u8; 4]) -> Option<&'static str> {
    let mut upper = *tag;
    upper.make_ascii_uppercase();
    Some(match &upper {
        b"H264" | b"X264" | b"AVC1" | b"DAVC" => "h264",
        b"HEVC" | b"H265" | b"X265" | b"HVC1" | b"HEV1" => "hevc",
        b"XVID" | b"DIVX" | b"DX50" | b"FMP4" | b"MP4V" | b"3IV2" => "mpeg4",
        b"DIV3" | b"MP43" => "msmpeg4v3",
        b"MP42" => "msmpeg4v2",
        b"MJPG" | b"AVRN" => "mjpeg",
        b"AV01" => "av1",
        b"VP80" => "vp8",
        b"VP90" => "vp9",
        b"MPG1" => "mpeg1video",
        b"MPG2" => "mpeg2video",
        b"WMV1" => "wmv1",
        b"WMV2" => "wmv2",
        b"WMV3" => "wmv3",
        b"WVC1" => "vc1",
        b"FFV1" => "ffv1",
        b"HFYU" => "huffyuv",
        b"CVID" => "cinepak",
        b"MSVC" | b"CRAM" => "msvideo1",
        [0, 0, 0, 0] | b"DIB " => "rawvideo",
        _ => return None,
    })
}

/// WAVEFORMATEX `wFormatTag`.
pub fn wave_codec(tag: u16) -> Option<&'static str> {
    Some(match tag {
        0x0001 | 0x0003 | 0xFFFE => "pcm",
        0x0002 => "adpcm_ms",
        0x0006 => "pcm_alaw",
        0x0007 => "pcm_mulaw",
        0x0011 => "adpcm_ima_wav",
        0x0050 => "mp2",
        0x0055 => "mp3",
        0x00FF | 0x1600 | 0x1601 | 0x706D => "aac",
        0x0161 => "wmav2",
        0x0162 => "wmapro",
        0x2000 => "ac3",
        0x2001 => "dts",
        0xF1AC => "flac",
        _ => return None,
    })
}

/// Parsed `esds` (MPEG-4 ES_Descriptor) box body.
pub struct Esds<'a> {
    pub object_type: u8,
    pub avg_bitrate: u32,
    pub specific: &'a [u8],
}

pub fn parse_esds(body: &[u8]) -> Option<Esds<'_>> {
    // Descriptor lengths use 7 bits per byte with a continuation bit.
    fn descriptor<'a>(b: &mut Bytes<'a>, want: u8) -> Option<Bytes<'a>> {
        loop {
            let tag = b.u8()?;
            let mut len = 0usize;
            for _ in 0..4 {
                let byte = b.u8()?;
                len = (len << 7) | (byte & 0x7F) as usize;
                if byte & 0x80 == 0 {
                    break;
                }
            }
            let body = b.take(len.min(b.remaining()))?;
            if tag == want {
                return Some(Bytes::new(body));
            }
        }
    }

    let mut b = Bytes::new(body);
    b.skip(4)?;
    let mut es = descriptor(&mut b, 0x03)?;
    es.skip(2)?;
    let flags = es.u8()?;
    if flags & 0x80 != 0 {
        es.skip(2)?;
    }
    if flags & 0x40 != 0 {
        let url_len = es.u8()?;
        es.skip(url_len as usize)?;
    }
    if flags & 0x20 != 0 {
        es.skip(2)?;
    }
    let mut config = descriptor(&mut es, 0x04)?;
    let object_type = config.u8()?;
    config.skip(1 + 3 + 4)?;
    let avg_bitrate = config.u32be()?;
    let specific = descriptor(&mut config, 0x05).map(|d| d.rest()).unwrap_or_default();
    Some(Esds { object_type, avg_bitrate, specific })
}

fn chroma_name(format: u32) -> Option<String> {
    Some(match format {
        0 => "4:0:0",
        1 => "4:2:0",
        2 => "4:2:2",
        3 => "4:4:4",
        _ => return None,
    }
    .to_string())
}

/// `avcC`: profile and level from the header, bit depth and chroma format from the first SPS.
pub fn apply_avcc(track: &mut Track, data: &[u8]) -> Option<()> {
    let mut b = Bytes::new(data);
    b.skip(1)?;
    let profile = b.u8()?;
    let compat = b.u8()?;
    let level = b.u8()?;
    track.profile = Some(
        match profile {
            66 if compat & 0x40 != 0 => "Constrained Baseline",
            66 => "Baseline",
            77 => "Main",
            88 => "Extended",
            100 => "High",
            110 => "High 10",
            122 => "High 4:2:2",
            244 => "High 4:4:4 Predictive",
            44 => "CAVLC 4:4:4",
            _ => return None,
        }
        .to_string(),
    );
    track.level = Some(format!("{}.{}", level / 10, level % 10));
    track.bit_depth = Some(8);
    track.chroma = chroma_name(1);

    b.skip(1)?;
    let sps_count = b.u8()? & 0x1F;
    if sps_count == 0 {
        return Some(());
    }
    let len = b.u16be()? as usize;
    let sps = unescape_rbsp(b.take(len)?);
    // Only the High-family profiles carry chroma_format_idc and bit depth in the SPS.
    if !matches!(profile, 100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135) {
        return Some(());
    }
    let mut bits = Bits::new(sps.get(4..)?);
    bits.ue()?;
    let chroma = bits.ue()?;
    if chroma == 3 {
        bits.flag()?;
    }
    let depth = bits.ue()? + 8;
    track.chroma = chroma_name(chroma);
    track.bit_depth = Some(depth);
    Some(())
}

/// `hvcC`: everything needed sits at fixed offsets in the header.
pub fn apply_hvcc(track: &mut Track, data: &[u8]) -> Option<()> {
    let mut b = Bytes::new(data);
    b.skip(1)?;
    let profile = b.u8()? & 0x1F;
    b.skip(4 + 6)?;
    let level = b.u8()?;
    b.skip(2 + 1)?;
    let chroma = b.u8()? & 0x03;
    let depth = (b.u8()? & 0x07) as u32 + 8;
    track.profile = match profile {
        1 => Some("Main"),
        2 => Some("Main 10"),
        3 => Some("Main Still Picture"),
        4 => Some("Rext"),
        5 => Some("High Throughput"),
        9 => Some("Screen Content"),
        _ => None,
    }
    .map(str::to_string);
    // general_level_idc is 30 × the level number.
    let tenths = level as u32 / 3;
    track.level = Some(if tenths.is_multiple_of(10) {
        format!("{}", tenths / 10)
    } else {
        format!("{}.{}", tenths / 10, tenths % 10)
    });
    track.chroma = chroma_name(chroma as u32);
    track.bit_depth = Some(depth);
    Some(())
}

pub fn apply_av1c(track: &mut Track, data: &[u8]) -> Option<()> {
    let mut b = Bytes::new(data);
    b.skip(1)?;
    let byte1 = b.u8()?;
    let byte2 = b.u8()?;
    let seq_level = byte1 & 0x1F;
    track.profile = match byte1 >> 5 {
        0 => Some("Main"),
        1 => Some("High"),
        2 => Some("Professional"),
        _ => None,
    }
    .map(str::to_string);
    track.level = Some(format!("{}.{}", 2 + (seq_level >> 2), seq_level & 3));
    let high_bitdepth = byte2 & 0x40 != 0;
    let twelve_bit = byte2 & 0x20 != 0;
    track.bit_depth = Some(match (high_bitdepth, twelve_bit) {
        (true, true) => 12,
        (true, false) => 10,
        _ => 8,
    });
    let (mono, sub_x, sub_y) = (byte2 & 0x10 != 0, byte2 & 0x08 != 0, byte2 & 0x04 != 0);
    track.chroma = chroma_name(match (mono, sub_x, sub_y) {
        (true, ..) => 0,
        (false, true, true) => 1,
        (false, true, false) => 2,
        _ => 3,
    });
    Some(())
}

fn vp9_chroma(subsampling: u8) -> Option<String> {
    chroma_name(match subsampling {
        0 | 1 => 1,
        2 => 2,
        3 => 3,
        _ => return None,
    })
}

/// MP4 `vpcC` (a full box: version and flags precede the record).
pub fn apply_vpcc(track: &mut Track, data: &[u8]) -> Option<()> {
    let mut b = Bytes::new(data);
    b.skip(4)?;
    let profile = b.u8()?;
    let level = b.u8()?;
    let packed = b.u8()?;
    track.profile = Some(format!("Profile {profile}"));
    track.level = Some(format!("{}.{}", level / 10, level % 10));
    track.bit_depth = Some((packed >> 4) as u32);
    track.chroma = vp9_chroma((packed >> 1) & 0x07);
    let (primaries, transfer, matrix) = (b.u8()?, b.u8()?, b.u8()?);
    apply_color(track, primaries as u64, transfer as u64, matrix as u64);
    Some(())
}

/// Matroska VP9 CodecPrivate: a list of (id, length, value) features.
pub fn apply_mkv_vp9(track: &mut Track, data: &[u8]) -> Option<()> {
    let mut b = Bytes::new(data);
    while b.remaining() >= 2 {
        let id = b.u8()?;
        let len = b.u8()? as usize;
        let value = b.take(len)?;
        let Some(&v) = value.first() else { continue };
        match id {
            1 => track.profile = Some(format!("Profile {v}")),
            2 => track.level = Some(format!("{}.{}", v / 10, v % 10)),
            3 => track.bit_depth = Some(v as u32),
            4 => track.chroma = vp9_chroma(v),
            _ => {}
        }
    }
    Some(())
}

/// AAC AudioSpecificConfig: object type (profile), and channels when the container lacks them.
pub fn apply_aac(track: &mut Track, data: &[u8]) -> Option<()> {
    let mut bits = Bits::new(data);
    let mut object = bits.read(5)?;
    if object == 31 {
        object = 32 + bits.read(6)?;
    }
    track.profile = match object {
        1 => Some("Main"),
        2 => Some("LC"),
        3 => Some("SSR"),
        4 => Some("LTP"),
        5 => Some("HE-AAC"),
        23 => Some("LD"),
        29 => Some("HE-AACv2"),
        39 => Some("ELD"),
        42 => Some("xHE-AAC"),
        _ => None,
    }
    .map(str::to_string);
    if bits.read(4)? == 15 {
        bits.read(24)?;
    }
    let channels = match bits.read(4)? {
        c @ 1..=6 => c,
        7 => 8,
        _ => return Some(()),
    };
    track.channels.get_or_insert(channels);
    Some(())
}

/// Colour description code points (ISO/IEC 23091-2), named as ffmpeg names them.
/// "Unspecified" (2) and unknown values leave the field empty.
pub fn apply_color(track: &mut Track, primaries: u64, transfer: u64, matrix: u64) {
    let name = |s: &str| Some(s.to_string());
    track.color_primaries = match primaries {
        1 => name("bt709"),
        4 => name("bt470m"),
        5 => name("bt470bg"),
        6 => name("smpte170m"),
        7 => name("smpte240m"),
        9 => name("bt2020"),
        11 => name("smpte431"),
        12 => name("smpte432"),
        _ => None,
    }
    .or(track.color_primaries.take());
    track.color_transfer = match transfer {
        1 => name("bt709"),
        4 => name("gamma22"),
        5 => name("gamma28"),
        6 => name("smpte170m"),
        8 => name("linear"),
        13 => name("iec61966-2-1"),
        14 => name("bt2020-10"),
        15 => name("bt2020-12"),
        16 => name("smpte2084"),
        18 => name("arib-std-b67"),
        _ => None,
    }
    .or(track.color_transfer.take());
    track.color_matrix = match matrix {
        0 => name("gbr"),
        1 => name("bt709"),
        5 => name("bt470bg"),
        6 => name("smpte170m"),
        9 => name("bt2020nc"),
        10 => name("bt2020c"),
        _ => None,
    }
    .or(track.color_matrix.take());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avcc_high_10() {
        // avcC header for High 10 @ 5.1, then one SPS: profile 110, level 51, sps_id 0,
        // chroma_format_idc 1, bit_depth_luma_minus8 2  →  bits 1 | 010 | 011
        let sps = [0x67, 110, 0, 51, 0b1010_0110];
        let mut data = vec![1, 110, 0, 51, 0xFF, 0xE1, 0, sps.len() as u8];
        data.extend_from_slice(&sps);
        let mut t = Track::default();
        apply_avcc(&mut t, &data).unwrap();
        assert_eq!(t.profile.as_deref(), Some("High 10"));
        assert_eq!(t.level.as_deref(), Some("5.1"));
        assert_eq!(t.bit_depth, Some(10));
        assert_eq!(t.chroma.as_deref(), Some("4:2:0"));
    }

    #[test]
    fn aac_lc_stereo() {
        let mut t = Track::default();
        apply_aac(&mut t, &[0x12, 0x10]).unwrap();
        assert_eq!(t.profile.as_deref(), Some("LC"));
        assert_eq!(t.channels, Some(2));
    }

    #[test]
    fn av1c() {
        let mut t = Track::default();
        apply_av1c(&mut t, &[0x81, 0x08, 0x4C]).unwrap();
        assert_eq!(t.profile.as_deref(), Some("Main"));
        assert_eq!(t.level.as_deref(), Some("4.0"));
        assert_eq!(t.bit_depth, Some(10));
        assert_eq!(t.chroma.as_deref(), Some("4:2:0"));
    }

    #[test]
    fn garbage_does_not_panic() {
        let mut t = Track::default();
        for data in [&[][..], &[1], &[0xFF; 3], &[1, 100, 0, 40, 0xFF, 0xE1, 0xFF, 0xFF]] {
            apply_avcc(&mut t, data);
            apply_hvcc(&mut t, data);
            apply_av1c(&mut t, data);
            apply_vpcc(&mut t, data);
            apply_mkv_vp9(&mut t, data);
            apply_aac(&mut t, data);
            parse_esds(data);
        }
    }
}
