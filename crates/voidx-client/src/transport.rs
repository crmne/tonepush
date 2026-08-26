use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A blocking, ordered byte stream. VoidX framing and sequencing live above
/// this boundary, making captures and future TCP/BLE transports interchangeable.
pub trait Link: Read + Write + Send {
    fn description(&self) -> &str;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub port_name: String,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial_number: Option<String>,
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
}

impl Found {
    pub fn open(&self) -> crate::Result<SerialLink> {
        SerialLink::open(&self.port_name)
    }
}

/// List ports whose USB descriptors identify a StompStation PRO.
///
/// We never send discovery probes to arbitrary serial devices. If a platform
/// cannot expose USB metadata, callers can still explicitly open a chosen path.
pub fn list() -> crate::Result<Vec<Found>> {
    let mut found = Vec::new();
    for port in serialport::available_ports()? {
        let mut candidate = match port.port_type {
            serialport::SerialPortType::UsbPort(info) => Found {
                port_name: port.port_name,
                manufacturer: info.manufacturer,
                product: info.product,
                serial_number: info.serial_number,
                vendor_id: Some(info.vid),
                product_id: Some(info.pid),
            },
            _ => Found {
                port_name: port.port_name,
                manufacturer: None,
                product: None,
                serial_number: None,
                vendor_id: None,
                product_id: None,
            },
        };
        enrich_from_linux_sysfs(&mut candidate);
        if is_stompstation_pro(&candidate) {
            found.push(candidate);
        }
    }
    found.sort_by(|a, b| a.port_name.cmp(&b.port_name));

    #[cfg(target_os = "macos")]
    {
        let cu_ports = found
            .iter()
            .filter_map(|item| item.port_name.strip_prefix("/dev/cu."))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        found.retain(|item| {
            item.port_name
                .strip_prefix("/dev/tty.")
                .is_none_or(|suffix| !cu_ports.iter().any(|candidate| candidate == suffix))
        });
    }

    Ok(found)
}

fn is_stompstation_pro(found: &Found) -> bool {
    let maker = found
        .manufacturer
        .as_deref()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("SONULAB"));
    let product = found
        .product
        .as_deref()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("StompStation PRO"));
    maker && product
}

#[cfg(target_os = "linux")]
fn enrich_from_linux_sysfs(found: &mut Found) {
    let Some(tty) = Path::new(&found.port_name).file_name() else {
        return;
    };
    let Ok(mut node) = fs::canonicalize(Path::new("/sys/class/tty").join(tty).join("device"))
    else {
        return;
    };
    while node != Path::new("/") {
        if node.join("idVendor").is_file() && node.join("idProduct").is_file() {
            found.vendor_id = found
                .vendor_id
                .or_else(|| read_hex_u16(node.join("idVendor")));
            found.product_id = found
                .product_id
                .or_else(|| read_hex_u16(node.join("idProduct")));
            if found.manufacturer.is_none() {
                found.manufacturer = read_trimmed(node.join("manufacturer"));
            }
            if found.product.is_none() {
                found.product = read_trimmed(node.join("product"));
            }
            if found.serial_number.is_none() {
                found.serial_number = read_trimmed(node.join("serial"));
            }
            break;
        }
        if !node.pop() {
            break;
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn enrich_from_linux_sysfs(_: &mut Found) {}

#[cfg(target_os = "linux")]
fn read_trimmed(path: PathBuf) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(target_os = "linux")]
fn read_hex_u16(path: PathBuf) -> Option<u16> {
    u16::from_str_radix(read_trimmed(path)?.as_str(), 16).ok()
}

pub struct SerialLink {
    description: String,
    port: Box<dyn serialport::SerialPort>,
}

impl SerialLink {
    pub fn open(port_name: &str) -> crate::Result<Self> {
        let port = serialport::new(port_name, 115_200)
            .timeout(Duration::from_millis(100))
            .open()?;
        Ok(Self {
            description: port_name.to_owned(),
            port,
        })
    }
}

impl Read for SerialLink {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.port.read(buffer)
    }
}

impl Write for SerialLink {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.port.write(buffer)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.port.flush()
    }
}

impl Link for SerialLink {
    fn description(&self) -> &str {
        &self.description
    }
}
