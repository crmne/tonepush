use std::io::{Read, Write};

use voidx_client::list;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let subject = std::env::args()
        .nth(1)
        .ok_or("usage: raw-query <read-or-browse-command>")?;
    if !(subject.starts_with("read root") || subject.starts_with("browse root")) {
        return Err("raw-query permits read and browse commands only".into());
    }
    if subject
        .bytes()
        .any(|byte| matches!(byte, 0 | b'\r' | b'\n'))
    {
        return Err("command contains a framing byte".into());
    }
    let found = list()?;
    if found.len() != 1 {
        return Err(format!("expected one StompStation PRO, found {}", found.len()).into());
    }
    let mut link = found[0].open()?;
    link.write_all(subject.as_bytes())?;
    link.write_all(&[0])?;
    link.flush()?;
    let mut response = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        match link.read(&mut buffer) {
            Ok(0) => {}
            Ok(length) => {
                response.extend_from_slice(&buffer[..length]);
                if response.contains(&0) {
                    break;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error.into()),
        }
    }
    println!("{}", response.escape_ascii());
    Ok(())
}
