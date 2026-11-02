//! MP4 / fMP4 inspection without decoding: walks ISO-BMFF boxes and extracts
//! duration, tracks (codec, resolution) and fragmentation info.
//!
//! Regular MP4: duration from `mvhd` (fallback: longest `mdhd`).
//! Fragmented MP4 (MediaMTX segments): `mvhd.duration == 0`, so the duration is
//! the sum of `trun` sample durations per track divided by that track's `mdhd`
//! timescale — the longest track wins.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::Serialize;

/// Boxes we load fully into memory (`moov`, `moof`) are capped to this size.
const MAX_META_BOX: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TrackInfo {
    pub id: u32,
    /// "video", "audio" or the raw handler fourcc.
    pub kind: String,
    /// Sample entry fourcc: avc1, hvc1, hev1, mp4a, ...
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub timescale: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Mp4Info {
    pub duration_sec: f64,
    pub fragmented: bool,
    pub fragments: u32,
    pub major_brand: Option<String>,
    pub tracks: Vec<TrackInfo>,
}

impl Mp4Info {
    pub fn video(&self) -> Option<&TrackInfo> {
        self.tracks.iter().find(|t| t.kind == "video")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mp4Error {
    /// First box is not a plausible ISO-BMFF box.
    NotMp4,
    /// No `moov` box (file cut before the header was written, or not an MP4).
    MoovMissing,
    /// A box extends past end of file.
    Truncated,
    /// Header parsed but no duration could be derived.
    DurationZero,
    /// Structurally invalid box.
    BoxInvalid(String),
    Io(String),
}

impl Mp4Error {
    /// Stable short code used in stats / API.
    pub fn code(&self) -> &'static str {
        match self {
            Mp4Error::NotMp4 => "not_mp4",
            Mp4Error::MoovMissing => "moov_missing",
            Mp4Error::Truncated => "truncated",
            Mp4Error::DurationZero => "duration_zero",
            Mp4Error::BoxInvalid(_) => "box_invalid",
            Mp4Error::Io(_) => "io_error",
        }
    }
}

impl std::fmt::Display for Mp4Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mp4Error::BoxInvalid(m) => write!(f, "box_invalid: {m}"),
            Mp4Error::Io(m) => write!(f, "io_error: {m}"),
            other => f.write_str(other.code()),
        }
    }
}

impl std::error::Error for Mp4Error {}

pub fn parse_file(path: &Path) -> Result<Mp4Info, Mp4Error> {
    let mut f = File::open(path).map_err(|e| Mp4Error::Io(e.to_string()))?;
    parse(&mut f)
}

/// Top-level box header as found in the stream.
struct BoxHeader {
    kind: [u8; 4],
    /// Absolute offset of the payload.
    payload_at: u64,
    payload_len: u64,
}

fn read_top_header<R: Read + Seek>(r: &mut R, at: u64, file_len: u64) -> Result<Option<BoxHeader>, Mp4Error> {
    if at + 8 > file_len {
        return Ok(None);
    }
    r.seek(SeekFrom::Start(at)).map_err(io)?;
    let mut h = [0u8; 8];
    r.read_exact(&mut h).map_err(io)?;
    let size32 = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
    let kind = [h[4], h[5], h[6], h[7]];
    let (header_len, size) = match size32 {
        1 => {
            if at + 16 > file_len {
                return Err(Mp4Error::Truncated);
            }
            let mut l = [0u8; 8];
            r.read_exact(&mut l).map_err(io)?;
            (16, u64::from_be_bytes(l))
        }
        0 => (8, file_len - at),
        s => (8, s),
    };
    if size < header_len {
        return Err(Mp4Error::BoxInvalid(format!("{} size {size}", fourcc(&kind))));
    }
    Ok(Some(BoxHeader { kind, payload_at: at + header_len, payload_len: size - header_len }))
}

fn io(e: std::io::Error) -> Mp4Error {
    if e.kind() == std::io::ErrorKind::UnexpectedEof {
        Mp4Error::Truncated
    } else {
        Mp4Error::Io(e.to_string())
    }
}

fn fourcc(k: &[u8]) -> String {
    k.iter().map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '?' }).collect()
}

fn plausible_fourcc(k: &[u8; 4]) -> bool {
    k.iter().all(|b| b.is_ascii_alphanumeric() || *b == b' ' || *b == b'-' || *b == b'_' || *b == 0xa9)
}

fn read_payload<R: Read + Seek>(r: &mut R, h: &BoxHeader) -> Result<Vec<u8>, Mp4Error> {
    if h.payload_len > MAX_META_BOX {
        return Err(Mp4Error::BoxInvalid(format!("{} too large ({} bytes)", fourcc(&h.kind), h.payload_len)));
    }
    r.seek(SeekFrom::Start(h.payload_at)).map_err(io)?;
    let mut buf = vec![0u8; h.payload_len as usize];
    r.read_exact(&mut buf).map_err(io)?;
    Ok(buf)
}

pub fn parse<R: Read + Seek>(r: &mut R) -> Result<Mp4Info, Mp4Error> {
    let file_len = r.seek(SeekFrom::End(0)).map_err(io)?;
    if file_len < 8 {
        return Err(Mp4Error::NotMp4);
    }

    let mut major_brand = None;
    let mut moov: Option<Moov> = None;
    let mut track_ticks: HashMap<u32, u64> = HashMap::new();
    let mut fragments = 0u32;
    let mut truncated = false;

    let mut at = 0u64;
    let mut first = true;
    while let Some(h) = read_top_header(r, at, file_len)? {
        if first {
            if !plausible_fourcc(&h.kind) {
                return Err(Mp4Error::NotMp4);
            }
            first = false;
        }
        let end = h.payload_at + h.payload_len;
        let overflows = end > file_len;
        match &h.kind {
            b"ftyp" if !overflows && h.payload_len >= 4 => {
                let p = read_payload(r, &h)?;
                major_brand = Some(fourcc(&p[0..4]).trim().to_string());
            }
            b"moov" => {
                if overflows {
                    return Err(Mp4Error::Truncated);
                }
                let p = read_payload(r, &h)?;
                moov = Some(parse_moov(&p)?);
            }
            b"moof" if !overflows => {
                let p = read_payload(r, &h)?;
                let defaults: HashMap<u32, u32> = moov
                    .as_ref()
                    .map(|m| m.trex_default_duration.clone())
                    .unwrap_or_default();
                accumulate_moof(&p, &defaults, &mut track_ticks)?;
                fragments += 1;
            }
            _ => {}
        }
        if overflows {
            // A trailing mdat cut short is common for an interrupted recording:
            // keep what was parsed so far but remember the file is incomplete.
            truncated = true;
            break;
        }
        at = end;
    }

    let moov = match moov {
        Some(m) => m,
        None => {
            return Err(if truncated { Mp4Error::Truncated } else { Mp4Error::MoovMissing });
        }
    };

    let fragmented = moov.has_mvex || fragments > 0;
    let mut duration = 0.0f64;
    if moov.mvhd_timescale > 0 && moov.mvhd_duration > 0 {
        duration = moov.mvhd_duration as f64 / moov.mvhd_timescale as f64;
    }
    if duration <= 0.0 && !track_ticks.is_empty() {
        for t in &moov.tracks {
            if let Some(ticks) = track_ticks.get(&t.info.id) {
                if t.info.timescale > 0 {
                    duration = duration.max(*ticks as f64 / t.info.timescale as f64);
                }
            }
        }
    }
    if duration <= 0.0 {
        for t in &moov.tracks {
            if t.info.timescale > 0 && t.mdhd_duration > 0 {
                duration = duration.max(t.mdhd_duration as f64 / t.info.timescale as f64);
            }
        }
    }
    if duration <= 0.0 {
        return Err(if truncated { Mp4Error::Truncated } else { Mp4Error::DurationZero });
    }

    Ok(Mp4Info {
        duration_sec: duration,
        fragmented,
        fragments,
        major_brand,
        tracks: moov.tracks.into_iter().map(|t| t.info).collect(),
    })
}

struct Track {
    info: TrackInfo,
    mdhd_duration: u64,
}

struct Moov {
    mvhd_timescale: u32,
    mvhd_duration: u64,
    has_mvex: bool,
    trex_default_duration: HashMap<u32, u32>,
    tracks: Vec<Track>,
}

/// In-memory child box iterator over a payload slice.
struct Boxes<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Boxes<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Boxes { buf, pos: 0 }
    }
}

impl<'a> Iterator for Boxes<'a> {
    type Item = Result<([u8; 4], &'a [u8]), Mp4Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let rest = &self.buf[self.pos..];
        if rest.len() < 8 {
            return None;
        }
        let size32 = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
        let kind = [rest[4], rest[5], rest[6], rest[7]];
        let (hl, size) = match size32 {
            1 => {
                if rest.len() < 16 {
                    return Some(Err(Mp4Error::Truncated));
                }
                let s = u64::from_be_bytes(rest[8..16].try_into().unwrap());
                (16usize, usize::try_from(s).unwrap_or(usize::MAX))
            }
            0 => (8, rest.len()),
            s => (8, s),
        };
        if size < hl || size > rest.len() {
            self.pos = self.buf.len();
            return Some(Err(Mp4Error::BoxInvalid(format!("{} size {size}", fourcc(&kind)))));
        }
        self.pos += size;
        Some(Ok((kind, &rest[hl..size])))
    }
}

struct Cur<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Cur<'a> {
    fn new(b: &'a [u8]) -> Self {
        Cur { b, p: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], Mp4Error> {
        if self.p + n > self.b.len() {
            return Err(Mp4Error::Truncated);
        }
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, Mp4Error> {
        Ok(self.take(1)?[0])
    }
    fn u24(&mut self) -> Result<u32, Mp4Error> {
        let s = self.take(3)?;
        Ok(u32::from_be_bytes([0, s[0], s[1], s[2]]))
    }
    fn u32(&mut self) -> Result<u32, Mp4Error> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, Mp4Error> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn skip(&mut self, n: usize) -> Result<(), Mp4Error> {
        self.take(n).map(|_| ())
    }
}

fn parse_moov(p: &[u8]) -> Result<Moov, Mp4Error> {
    let mut m = Moov {
        mvhd_timescale: 0,
        mvhd_duration: 0,
        has_mvex: false,
        trex_default_duration: HashMap::new(),
        tracks: Vec::new(),
    };
    for b in Boxes::new(p) {
        let (kind, body) = b?;
        match &kind {
            b"mvhd" => {
                let mut c = Cur::new(body);
                let version = c.u8()?;
                c.skip(3)?;
                if version == 1 {
                    c.skip(16)?;
                    m.mvhd_timescale = c.u32()?;
                    m.mvhd_duration = c.u64()?;
                } else {
                    c.skip(8)?;
                    m.mvhd_timescale = c.u32()?;
                    let d = c.u32()?;
                    // 0xFFFFFFFF means "unknown" in version 0.
                    m.mvhd_duration = if d == u32::MAX { 0 } else { d as u64 };
                }
            }
            b"trak" => {
                if let Some(t) = parse_trak(body)? {
                    m.tracks.push(t);
                }
            }
            b"mvex" => {
                m.has_mvex = true;
                for b in Boxes::new(body) {
                    let (k, tb) = b?;
                    if &k == b"trex" {
                        let mut c = Cur::new(tb);
                        c.skip(4)?;
                        let track_id = c.u32()?;
                        c.skip(4)?; // default_sample_description_index
                        let dur = c.u32()?;
                        m.trex_default_duration.insert(track_id, dur);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(m)
}

fn parse_trak(p: &[u8]) -> Result<Option<Track>, Mp4Error> {
    let mut id = 0u32;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut timescale = 0u32;
    let mut mdhd_duration = 0u64;
    let mut handler = String::new();
    let mut codec = String::new();
    for b in Boxes::new(p) {
        let (kind, body) = b?;
        match &kind {
            b"tkhd" => {
                let mut c = Cur::new(body);
                let version = c.u8()?;
                c.skip(3)?;
                if version == 1 {
                    c.skip(16)?;
                    id = c.u32()?;
                    c.skip(4 + 8)?;
                } else {
                    c.skip(8)?;
                    id = c.u32()?;
                    c.skip(4 + 4)?;
                }
                // reserved(8) layer(2) alt_group(2) volume(2) reserved(2) matrix(36)
                c.skip(8 + 2 + 2 + 2 + 2 + 36)?;
                width = c.u32()? >> 16;
                height = c.u32()? >> 16;
            }
            b"mdia" => {
                for b in Boxes::new(body) {
                    let (k, mb) = b?;
                    match &k {
                        b"mdhd" => {
                            let mut c = Cur::new(mb);
                            let version = c.u8()?;
                            c.skip(3)?;
                            if version == 1 {
                                c.skip(16)?;
                                timescale = c.u32()?;
                                mdhd_duration = c.u64()?;
                            } else {
                                c.skip(8)?;
                                timescale = c.u32()?;
                                let d = c.u32()?;
                                mdhd_duration = if d == u32::MAX { 0 } else { d as u64 };
                            }
                        }
                        b"hdlr" => {
                            let mut c = Cur::new(mb);
                            c.skip(4 + 4)?;
                            handler = fourcc(c.take(4)?);
                        }
                        b"minf" => {
                            codec = find_codec(mb)?.unwrap_or_default();
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if id == 0 {
        return Ok(None);
    }
    let kind = match handler.as_str() {
        "vide" => "video".to_string(),
        "soun" => "audio".to_string(),
        other => other.to_string(),
    };
    Ok(Some(Track {
        info: TrackInfo { id, kind, codec, width, height, timescale },
        mdhd_duration,
    }))
}

fn find_codec(minf: &[u8]) -> Result<Option<String>, Mp4Error> {
    for b in Boxes::new(minf) {
        let (k, body) = b?;
        if &k != b"stbl" {
            continue;
        }
        for b in Boxes::new(body) {
            let (k, sb) = b?;
            if &k != b"stsd" {
                continue;
            }
            let mut c = Cur::new(sb);
            c.skip(4)?;
            let count = c.u32()?;
            if count == 0 {
                return Ok(None);
            }
            let entry = c.take(8)?;
            return Ok(Some(fourcc(&entry[4..8])));
        }
    }
    Ok(None)
}

fn accumulate_moof(p: &[u8], trex: &HashMap<u32, u32>, ticks: &mut HashMap<u32, u64>) -> Result<(), Mp4Error> {
    for b in Boxes::new(p) {
        let (kind, body) = b?;
        if &kind != b"traf" {
            continue;
        }
        let mut track_id = None;
        let mut default_duration = None;
        for b in Boxes::new(body) {
            let (k, tb) = b?;
            match &k {
                b"tfhd" => {
                    let mut c = Cur::new(tb);
                    c.u8()?;
                    let flags = c.u24()?;
                    let tid = c.u32()?;
                    if flags & 0x01 != 0 {
                        c.skip(8)?; // base_data_offset
                    }
                    if flags & 0x02 != 0 {
                        c.skip(4)?; // sample_description_index
                    }
                    default_duration = if flags & 0x08 != 0 { Some(c.u32()?) } else { trex.get(&tid).copied() };
                    track_id = Some(tid);
                }
                b"trun" => {
                    let Some(tid) = track_id else { continue };
                    let mut c = Cur::new(tb);
                    c.u8()?;
                    let flags = c.u24()?;
                    let count = c.u32()?;
                    if flags & 0x001 != 0 {
                        c.skip(4)?;
                    }
                    if flags & 0x004 != 0 {
                        c.skip(4)?;
                    }
                    let mut total = 0u64;
                    for _ in 0..count {
                        if flags & 0x100 != 0 {
                            total += c.u32()? as u64;
                        } else if let Some(d) = default_duration {
                            total += d as u64;
                        }
                        if flags & 0x200 != 0 {
                            c.skip(4)?;
                        }
                        if flags & 0x400 != 0 {
                            c.skip(4)?;
                        }
                        if flags & 0x800 != 0 {
                            c.skip(4)?;
                        }
                    }
                    *ticks.entry(tid).or_insert(0) += total;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub mod testgen {
    //! Minimal synthetic MP4 writer for tests.

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(body.len() + 8);
        v.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    fn full(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
        let mut b = vec![version];
        b.extend_from_slice(&flags.to_be_bytes()[1..]);
        b.extend_from_slice(body);
        bx(kind, &b)
    }

    fn tkhd(id: u32, w: u32, h: u32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&[0u8; 8]);
        b.extend_from_slice(&id.to_be_bytes());
        b.extend_from_slice(&[0u8; 4 + 4]);
        b.extend_from_slice(&[0u8; 8 + 2 + 2 + 2 + 2 + 36]);
        b.extend_from_slice(&(w << 16).to_be_bytes());
        b.extend_from_slice(&(h << 16).to_be_bytes());
        full(b"tkhd", 0, 3, &b)
    }

    fn trak(id: u32, handler: &[u8; 4], codec: &[u8; 4], ts: u32, dur: u32, w: u32, h: u32) -> Vec<u8> {
        let mut mdhd = Vec::new();
        mdhd.extend_from_slice(&[0u8; 8]);
        mdhd.extend_from_slice(&ts.to_be_bytes());
        mdhd.extend_from_slice(&dur.to_be_bytes());
        mdhd.extend_from_slice(&[0u8; 4]);
        let mut hdlr = vec![0u8; 4];
        hdlr.extend_from_slice(handler);
        hdlr.extend_from_slice(&[0u8; 12 + 1]);
        let mut stsd = 1u32.to_be_bytes().to_vec();
        stsd.extend_from_slice(&bx(codec, &[0u8; 8]));
        let stbl = bx(b"stbl", &full(b"stsd", 0, 0, &stsd));
        let minf = bx(b"minf", &stbl);
        let mut mdia = full(b"mdhd", 0, 0, &mdhd);
        mdia.extend(full(b"hdlr", 0, 0, &hdlr));
        mdia.extend(minf);
        let mut t = tkhd(id, w, h);
        t.extend(bx(b"mdia", &mdia));
        bx(b"trak", &t)
    }

    fn mvhd(ts: u32, dur: u32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&[0u8; 8]);
        b.extend_from_slice(&ts.to_be_bytes());
        b.extend_from_slice(&dur.to_be_bytes());
        b.extend_from_slice(&[0u8; 80]);
        full(b"mvhd", 0, 0, &b)
    }

    fn ftyp() -> Vec<u8> {
        bx(b"ftyp", b"isom\0\0\x02\0isomiso2avc1mp41")
    }

    /// Progressive MP4 with `secs` seconds of h264 1920x1080 (mvhd timescale 1000).
    pub fn progressive(secs: f64, mdat_len: usize) -> Vec<u8> {
        let mut moov = mvhd(1000, (secs * 1000.0) as u32);
        moov.extend(trak(1, b"vide", b"avc1", 90000, (secs * 90000.0) as u32, 1920, 1080));
        let mut v = ftyp();
        v.extend(bx(b"moov", &moov));
        v.extend(bx(b"mdat", &vec![0xAB; mdat_len]));
        v
    }

    /// MediaMTX-style fMP4: mvhd duration 0, `frags` fragments of `samples`
    /// samples of `sample_ticks` each on a 90 kHz h265 track.
    pub fn fragmented(frags: u32, samples: u32, sample_ticks: u32, explicit_durations: bool) -> Vec<u8> {
        let mut moov = mvhd(1000, 0);
        moov.extend(trak(1, b"vide", b"hvc1", 90000, 0, 2560, 1440));
        moov.extend(trak(2, b"soun", b"mp4a", 48000, 0, 0, 0));
        let mut trex = Vec::new();
        trex.extend_from_slice(&1u32.to_be_bytes());
        trex.extend_from_slice(&1u32.to_be_bytes());
        trex.extend_from_slice(&sample_ticks.to_be_bytes());
        trex.extend_from_slice(&[0u8; 8]);
        moov.extend(bx(b"mvex", &full(b"trex", 0, 0, &trex)));
        let mut v = ftyp();
        v.extend(bx(b"moov", &moov));
        for seq in 0..frags {
            let mut mfhd = Vec::new();
            mfhd.extend_from_slice(&(seq + 1).to_be_bytes());
            let tfhd = full(b"tfhd", 0, 0x020000, &1u32.to_be_bytes());
            let mut trun = Vec::new();
            trun.extend_from_slice(&samples.to_be_bytes());
            let flags = if explicit_durations {
                for _ in 0..samples {
                    trun.extend_from_slice(&sample_ticks.to_be_bytes());
                    trun.extend_from_slice(&100u32.to_be_bytes());
                }
                0x300
            } else {
                for _ in 0..samples {
                    trun.extend_from_slice(&100u32.to_be_bytes());
                }
                0x200
            };
            let mut traf = tfhd;
            traf.extend(full(b"trun", 0, flags, &trun));
            let mut moof = full(b"mfhd", 0, 0, &mfhd);
            moof.extend(bx(b"traf", &traf));
            v.extend(bx(b"moof", &moof));
            v.extend(bx(b"mdat", &vec![0u8; samples as usize * 100]));
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn progressive_duration_and_tracks() {
        let data = testgen::progressive(61.5, 4096);
        let info = parse(&mut Cursor::new(data)).unwrap();
        assert!((info.duration_sec - 61.5).abs() < 1e-6);
        assert!(!info.fragmented);
        assert_eq!(info.major_brand.as_deref(), Some("isom"));
        let v = info.video().unwrap();
        assert_eq!((v.codec.as_str(), v.width, v.height), ("avc1", 1920, 1080));
    }

    #[test]
    fn fragmented_sums_trun_explicit() {
        // 10 fragments * 30 samples * 3000 ticks / 90000 = 10s
        let data = testgen::fragmented(10, 30, 3000, true);
        let info = parse(&mut Cursor::new(data)).unwrap();
        assert!(info.fragmented);
        assert_eq!(info.fragments, 10);
        assert!((info.duration_sec - 10.0).abs() < 1e-6, "{}", info.duration_sec);
        assert_eq!(info.video().unwrap().codec, "hvc1");
        assert_eq!(info.tracks.len(), 2);
    }

    #[test]
    fn fragmented_uses_trex_default_duration() {
        let data = testgen::fragmented(4, 25, 3600, false);
        let info = parse(&mut Cursor::new(data)).unwrap();
        assert!((info.duration_sec - 4.0).abs() < 1e-6, "{}", info.duration_sec);
    }

    #[test]
    fn truncated_mdat_keeps_parsed_fragments() {
        let mut data = testgen::fragmented(3, 30, 3000, true);
        data.truncate(data.len() - 50);
        let info = parse(&mut Cursor::new(data)).unwrap();
        // last fragment's moof is intact, its mdat is cut: duration still counts it
        assert!((info.duration_sec - 3.0).abs() < 1e-6);
    }

    #[test]
    fn cut_inside_moov_is_truncated() {
        let data = testgen::progressive(5.0, 10);
        let cut = data[..60].to_vec();
        assert_eq!(parse(&mut Cursor::new(cut)).unwrap_err(), Mp4Error::Truncated);
    }

    #[test]
    fn garbage_is_not_mp4() {
        let data = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDRxxxxxxxxxxxx".to_vec();
        assert_eq!(parse(&mut Cursor::new(data)).unwrap_err(), Mp4Error::NotMp4);
        assert_eq!(parse(&mut Cursor::new(vec![0u8; 3])).unwrap_err(), Mp4Error::NotMp4);
    }

    #[test]
    fn no_moov_is_reported() {
        let mut v = Vec::new();
        v.extend_from_slice(&16u32.to_be_bytes());
        v.extend_from_slice(b"ftypisom\0\0\0\0");
        v.extend_from_slice(&16u32.to_be_bytes());
        v.extend_from_slice(b"mdat01234567");
        assert_eq!(parse(&mut Cursor::new(v)).unwrap_err(), Mp4Error::MoovMissing);
    }

    #[test]
    fn zero_duration_reported() {
        let data = testgen::fragmented(0, 0, 0, true);
        assert_eq!(parse(&mut Cursor::new(data)).unwrap_err(), Mp4Error::DurationZero);
    }

    #[test]
    fn largesize_box_header() {
        // 64-bit size header on mdat
        let mut v = testgen::progressive(2.0, 0);
        let mdat_at = v.len() - 8;
        v.truncate(mdat_at);
        v.extend_from_slice(&1u32.to_be_bytes());
        v.extend_from_slice(b"mdat");
        v.extend_from_slice(&(16u64 + 32).to_be_bytes());
        v.extend_from_slice(&[0u8; 32]);
        let info = parse(&mut Cursor::new(v)).unwrap();
        assert!((info.duration_sec - 2.0).abs() < 1e-6);
    }
}
