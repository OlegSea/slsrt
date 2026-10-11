//! Container-independent description of a media file, and its Lua representation.

use std::collections::BTreeMap;

use mlua::prelude::*;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Video,
    Audio,
    Subtitle,
    #[default]
    Other,
}

impl Kind {
    pub const NAMES: [&str; 4] = ["video", "audio", "subtitle", "other"];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Video => "video",
            Kind::Audio => "audio",
            Kind::Subtitle => "subtitle",
            Kind::Other => "other",
        }
    }
}

/// One stream. Fields that don't apply to the track's kind stay `None`.
#[derive(Debug, Default, Clone)]
pub struct Track {
    pub id: u64,
    pub kind: Kind,
    /// Normalised name (`h264`, `opus`, `ass`...), or the lowercased tag when unrecognised.
    pub codec: String,
    /// The container's own identifier: fourcc, Matroska CodecID or WAVE format tag.
    pub codec_tag: String,
    pub language: Option<String>,
    pub name: Option<String>,
    pub default: bool,
    pub forced: bool,
    pub duration: Option<f64>,
    pub bitrate: Option<u64>,

    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Display size: aspect ratio and rotation applied.
    pub display_width: Option<u32>,
    pub display_height: Option<u32>,
    pub rotation: Option<u32>,
    pub fps: Option<f64>,
    pub frame_count: Option<u64>,
    pub color_primaries: Option<String>,
    pub color_transfer: Option<String>,
    pub color_matrix: Option<String>,

    pub profile: Option<String>,
    pub level: Option<String>,
    pub bit_depth: Option<u32>,
    pub chroma: Option<String>,

    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
}

impl Track {
    pub fn new(kind: Kind) -> Self {
        Track { kind, ..Default::default() }
    }

    /// Set the codec from a lookup, falling back to the raw tag.
    pub fn set_codec(&mut self, tag: String, known: Option<&str>) {
        self.codec = known.map(str::to_string).unwrap_or_else(|| tag.trim().to_ascii_lowercase());
        self.codec_tag = tag;
    }

    /// Display size if known, else the coded size.
    pub fn resolution(&self) -> Option<(u32, u32)> {
        let w = self.display_width.or(self.width)?;
        let h = self.display_height.or(self.height)?;
        Some((w, h))
    }

    fn to_lua(&self, lua: &Lua) -> LuaResult<LuaTable> {
        let t = lua.create_table()?;
        t.set("id", self.id)?;
        t.set("kind", self.kind.name())?;
        t.set("codec", self.codec.as_str())?;
        t.set("codec_tag", self.codec_tag.as_str())?;
        t.set("language", self.language.as_deref())?;
        t.set("name", self.name.as_deref())?;
        t.set("default", self.default)?;
        t.set("forced", self.forced)?;
        t.set("duration", self.duration)?;
        t.set("bitrate", self.bitrate)?;
        t.set("width", self.width)?;
        t.set("height", self.height)?;
        t.set("display_width", self.display_width)?;
        t.set("display_height", self.display_height)?;
        t.set("rotation", self.rotation)?;
        t.set("fps", self.fps)?;
        t.set("frame_count", self.frame_count)?;
        t.set("color_primaries", self.color_primaries.as_deref())?;
        t.set("color_transfer", self.color_transfer.as_deref())?;
        t.set("color_matrix", self.color_matrix.as_deref())?;
        t.set("profile", self.profile.as_deref())?;
        t.set("level", self.level.as_deref())?;
        t.set("bit_depth", self.bit_depth)?;
        t.set("chroma", self.chroma.as_deref())?;
        t.set("sample_rate", self.sample_rate)?;
        t.set("channels", self.channels)?;
        Ok(t)
    }
}

#[derive(Debug, Default, Clone)]
pub struct MediaInfo {
    pub container: &'static str,
    pub duration: Option<f64>,
    pub bitrate: Option<u64>,
    pub tags: BTreeMap<String, String>,
    pub tracks: Vec<Track>,
}

impl MediaInfo {
    pub fn new(container: &'static str) -> Self {
        MediaInfo { container, ..Default::default() }
    }

    /// Add a tag under its normalised key. The first value for a key wins.
    pub fn add_tag(&mut self, key: &str, value: &str) {
        let value = value.trim_matches(|c: char| c == '\0' || c.is_whitespace());
        if !value.is_empty() {
            self.tags.entry(tag_key(key)).or_insert_with(|| value.to_string());
        }
    }

    pub fn first(&self, kind: Kind) -> Option<&Track> {
        self.tracks.iter().find(|t| t.kind == kind)
    }

    pub fn tracks_lua(&self, lua: &Lua, kind: Option<Kind>) -> LuaResult<LuaTable> {
        let list = lua.create_table()?;
        for track in self.tracks.iter().filter(|t| kind.is_none_or(|k| t.kind == k)) {
            list.push(track.to_lua(lua)?)?;
        }
        Ok(list)
    }

    pub fn to_lua(&self, lua: &Lua) -> LuaResult<LuaTable> {
        let t = lua.create_table()?;
        t.set("container", self.container)?;
        t.set("duration", self.duration)?;
        t.set("bitrate", self.bitrate)?;
        let tags = lua.create_table()?;
        for (k, v) in &self.tags {
            tags.set(k.as_str(), v.as_str())?;
        }
        t.set("tags", tags)?;
        t.set("tracks", self.tracks_lua(lua, None)?)?;
        Ok(t)
    }
}

/// Map container-specific tag names onto one vocabulary, so `f:tag("artist")` works for MP4
/// `©ART`, Matroska `ARTIST` and AVI `IART` alike.
pub fn tag_key(raw: &str) -> String {
    let key = raw.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    let mapped = match key.as_str() {
        "author" | "performer" | "lead_performer" => "artist",
        "album_artist" | "albumartist" => "album_artist",
        "year" | "date_released" | "date_recorded" | "released" => "date",
        "comments" => "comment",
        "writing_app" | "writingapp" | "software" | "encoding_tool" => "encoder",
        _ => return key,
    };
    mapped.to_string()
}

/// Round to millihertz: averages like 29.97002997 compare cleanly against 29.97.
pub fn round_fps(fps: f64) -> Option<f64> {
    (fps.is_finite() && fps > 0.0).then(|| (fps * 1000.0).round() / 1000.0)
}

/// `"und"` and empty codes carry no information.
pub fn language(code: &str) -> Option<String> {
    let code = code.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    (!code.is_empty() && !code.eq_ignore_ascii_case("und")).then(|| code.to_string())
}

/// Unix seconds as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn iso8601(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's days-to-civil algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso8601(1_760_000_000), "2025-10-09T08:53:20Z");
    }

    #[test]
    fn tag_keys() {
        assert_eq!(tag_key("ARTIST"), "artist");
        assert_eq!(tag_key("Author"), "artist");
        assert_eq!(tag_key("DATE_RELEASED"), "date");
        assert_eq!(tag_key("Some Key"), "some_key");
    }
}
