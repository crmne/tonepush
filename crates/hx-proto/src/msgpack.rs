//! A small MessagePack codec.
//!
//! We carry our own rather than pulling in a general one because Line 6 use the
//! format with one consistent deviation: strings are C strings whose declared
//! length *includes* the terminating NUL, and the same string types double as
//! opaque blobs holding nested MessagePack documents. A general decoder either
//! rejects those as invalid UTF-8 or silently mangles them, and both behaviours
//! destroy information we need.

use std::fmt;

/// A decoded MessagePack value.
///
/// Map keys are kept as `Key` because the protocol only ever uses integers and
/// strings, and integer keys are the norm - `{102: txn, 100: opcode}`.
#[derive(Clone, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    /// An unsigned integer, remembering how wide it was on the wire.
    ///
    /// Width matters when a document is written back: a preset carries a table
    /// of byte offsets into itself, so re-encoding `0xcc 05` as `0x05` shifts
    /// everything after it and the device reads the result as empty. In one
    /// captured preset, 91 of 103 wide tags would have shrunk.
    UInt(u64),
    /// An unsigned integer that must keep its original tag width.
    Wide(u64, u8),
    /// A signed integer that must keep its original tag width.
    WideInt(i64, u8),
    F32(f32),
    F64(f64),
    /// Text that decoded cleanly as UTF-8, with trailing NULs stripped.
    Str(String),
    /// A string or binary field that is not text - typically a nested document.
    ///
    /// The second field is the header width the device used (1, 2 or 4 bytes,
    /// or 0 for a fixstr), with [`BIN_FAMILY`] added when the field came in as
    /// MessagePack bin (`c4`-`c6`) rather than str. It is carried so a
    /// document can be written back unchanged: the encoder would otherwise
    /// pick the narrowest str tag that fits, and a preset's own offset table
    /// stops matching the moment its length changes.
    ///
    /// Text written with a wider header than it needs also decodes here
    /// rather than as `Str`, since `Str` always encodes with the narrowest
    /// header; [`Value::as_str`] still reads it as text.
    Bin(Vec<u8>, u8),
    Array(Vec<Value>),
    /// Insertion-ordered: the protocol's own key order is reproduced exactly on
    /// re-encode, which matters when replaying captured traffic.
    Map(Vec<(Key, Value)>),
}

/// Added to [`Value::Bin`]'s width for a field that arrived as MessagePack
/// bin (`c4`, `c5`, `c6`) rather than as a str.
pub const BIN_FAMILY: u8 = 0x80;

#[derive(Clone)]
pub enum Key {
    Int(i64),
    Str(String),
    /// A key the wire spelled wider than it needed to (`cc 05` for 5, or a
    /// short string behind a `d9` header), kept as the value it decoded to so
    /// the map re-encodes byte for byte. It is the same key as the plain
    /// `Int` or `Str` it names: it compares equal to it and is found by
    /// [`Value::get`].
    Wide(Box<Value>),
}

/// What a key names, whatever width it was written with.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum PlainKey<'a> {
    Int(i64),
    Str(&'a str),
}

impl Key {
    fn plain(&self) -> PlainKey<'_> {
        match self {
            Key::Int(i) => PlainKey::Int(*i),
            Key::Str(s) => PlainKey::Str(s),
            Key::Wide(value) => match value.as_i64() {
                Some(i) => PlainKey::Int(i),
                // The decoder only builds a wide key from an integer or text.
                None => PlainKey::Str(value.as_str().unwrap_or_default()),
            },
        }
    }

    /// The integer this key names, if it is one.
    pub fn as_i64(&self) -> Option<i64> {
        match self.plain() {
            PlainKey::Int(i) => Some(i),
            PlainKey::Str(_) => None,
        }
    }
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.plain() == other.plain()
    }
}

impl Eq for Key {}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Key {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.plain().cmp(&other.plain())
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.plain() {
            PlainKey::Int(i) => write!(f, "{i}"),
            PlainKey::Str(s) => write!(f, "{s:?}"),
        }
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Nil => write!(f, "nil"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(i) => write!(f, "{i}"),
            Value::UInt(u) => write!(f, "{u}"),
            Value::Wide(u, _) => write!(f, "{u}"),
            Value::WideInt(i, _) => write!(f, "{i}"),
            Value::F32(v) => write!(f, "{v}"),
            Value::F64(v) => write!(f, "{v}"),
            Value::Str(s) => write!(f, "{s:?}"),
            Value::Bin(b, _) => write!(f, "<{} bytes>", b.len()),
            Value::Array(a) => f.debug_list().entries(a).finish(),
            Value::Map(m) => f
                .debug_map()
                .entries(m.iter().map(|(k, v)| (k, v)))
                .finish(),
        }
    }
}

impl Value {
    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Value::Bool(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Value::Int(i) | Value::WideInt(i, _) => Some(i),
            Value::UInt(u) | Value::Wide(u, _) => i64::try_from(u).ok(),
            _ => None,
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        match *self {
            Value::F32(v) => Some(v),
            Value::F64(v) => Some(v as f32),
            Value::Int(i) | Value::WideInt(i, _) => Some(i as f32),
            Value::UInt(u) | Value::Wide(u, _) => Some(u as f32),
            _ => None,
        }
    }

    /// Text, including text that arrived behind a wider str header than it
    /// needed and so is held as [`Value::Bin`] to re-encode exactly.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            Value::Bin(raw, width) if width & BIN_FAMILY == 0 => c_string(raw),
            _ => None,
        }
    }

    /// Bytes of a string-or-blob field, whichever way it decoded.
    ///
    /// The two are the same wire type, and which one comes back depends on the
    /// content: an all-zero blob is indistinguishable from an empty string.
    /// Callers that want the raw bytes should not have to care.
    pub fn as_raw(&self) -> Option<&[u8]> {
        match self {
            Value::Bin(b, _) => Some(b),
            Value::Str(s) => Some(s.as_bytes()),
            _ => None,
        }
    }

    /// Look up an integer-keyed field, the protocol's usual shape.
    pub fn get(&self, key: i64) -> Option<&Value> {
        match self {
            Value::Map(m) => m.iter().find(|(k, _)| *k == Key::Int(key)).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Mutable lookup, for editing a document in place before writing it back.
    pub fn get_mut(&mut self, key: i64) -> Option<&mut Value> {
        match self {
            Value::Map(m) => m
                .iter_mut()
                .find(|(k, _)| *k == Key::Int(key))
                .map(|(_, v)| v),
            _ => None,
        }
    }

    /// Follow a path of integer keys.
    pub fn at_mut(&mut self, path: &[i64]) -> Option<&mut Value> {
        path.iter().try_fold(self, |v, k| v.get_mut(*k))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Ran off the end of the buffer; usually means more bytes are on the way.
    Eof,
    /// A tag byte MessagePack does not define.
    BadTag(u8),
    /// A map key that was neither an integer nor a string.
    BadKey,
    /// More nesting than a preset can legitimately contain.
    NestingTooDeep,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Eof => write!(f, "unexpected end of buffer"),
            Error::BadTag(t) => write!(f, "unknown MessagePack tag {t:#04x}"),
            Error::BadKey => write!(f, "map key was neither integer nor string"),
            Error::NestingTooDeep => write!(f, "MessagePack values are nested too deeply"),
        }
    }
}

impl std::error::Error for Error {}

type Result<T> = std::result::Result<T, Error>;

// ------------------------------------------------------------------ decode ---

pub struct Decoder<'a> {
    buf: &'a [u8],
    pos: usize,
    depth: usize,
}

impl<'a> Decoder<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Decoder {
            buf,
            pos: 0,
            depth: 0,
        }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// Decode every value in the buffer. Nested documents are stored as blobs
    /// that hold a bare sequence of values rather than a single root, so this
    /// is the entry point for those too.
    pub fn decode_all(buf: &'a [u8]) -> Result<Vec<Value>> {
        let mut d = Decoder::new(buf);
        let mut out = Vec::new();
        while d.remaining() > 0 {
            out.push(d.value()?);
        }
        Ok(out)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(Error::Eof)?;
        if end > self.buf.len() {
            return Err(Error::Eof);
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn uint(&mut self, n: usize) -> Result<u64> {
        let b = self.take(n)?;
        Ok(b.iter().fold(0u64, |acc, &x| (acc << 8) | x as u64))
    }

    pub fn value(&mut self) -> Result<Value> {
        // Imported preset documents are untrusted. Recursive arrays and maps
        // otherwise let a tiny input exhaust the process stack before the
        // decoder gets a chance to return an error.
        const MAX_DEPTH: usize = 128;
        if self.depth >= MAX_DEPTH {
            return Err(Error::NestingTooDeep);
        }
        self.depth += 1;
        let result = self.value_inner();
        self.depth -= 1;
        result
    }

    fn value_inner(&mut self) -> Result<Value> {
        let tag = self.u8()?;
        Ok(match tag {
            0x00..=0x7f => Value::UInt(tag as u64),
            0xe0..=0xff => Value::Int(tag as i8 as i64),
            0x80..=0x8f => self.map((tag & 0x0f) as usize)?,
            0x90..=0x9f => self.array((tag & 0x0f) as usize)?,
            0xa0..=0xbf => self.string((tag & 0x1f) as usize, 0)?,
            0xc0 => Value::Nil,
            0xc2 => Value::Bool(false),
            0xc3 => Value::Bool(true),
            0xc4 => {
                let n = self.uint(1)? as usize;
                Value::Bin(self.take(n)?.to_vec(), BIN_FAMILY | 1)
            }
            0xc5 => {
                let n = self.uint(2)? as usize;
                Value::Bin(self.take(n)?.to_vec(), BIN_FAMILY | 2)
            }
            0xc6 => {
                let n = self.uint(4)? as usize;
                Value::Bin(self.take(n)?.to_vec(), BIN_FAMILY | 4)
            }
            0xca => Value::F32(f32::from_bits(self.uint(4)? as u32)),
            0xcb => Value::F64(f64::from_bits(self.uint(8)?)),
            0xcc => Value::Wide(self.uint(1)?, 1),
            0xcd => Value::Wide(self.uint(2)?, 2),
            0xce => Value::Wide(self.uint(4)?, 4),
            0xcf => Value::Wide(self.uint(8)?, 8),
            0xd0 => Value::WideInt(self.uint(1)? as u8 as i8 as i64, 1),
            0xd1 => Value::WideInt(self.uint(2)? as u16 as i16 as i64, 2),
            0xd2 => Value::WideInt(self.uint(4)? as u32 as i32 as i64, 4),
            0xd3 => Value::WideInt(self.uint(8)? as i64, 8),
            0xd9 => {
                let n = self.uint(1)? as usize;
                self.string(n, 1)?
            }
            0xda => {
                let n = self.uint(2)? as usize;
                self.string(n, 2)?
            }
            0xdb => {
                let n = self.uint(4)? as usize;
                self.string(n, 4)?
            }
            0xdc => {
                let n = self.uint(2)? as usize;
                self.array(n)?
            }
            0xdd => {
                let n = self.uint(4)? as usize;
                self.array(n)?
            }
            0xde => {
                let n = self.uint(2)? as usize;
                self.map(n)?
            }
            0xdf => {
                let n = self.uint(4)? as usize;
                self.map(n)?
            }
            other => return Err(Error::BadTag(other)),
        })
    }

    fn array(&mut self, n: usize) -> Result<Value> {
        let mut v = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            v.push(self.value()?);
        }
        Ok(Value::Array(v))
    }

    fn map(&mut self, n: usize) -> Result<Value> {
        let mut m = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            let from = self.pos;
            let value = self.value()?;
            let plain = match &value {
                Value::UInt(u) | Value::Wide(u, _) => {
                    Key::Int(i64::try_from(*u).map_err(|_| Error::BadKey)?)
                }
                Value::WideInt(i, _) | Value::Int(i) => Key::Int(*i),
                Value::Str(s) => Key::Str(s.clone()),
                Value::Bin(..) => Key::Str(value.as_str().ok_or(Error::BadKey)?.to_owned()),
                _ => return Err(Error::BadKey),
            };
            // A plain key encodes with the narrowest header. If that is not
            // how this one arrived, keep the original so it writes back the
            // same.
            let k = if Encoder::encode_key(&plain) == self.buf[from..self.pos] {
                plain
            } else {
                Key::Wide(Box::new(value))
            };
            m.push((k, self.value()?));
        }
        Ok(Value::Map(m))
    }

    /// Line 6 strings include the NUL in the declared length, and the same
    /// types carry opaque blobs, so anything that is not clean text is kept as
    /// bytes for the caller to re-parse.
    fn string(&mut self, n: usize, width: u8) -> Result<Value> {
        let raw = self.take(n)?;
        // `Str` always encodes with the narrowest header, so text that came
        // with a wider one is kept as bytes with its width, or writing it back
        // would shrink the document.
        match c_string(raw) {
            Some(s) if str_width(n) == width => Ok(Value::Str(s.to_owned())),
            _ => Ok(Value::Bin(raw.to_vec(), width)),
        }
    }
}

/// The text in a field spelled the protocol's canonical way: printable UTF-8
/// followed by exactly one NUL. A standard MessagePack string with no NUL, an
/// empty field, or extra padding is not text here; calling it text would make
/// the encoder add or remove bytes and invalidate a preset's offset table.
fn c_string(raw: &[u8]) -> Option<&str> {
    let (&0, text) = raw.split_last()? else {
        return None;
    };
    if text.iter().any(|&b| b < 0x20 && b != b'\t') {
        return None;
    }
    std::str::from_utf8(text).ok()
}

/// The header width the encoder picks for a str of `n` bytes.
fn str_width(n: usize) -> u8 {
    if n < 32 {
        0
    } else if u8::try_from(n).is_ok() {
        1
    } else if u16::try_from(n).is_ok() {
        2
    } else {
        4
    }
}

// ------------------------------------------------------------------ encode ---

#[derive(Default)]
pub struct Encoder {
    pub buf: Vec<u8>,
}

impl Encoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn encode(v: &Value) -> Vec<u8> {
        let mut e = Encoder::new();
        e.value(v);
        e.buf
    }

    pub fn value(&mut self, v: &Value) {
        match v {
            Value::Nil => self.buf.push(0xc0),
            Value::Bool(false) => self.buf.push(0xc2),
            Value::Bool(true) => self.buf.push(0xc3),
            Value::UInt(u) => self.uint(*u),
            Value::WideInt(i, width) => {
                let width = signed_width(*i, *width);
                self.buf.push(match width {
                    1 => 0xd0,
                    2 => 0xd1,
                    4 => 0xd2,
                    _ => 0xd3,
                });
                self.buf.extend_from_slice(&i.to_be_bytes()[8 - width..]);
            }
            Value::Wide(u, width) => {
                let width = unsigned_width(*u, *width);
                self.buf.push(match width {
                    1 => 0xcc,
                    2 => 0xcd,
                    4 => 0xce,
                    _ => 0xcf,
                });
                self.buf.extend_from_slice(&u.to_be_bytes()[8 - width..]);
            }
            Value::Int(i) => {
                if *i >= 0 {
                    self.uint(*i as u64)
                } else if *i >= -32 {
                    self.buf.push(*i as i8 as u8)
                } else {
                    self.buf.push(0xd3);
                    self.buf.extend_from_slice(&i.to_be_bytes());
                }
            }
            Value::F32(f) => {
                self.buf.push(0xca);
                self.buf.extend_from_slice(&f.to_be_bytes());
            }
            Value::F64(f) => {
                self.buf.push(0xcb);
                self.buf.extend_from_slice(&f.to_be_bytes());
            }
            // Round-trip the C-string convention: the length counts the NUL.
            Value::Str(s) => {
                let n = s.len() + 1;
                self.str_header(n);
                self.buf.extend_from_slice(s.as_bytes());
                self.buf.push(0);
            }
            Value::Bin(b, width) => {
                self.str_header_of_width(b.len(), *width);
                self.buf.extend_from_slice(b);
            }
            Value::Array(a) => {
                if a.len() < 16 {
                    self.buf.push(0x90 | a.len() as u8);
                } else if u16::try_from(a.len()).is_ok() {
                    self.buf.push(0xdc);
                    self.buf.extend_from_slice(&(a.len() as u16).to_be_bytes());
                } else {
                    self.buf.push(0xdd);
                    self.buf.extend_from_slice(&(a.len() as u32).to_be_bytes());
                }
                for x in a {
                    self.value(x);
                }
            }
            Value::Map(m) => {
                if m.len() < 16 {
                    self.buf.push(0x80 | m.len() as u8);
                } else if u16::try_from(m.len()).is_ok() {
                    self.buf.push(0xde);
                    self.buf.extend_from_slice(&(m.len() as u16).to_be_bytes());
                } else {
                    self.buf.push(0xdf);
                    self.buf.extend_from_slice(&(m.len() as u32).to_be_bytes());
                }
                for (k, val) in m {
                    self.key(k);
                    self.value(val);
                }
            }
        }
    }

    fn key(&mut self, k: &Key) {
        match k {
            Key::Int(i) => self.value(&Value::Int(*i)),
            Key::Str(s) => self.value(&Value::Str(s.clone())),
            Key::Wide(value) => self.value(value),
        }
    }

    fn encode_key(k: &Key) -> Vec<u8> {
        let mut e = Encoder::new();
        e.key(k);
        e.buf
    }

    /// Write a header of exactly the width and type the value arrived with.
    fn str_header_of_width(&mut self, n: usize, width: u8) {
        if width & BIN_FAMILY != 0 {
            return self.bin_header(n, width & !BIN_FAMILY);
        }
        match width {
            0 if n < 32 => self.buf.push(0xa0 | n as u8),
            1 if u8::try_from(n).is_ok() => {
                self.buf.push(0xd9);
                self.buf.push(n as u8);
            }
            2 if u16::try_from(n).is_ok() => {
                self.buf.push(0xda);
                self.buf.extend_from_slice(&(n as u16).to_be_bytes());
            }
            4 if u32::try_from(n).is_ok() => {
                self.buf.push(0xdb);
                self.buf.extend_from_slice(&(n as u32).to_be_bytes());
            }
            // A blob that outgrew its original tag, or one we built ourselves.
            _ => self.str_header(n),
        }
    }

    /// A bin header, at least as wide as it arrived and wider if the bytes
    /// have outgrown it.
    fn bin_header(&mut self, n: usize, width: u8) {
        if width <= 1 && u8::try_from(n).is_ok() {
            self.buf.push(0xc4);
            self.buf.push(n as u8);
        } else if width <= 2 && u16::try_from(n).is_ok() {
            self.buf.push(0xc5);
            self.buf.extend_from_slice(&(n as u16).to_be_bytes());
        } else {
            self.buf.push(0xc6);
            self.buf.extend_from_slice(&(n as u32).to_be_bytes());
        }
    }

    fn str_header(&mut self, n: usize) {
        if n < 32 {
            self.buf.push(0xa0 | n as u8);
        } else if u8::try_from(n).is_ok() {
            self.buf.push(0xd9);
            self.buf.push(n as u8);
        } else if u16::try_from(n).is_ok() {
            self.buf.push(0xda);
            self.buf.extend_from_slice(&(n as u16).to_be_bytes());
        } else {
            self.buf.push(0xdb);
            self.buf.extend_from_slice(&(n as u32).to_be_bytes());
        }
    }

    fn uint(&mut self, u: u64) {
        if u < 128 {
            self.buf.push(u as u8);
        } else if u < 256 {
            self.buf.push(0xcc);
            self.buf.push(u as u8);
        } else if u < 65536 {
            self.buf.push(0xcd);
            self.buf.extend_from_slice(&(u as u16).to_be_bytes());
        } else if u < 1 << 32 {
            self.buf.push(0xce);
            self.buf.extend_from_slice(&(u as u32).to_be_bytes());
        } else {
            self.buf.push(0xcf);
            self.buf.extend_from_slice(&u.to_be_bytes());
        }
    }
}

fn integer_width(width: u8) -> usize {
    match width {
        1 | 2 | 4 | 8 => usize::from(width),
        _ => 8,
    }
}

fn signed_width(value: i64, requested: u8) -> usize {
    let needed = if i8::try_from(value).is_ok() {
        1
    } else if i16::try_from(value).is_ok() {
        2
    } else if i32::try_from(value).is_ok() {
        4
    } else {
        8
    };
    integer_width(requested).max(needed)
}

fn unsigned_width(value: u64, requested: u8) -> usize {
    let needed = if u8::try_from(value).is_ok() {
        1
    } else if u16::try_from(value).is_ok() {
        2
    } else if u32::try_from(value).is_ok() {
        4
    } else {
        8
    };
    integer_width(requested).max(needed)
}

/// Build a map from integer-keyed pairs - the shape nearly every request takes.
#[macro_export]
macro_rules! msgmap {
    ($($k:expr => $v:expr),* $(,)?) => {{
        $crate::msgpack::Value::Map(vec![
            $( ($crate::msgpack::Key::Int($k), $v) ),*
        ])
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real select-preset request captured from HX Edit:
    /// {102: 1009, 100: 20, 101: {107: 0, 108: 12}}
    const SELECT_PRESET: &[u8] = &[
        0x83, 0x66, 0xcd, 0x03, 0xf1, 0x64, 0x14, 0x65, 0x82, 0x6b, 0x00, 0x6c, 0x0c,
    ];

    #[test]
    fn decodes_a_real_request() {
        let v = Decoder::new(SELECT_PRESET).value().unwrap();
        assert_eq!(v.get(102).unwrap().as_i64(), Some(1009));
        assert_eq!(v.get(100).unwrap().as_i64(), Some(20));
        let args = v.get(101).unwrap();
        assert_eq!(args.get(107).unwrap().as_i64(), Some(0));
        assert_eq!(args.get(108).unwrap().as_i64(), Some(12));
    }

    #[test]
    fn reencodes_a_real_request_byte_for_byte() {
        let v = Decoder::new(SELECT_PRESET).value().unwrap();
        assert_eq!(Encoder::encode(&v), SELECT_PRESET);
    }

    #[test]
    fn strings_keep_the_line6_nul_convention() {
        // 0xa9 introduces nine bytes: "l6-helix" plus its NUL.
        let raw = b"\xa9l6-helix\x00";
        let v = Decoder::new(raw).value().unwrap();
        assert_eq!(v.as_str(), Some("l6-helix"));
        assert_eq!(Encoder::encode(&v), raw);
    }

    #[test]
    fn fields_that_are_not_canonical_c_strings_stay_byte_exact() {
        // None of these is the one-NUL spelling the Line 6 protocol uses for
        // text. Calling them strings would make the encoder add or remove a
        // byte, which is corruption for a document with an offset table.
        for raw in [
            &[0xa0][..],
            &[0xa3, b'f', b'o', b'o'][..],
            &[0xa2, 0, 0][..],
        ] {
            let value = Decoder::new(raw).value().unwrap();
            assert!(matches!(value, Value::Bin(..)));
            assert_eq!(Encoder::encode(&value), raw);
        }
    }

    /// A document written back must be byte-identical, or the preset's own
    /// offset table stops matching its contents.
    #[test]
    fn wide_integers_keep_their_width() {
        for raw in [
            &[0xcc, 0x05][..],
            &[0xcd, 0x00, 0x05][..],
            &[0xce, 0, 0, 0, 5][..],
            &[0xcf, 0, 0, 0, 0, 0, 0, 0, 5][..],
        ] {
            let v = Decoder::new(raw).value().unwrap();
            assert_eq!(v.as_i64(), Some(5));
            assert_eq!(Encoder::encode(&v), raw, "width not preserved");
        }
        // Signed integers too: a preset is full of int16 zeroes that a minimal
        // encoder would collapse to a single byte.
        for raw in [
            &[0xd0u8, 0xfb][..],
            &[0xd1, 0x00, 0x00][..],
            &[0xd2, 0, 0, 0, 7][..],
        ] {
            let v = Decoder::new(raw).value().unwrap();
            assert_eq!(Encoder::encode(&v), raw, "signed width not preserved");
        }
    }

    /// Every header width and type the format allows for text, blobs and
    /// map keys comes back exactly as it went in.
    #[test]
    fn string_bin_and_key_headers_keep_their_width_and_type() {
        for raw in [
            // Text behind headers wider than it needs.
            &[0xd9, 0x03, b'h', b'i', 0][..],
            &[0xda, 0x00, 0x03, b'h', b'i', 0][..],
            &[0xdb, 0x00, 0x00, 0x00, 0x03, b'h', b'i', 0][..],
            // MessagePack bin, which is not the str family.
            &[0xc4, 0x02, 0x01, 0x02][..],
            &[0xc4, 0x03, b'h', b'i', 0][..],
            &[0xc5, 0x00, 0x02, 0x01, 0x02][..],
            &[0xc6, 0x00, 0x00, 0x00, 0x02, 0x01, 0x02][..],
            // Map keys written wider than they need.
            &[0x81, 0xcc, 0x05, 0xc0][..],
            &[0x81, 0xcd, 0x00, 0x05, 0xc0][..],
            &[0x81, 0xd0, 0x05, 0xc0][..],
            &[
                0x81, 0xd3, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfb, 0xc0,
            ][..],
            &[0x81, 0xd9, 0x02, b'k', 0, 0xc0][..],
        ] {
            let value = Decoder::new(raw).value().unwrap();
            assert_eq!(Encoder::encode(&value), raw, "{value:?} changed width");
        }

        // Wider spellings still read the same.
        let text = Decoder::new(&[0xd9, 0x03, b'h', b'i', 0]).value().unwrap();
        assert_eq!(text.as_str(), Some("hi"));
        let bin = Decoder::new(&[0xc4, 0x03, b'h', b'i', 0]).value().unwrap();
        assert_eq!(bin.as_str(), None);
        assert_eq!(bin.as_raw(), Some(&b"hi\0"[..]));
        let map = Decoder::new(&[0x82, 0xcc, 0x05, 0x01, 0xd0, 0x06, 0x02])
            .value()
            .unwrap();
        assert_eq!(map.get(5).and_then(Value::as_i64), Some(1));
        assert_eq!(map.get(6).and_then(Value::as_i64), Some(2));
        let Value::Map(fields) = &map else {
            panic!("expected a map");
        };
        assert_eq!(fields[0].0, Key::Int(5));
        assert_eq!(fields[0].0.as_i64(), Some(5));

        // A blob that outgrows its bin header is promoted within bin.
        let grown = Encoder::encode(&Value::Bin(vec![0; 300], BIN_FAMILY | 1));
        assert_eq!(&grown[..3], &[0xc5, 0x01, 0x2c]);
    }

    #[test]
    fn floats_are_big_endian() {
        let v = Decoder::new(&[0xca, 0x42, 0xf0, 0x00, 0x00])
            .value()
            .unwrap();
        assert_eq!(v.as_f32(), Some(120.0));
    }

    #[test]
    fn invalid_preserved_integer_widths_encode_as_64_bit_values() {
        for width in [0, 3, 9, u8::MAX] {
            let signed = Encoder::encode(&Value::WideInt(-7, width));
            assert_eq!(signed[0], 0xd3);
            assert_eq!(Decoder::new(&signed).value().unwrap().as_i64(), Some(-7));

            let unsigned = Encoder::encode(&Value::Wide(7, width));
            assert_eq!(unsigned[0], 0xcf);
            assert_eq!(Decoder::new(&unsigned).value().unwrap().as_i64(), Some(7));
        }
    }

    #[test]
    fn undersized_preserved_integer_widths_are_promoted() {
        let signed = Encoder::encode(&Value::WideInt(300, 1));
        assert_eq!(signed[0], 0xd1);
        assert_eq!(Decoder::new(&signed).value().unwrap().as_i64(), Some(300));

        let unsigned = Encoder::encode(&Value::Wide(u64::from(u32::MAX) + 1, 2));
        assert_eq!(unsigned[0], 0xcf);
        assert_eq!(
            Decoder::new(&unsigned).value().unwrap(),
            Value::Wide(u64::from(u32::MAX) + 1, 8)
        );
    }

    #[test]
    fn a_blob_is_not_mangled_into_text() {
        // Non-text bytes behind a string tag must survive as bytes.
        let raw = &[0xa4, 0x01, 0x02, 0x03, 0xff];
        match Decoder::new(raw).value().unwrap() {
            Value::Bin(b, _) => assert_eq!(b, vec![0x01, 0x02, 0x03, 0xff]),
            other => panic!("expected blob, got {other:?}"),
        }
    }

    #[test]
    fn values_larger_than_u16_keep_their_full_declared_length() {
        const N: usize = u16::MAX as usize + 1;

        let array = Value::Array(vec![Value::Nil; N]);
        let encoded = Encoder::encode(&array);
        assert_eq!(&encoded[..5], &[0xdd, 0, 1, 0, 0], "array32");
        let Value::Array(decoded) = Decoder::new(&encoded).value().unwrap() else {
            panic!("expected an array");
        };
        assert_eq!(decoded.len(), N);

        let map = Value::Map(
            (0..N)
                .map(|key| (Key::Int(key as i64), Value::Nil))
                .collect(),
        );
        assert_eq!(&Encoder::encode(&map)[..5], &[0xdf, 0, 1, 0, 0]);

        // Strings count their trailing NUL, so 65,535 characters are already
        // one byte too large for str16. A blob that grew beyond the width it
        // arrived with must be promoted in the same way.
        let text = "x".repeat(u16::MAX as usize);
        let encoded = Encoder::encode(&Value::Str(text.clone()));
        assert_eq!(&encoded[..5], &[0xdb, 0, 1, 0, 0], "str32");
        assert_eq!(
            Decoder::new(&encoded).value().unwrap().as_str(),
            Some(text.as_str())
        );

        let encoded = Encoder::encode(&Value::Bin(vec![0; N], 2));
        assert_eq!(&encoded[..5], &[0xdb, 0, 1, 0, 0], "grown str16");
    }

    #[test]
    fn excessive_nesting_and_unrepresentable_map_keys_are_rejected() {
        let mut nested = vec![0x91; 129]; // array containing array containing ...
        nested.push(0xc0);
        assert_eq!(Decoder::new(&nested).value(), Err(Error::NestingTooDeep));

        // Key stores signed protocol keys. Wrapping u64::MAX into -1 would
        // make two distinct wire maps indistinguishable.
        let huge_key = [
            0x81, 0xcf, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xc0,
        ];
        assert_eq!(Decoder::new(&huge_key).value(), Err(Error::BadKey));
    }

    #[test]
    fn an_overflowing_declared_length_is_an_incomplete_value_not_a_panic() {
        let mut decoder = Decoder {
            buf: &[],
            pos: usize::MAX,
            depth: 0,
        };
        assert_eq!(decoder.take(1), Err(Error::Eof));
    }
}
