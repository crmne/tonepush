//! Firmware images for the StompStation PRO and the update-mode transfer.
//!
//! An official update is a `.zip` holding one `.upd` file, which is the
//! pedal's whole DSP application: a 64-bit ARM Linux executable. The pedal
//! writes whatever it receives and checks nothing, so every check lives here.
//! An image is accepted only when it is that kind of executable and its
//! version is known, either from the SHA-256 of an official release or, for a
//! release TonePush does not list yet, from a file name such as
//! `s_pro_2_2_6.upd` whose version also appears in the image.
//!
//! The transfer itself is described in [`voidx_proto::update`]. The pedal is
//! never restarted from here: the image becomes active after the owner turns
//! it off and on again, once the pedal has had time to finish writing.

use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use voidx_proto::update::{self, UPDATE_MODE_VERSION};

use crate::{Device, Error, Identity, Link, Result};

/// Official StompStation PRO releases: version, SHA-256 of the `.upd`.
const OFFICIAL: &[(&str, &str)] = &[
    (
        "1.5.12",
        "eb4892b99d85e5958d0819c3b352dc177032d3966e8699b7fdcb80eb97b1137a",
    ),
    (
        "2.0.10",
        "08fb13cd5860a03a635ca3c4c1a5d2c12b8fd3cea166555158ed9281c9149d4d",
    ),
    (
        "2.2.6",
        "63d9b8047ae47a0c9a22b6be7c7de6540cc8d2b2c4dabb756a842b14043083c9",
    ),
];

/// Images are a few megabytes; anything far outside that is not one.
const SIZE_RANGE: std::ops::RangeInclusive<usize> = 1 << 20..=64 << 20;
/// The 1.5.12 updater reads at most 10000 bytes per command, and one batch
/// costs twice its size in hex plus five bytes. Larger batches are refused.
const MAX_BATCH: u64 = 4096;
const REPLY_TIMEOUT: Duration = Duration::from_secs(15);
/// The last reply comes after the pedal has written the whole image.
const FINAL_REPLY_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub version: String,
    pub sha256: String,
    /// The SHA-256 matches an official release TonePush lists.
    pub official: bool,
    bytes: Vec<u8>,
}

impl Image {
    /// Read a `.upd`, or a `.zip` holding exactly one.
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read(path).map_err(|source| Error::Io {
            operation: "reading the firmware file",
            source,
        })?;
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if raw.starts_with(b"PK\x03\x04") {
            let (name, bytes) = single_zip_entry(&raw)?;
            Self::from_bytes(&name, bytes)
        } else {
            Self::from_bytes(file_name, raw)
        }
    }

    pub fn from_bytes(file_name: &str, bytes: Vec<u8>) -> Result<Self> {
        if !SIZE_RANGE.contains(&bytes.len()) {
            return Err(Error::Format(format!(
                "{file_name} is {} bytes, not a StompStation PRO firmware image",
                bytes.len()
            )));
        }
        check_aarch64_linux_executable(&bytes)
            .map_err(|why| Error::Format(format!("{file_name} is not PRO firmware: {why}")))?;
        let sha256 = hex(&Sha256::digest(&bytes));
        if let Some((version, _)) = OFFICIAL.iter().find(|(_, hash)| *hash == sha256) {
            return Ok(Self {
                version: (*version).to_owned(),
                sha256,
                official: true,
                bytes,
            });
        }
        let version = version_from_name(file_name).ok_or_else(|| {
            Error::Format(format!(
                "{file_name} is not an official release TonePush knows, and its name does not say its version (like s_pro_2_2_6.upd)"
            ))
        })?;
        let marker = [b"\0", version.as_bytes(), b"\0"].concat();
        if !bytes.windows(marker.len()).any(|window| window == marker) {
            return Err(Error::Format(format!(
                "{file_name} says it is {version}, but that version is not inside the image"
            )));
        }
        Ok(Self {
            version,
            sha256,
            official: false,
            bytes,
        })
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

/// Whether the connected pedal is running its built-in updater.
pub fn is_update_mode(identity: &Identity) -> bool {
    identity.version == UPDATE_MODE_VERSION
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The pedal accepted the size and asked for batches of this many bytes.
    Started { batch: usize },
    /// The pedal confirmed this many bytes of `total`.
    Sent { bytes: usize, total: usize },
}

/// Send `image` to a pedal in update mode. The pedal writes it once the last
/// byte arrives; it must then be left on for a few minutes and turned off and
/// on by hand. Every reply must confirm exactly the bytes sent so far, or the
/// transfer stops before the pedal has the whole image and nothing is written.
pub fn flash<L: Link>(
    device: Device<L>,
    image: &Image,
    mut progress: impl FnMut(Step),
) -> Result<L> {
    let identity = device.identity().clone();
    if identity.name != "StompStation PRO" || !is_update_mode(&identity) {
        return Err(Error::WriteRefused(format!(
            "the pedal reports {} {}; start it in update mode (hold UPD while it starts) first",
            identity.name, identity.version
        )));
    }
    let mut link = device.disconnect();
    let mut replies = Replies::default();
    let total = image.bytes.len();

    send(&mut link, &update::start(total))?;
    let batch = replies.next(&mut link, REPLY_TIMEOUT, "supd")?;
    if !(1..=MAX_BATCH).contains(&batch) {
        return Err(Error::InvalidResponse {
            subject: "supd".into(),
            detail: format!("the pedal asked for batches of {batch} bytes"),
        });
    }
    let batch = batch as usize;
    progress(Step::Started { batch });

    let mut sent = 0usize;
    for chunk in image.bytes.chunks(batch) {
        send(&mut link, &update::data(chunk))?;
        sent += chunk.len();
        let timeout = if sent == total {
            FINAL_REPLY_TIMEOUT
        } else {
            REPLY_TIMEOUT
        };
        let confirmed = replies.next(&mut link, timeout, "upd")?;
        if confirmed != sent as u64 {
            return Err(Error::InvalidResponse {
                subject: "upd".into(),
                detail: format!("the pedal confirmed {confirmed} bytes after {sent} were sent"),
            });
        }
        progress(Step::Sent { bytes: sent, total });
    }
    Ok(link)
}

fn send<L: Link>(link: &mut L, command: &[u8]) -> Result<()> {
    link.write_all(command)
        .and_then(|()| link.flush())
        .map_err(|source| Error::Io {
            operation: "sending firmware to the pedal",
            source,
        })
}

/// NUL-delimited reply tokens, skipping any subject:JSON record.
#[derive(Default)]
struct Replies {
    buffer: Vec<u8>,
}

impl Replies {
    fn next<L: Link>(&mut self, link: &mut L, timeout: Duration, subject: &str) -> Result<u64> {
        let deadline = Instant::now() + timeout;
        loop {
            while let Some(end) = self.buffer.iter().position(|byte| *byte == 0) {
                let token = self.buffer.drain(..=end).collect::<Vec<_>>();
                match update::parse_count(&token[..token.len() - 1]) {
                    None => continue,
                    Some(Ok(count)) => return Ok(count),
                    Some(Err(error)) => {
                        return Err(Error::InvalidResponse {
                            subject: subject.into(),
                            detail: error.to_string(),
                        })
                    }
                }
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout {
                    subject: subject.into(),
                });
            }
            let mut chunk = [0_u8; 256];
            match link.read(&mut chunk) {
                Ok(length) => self.buffer.extend_from_slice(&chunk[..length]),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) => {}
                Err(source) => {
                    return Err(Error::Io {
                        operation: "reading the pedal's update reply",
                        source,
                    })
                }
            }
        }
    }
}

fn check_aarch64_linux_executable(bytes: &[u8]) -> std::result::Result<(), &'static str> {
    let header = bytes.get(..20).ok_or("too short")?;
    if &header[..4] != b"\x7fELF" {
        return Err("not an ELF executable");
    }
    if header[4] != 2 || header[5] != 1 {
        return Err("not a 64-bit little-endian executable");
    }
    let kind = u16::from_le_bytes([header[16], header[17]]);
    if kind != 2 && kind != 3 {
        return Err("not an executable");
    }
    if u16::from_le_bytes([header[18], header[19]]) != 0xb7 {
        return Err("built for another processor");
    }
    Ok(())
}

/// `s_pro_2_2_6.upd` or `s_pro_2_2_6.zip` to `2.2.6`.
fn version_from_name(name: &str) -> Option<String> {
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    let digits = stem.strip_prefix("s_pro_")?;
    let parts = digits.split('_').collect::<Vec<_>>();
    (parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty() && part.len() <= 3 && part.bytes().all(|b| b.is_ascii_digit())
        }))
    .then(|| parts.join("."))
}

/// The one file in a zip archive, inflated and checked against its CRC-32.
fn single_zip_entry(zip: &[u8]) -> Result<(String, Vec<u8>)> {
    let bad = |why: &str| Error::Format(format!("firmware archive: {why}"));
    let u16_at = |at: usize| {
        zip.get(at..at + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
    };
    let u32_at = |at: usize| {
        zip.get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    // The central directory has the sizes even when the local header defers
    // them to a data descriptor.
    let end = (0..zip.len().saturating_sub(21))
        .rev()
        .find(|&at| zip[at..].starts_with(b"PK\x05\x06"))
        .ok_or_else(|| bad("no central directory"))?;
    if u16_at(end + 10) != Some(1) {
        return Err(bad("it must hold exactly one file"));
    }
    let central = u32_at(end + 16).ok_or_else(|| bad("truncated"))? as usize;
    if !zip
        .get(central..)
        .is_some_and(|tail| tail.starts_with(b"PK\x01\x02"))
    {
        return Err(bad("damaged central directory"));
    }
    let method = u16_at(central + 10).ok_or_else(|| bad("truncated"))?;
    let crc = u32_at(central + 16).ok_or_else(|| bad("truncated"))?;
    let compressed = u32_at(central + 20).ok_or_else(|| bad("truncated"))? as usize;
    let size = u32_at(central + 24).ok_or_else(|| bad("truncated"))? as usize;
    let name_length = u16_at(central + 28).ok_or_else(|| bad("truncated"))?;
    let local = u32_at(central + 42).ok_or_else(|| bad("truncated"))? as usize;
    let name = zip
        .get(central + 46..central + 46 + name_length)
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .ok_or_else(|| bad("truncated"))?;
    if !name.ends_with(".upd") {
        return Err(bad("it does not hold a .upd file"));
    }
    if !SIZE_RANGE.contains(&size) {
        return Err(bad("its file is not the size of a firmware image"));
    }
    if !zip
        .get(local..)
        .is_some_and(|tail| tail.starts_with(b"PK\x03\x04"))
    {
        return Err(bad("damaged local header"));
    }
    let start = local
        + 30
        + u16_at(local + 26).ok_or_else(|| bad("truncated"))?
        + u16_at(local + 28).ok_or_else(|| bad("truncated"))?;
    let data = zip
        .get(start..start + compressed)
        .ok_or_else(|| bad("truncated"))?;
    let bytes = match method {
        0 => data.to_vec(),
        8 => {
            let mut bytes = Vec::with_capacity(size);
            flate2::read::DeflateDecoder::new(data)
                .take(size as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| bad("its file does not inflate"))?;
            bytes
        }
        _ => return Err(bad("unsupported compression")),
    };
    let mut check = flate2::Crc::new();
    check.update(&bytes);
    if bytes.len() != size || check.sum() != crc {
        return Err(bad("its file is damaged (size or CRC-32 differ)"));
    }
    Ok((name, bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io::Write;

    use super::*;

    fn elf(size: usize) -> Vec<u8> {
        let mut bytes = vec![0_u8; size];
        bytes[..6].copy_from_slice(b"\x7fELF\x02\x01");
        bytes[16] = 3;
        bytes[18] = 0xb7;
        bytes
    }

    #[test]
    fn an_image_must_be_an_arm64_executable_of_a_known_version() {
        let mut bytes = elf(2 << 20);
        let marker = b"\x002.3.4\x00";
        bytes[1000..1000 + marker.len()].copy_from_slice(marker);

        let image = Image::from_bytes("s_pro_2_3_4.upd", bytes.clone()).unwrap();
        assert_eq!(image.version, "2.3.4");
        assert!(!image.official);

        assert!(Image::from_bytes("firmware.upd", bytes.clone()).is_err());
        assert!(Image::from_bytes("s_pro_2_3_5.upd", bytes.clone()).is_err());
        let mut x86 = bytes.clone();
        x86[18] = 0x3e;
        assert!(Image::from_bytes("s_pro_2_3_4.upd", x86).is_err());
        assert!(Image::from_bytes("s_pro_2_3_4.upd", b"\x7fELF".to_vec()).is_err());
    }

    #[test]
    fn a_zip_yields_its_single_upd() {
        let mut image = elf(1 << 20);
        image[2000..2009].copy_from_slice(b"\x001.2.300\x00");
        let zip = zip_of("s_pro_1_2_300.upd", &image);
        let dir = std::env::temp_dir().join(format!("voidx-firmware-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s_pro_1_2_300.zip");
        std::fs::write(&path, &zip).unwrap();
        let loaded = Image::load(&path).unwrap();
        assert_eq!(loaded.version, "1.2.300");
        assert_eq!(loaded.len(), image.len());

        let mut damaged = zip.clone();
        let middle = damaged.len() / 2;
        damaged[middle] ^= 0xff;
        std::fs::write(&path, &damaged).unwrap();
        assert!(Image::load(&path).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A minimal deflated single-entry archive.
    fn zip_of(name: &str, bytes: &[u8]) -> Vec<u8> {
        let mut encoder =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        let data = encoder.finish().unwrap();
        let mut crc = flate2::Crc::new();
        crc.update(bytes);
        let mut zip = Vec::new();
        let common = |zip: &mut Vec<u8>| {
            zip.extend_from_slice(&8_u16.to_le_bytes());
            zip.extend_from_slice(&[0; 4]);
            zip.extend_from_slice(&crc.sum().to_le_bytes());
            zip.extend_from_slice(&(data.len() as u32).to_le_bytes());
            zip.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            zip.extend_from_slice(&(name.len() as u16).to_le_bytes());
        };
        zip.extend_from_slice(b"PK\x03\x04\x14\x00\x00\x00");
        common(&mut zip);
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip.extend_from_slice(name.as_bytes());
        zip.extend_from_slice(&data);
        let central = zip.len();
        zip.extend_from_slice(b"PK\x01\x02\x14\x00\x14\x00\x00\x00");
        common(&mut zip);
        zip.extend_from_slice(&[0; 12]);
        zip.extend_from_slice(&0_u32.to_le_bytes());
        zip.extend_from_slice(name.as_bytes());
        let size = zip.len() - central;
        zip.extend_from_slice(b"PK\x05\x06\x00\x00\x00\x00\x01\x00\x01\x00");
        zip.extend_from_slice(&(size as u32).to_le_bytes());
        zip.extend_from_slice(&(central as u32).to_le_bytes());
        zip.extend_from_slice(&0_u16.to_le_bytes());
        zip
    }

    /// Answers the identity reads, then one scripted reply per update command.
    struct Pedal {
        input: VecDeque<u8>,
        replies: VecDeque<&'static str>,
        output: Vec<u8>,
    }

    impl Read for Pedal {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let length = buffer.len().min(self.input.len());
            if length == 0 {
                return Err(std::io::ErrorKind::TimedOut.into());
            }
            for (slot, byte) in buffer.iter_mut().zip(self.input.drain(..length)) {
                *slot = byte;
            }
            Ok(length)
        }
    }

    impl Write for Pedal {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.output.extend_from_slice(buffer);
            if buffer.starts_with(b"supd ") || buffer.starts_with(b"upd ") {
                if let Some(reply) = self.replies.pop_front() {
                    self.input.extend(reply.as_bytes());
                }
            }
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Link for Pedal {
        fn description(&self) -> &str {
            "pedal"
        }
    }

    fn pedal(version: &str, replies: &[&'static str]) -> Device<Pedal> {
        let identity = format!(
            "root\\sys\\_name:{{\"value\":\"StompStation PRO\"}}\0\
             root\\sys\\_ver:{{\"value\":\"{version}\"}}\0\
             root\\sys\\_arch:{{\"value\":\"CM4\"}}\0\
             root\\sys\\_license:{{\"value\":\"sspro\"}}\0"
        );
        Device::connect_with_timeout(
            Pedal {
                input: identity.into_bytes().into(),
                replies: replies.iter().copied().collect(),
                output: Vec::new(),
            },
            Duration::from_secs(1),
        )
        .unwrap()
    }

    fn image(size: usize) -> Image {
        Image {
            version: "2.2.6".into(),
            sha256: String::new(),
            official: true,
            bytes: (0..size).map(|index| index as u8).collect(),
        }
    }

    #[test]
    fn the_image_goes_in_confirmed_batches() {
        let device = pedal(
            "Update Mode",
            &["2048\0", "\r\n2048\r\n\0", "4096\0", "5000\0"],
        );
        let mut steps = Vec::new();
        let link = flash(device, &image(5000), |step| steps.push(step)).unwrap();
        assert_eq!(
            steps,
            [
                Step::Started { batch: 2048 },
                Step::Sent {
                    bytes: 2048,
                    total: 5000
                },
                Step::Sent {
                    bytes: 4096,
                    total: 5000
                },
                Step::Sent {
                    bytes: 5000,
                    total: 5000
                },
            ]
        );
        let sent = String::from_utf8(link.output).unwrap();
        let commands = sent.split('\0').collect::<Vec<_>>();
        assert!(commands.contains(&"supd 5000"));
        assert!(commands[commands.len() - 2].starts_with("upd 00010203"));
        assert_eq!(
            commands
                .iter()
                .filter(|command| command.starts_with("upd "))
                .count(),
            3
        );
    }

    #[test]
    fn a_wrong_count_stops_before_the_last_batch() {
        let device = pedal("Update Mode", &["2048\0", "2000\0"]);
        assert!(matches!(
            flash(device, &image(5000), |_| {}),
            Err(Error::InvalidResponse { .. })
        ));
    }

    #[test]
    fn only_a_pedal_in_update_mode_is_flashed() {
        let device = pedal("1.5.12", &[]);
        assert!(matches!(
            flash(device, &image(5000), |_| {}),
            Err(Error::WriteRefused(_))
        ));
    }
}
