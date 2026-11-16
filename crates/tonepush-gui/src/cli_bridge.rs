//! An opt-in local listener. Only the editor worker touches the HX session.

use crate::Cmd;
use anyhow::Result;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::time::Duration;
use tonepush_cli::bridge::{event, private_parent, same_user, Reply, Request};

pub struct Bridge(PathBuf);

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub fn listen(path: PathBuf, commands: Sender<Cmd>) -> Result<Bridge> {
    private_parent(&path)?;
    let listener = bind(&path)?;
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let commands = commands.clone();
            std::thread::spawn(move || {
                if let Err(error) = serve(&mut stream, &commands) {
                    let _ = Reply::failed(format!("{error:#}")).write(&mut stream);
                }
            });
        }
    });
    Ok(Bridge(path))
}

/// Bind the socket, taking over a path an editor that crashed left behind.
///
/// A socket file outlives the process that made it, and binding over one
/// fails. Whether an editor still answers there is what tells a leftover
/// from a second editor: only a refused connection is cleared away.
fn bind(path: &Path) -> Result<UnixListener> {
    use std::io::ErrorKind;
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};

    let listener = match UnixListener::bind(path) {
        Err(error) if error.kind() == ErrorKind::AddrInUse => {
            if !std::fs::symlink_metadata(path)?.file_type().is_socket() {
                anyhow::bail!("{} exists and is not a socket", path.display());
            }
            match UnixStream::connect(path) {
                Ok(_) => anyhow::bail!(
                    "another TonePush editor is already listening at {}",
                    path.display()
                ),
                Err(error) if error.kind() == ErrorKind::ConnectionRefused => {
                    std::fs::remove_file(path)?;
                    UnixListener::bind(path)?
                }
                Err(error) => return Err(error.into()),
            }
        }
        listener => listener?,
    };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

fn serve(stream: &mut UnixStream, commands: &Sender<Cmd>) -> Result<()> {
    same_user(stream)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let request = Request::read(stream)?;
    if request.watch() {
        let (send, receive) = mpsc::channel();
        let (ack, status) = mpsc::channel();
        commands
            .send(Cmd::CliWatch(send, ack))
            .map_err(|_| anyhow::anyhow!("TonePush closed"))?;
        status.recv()?.write(stream)?;
        loop {
            match receive.recv_timeout(Duration::from_secs(2)) {
                Ok(line) => event(line).write(stream)?,
                Err(mpsc::RecvTimeoutError::Timeout) => Reply::ok().write(stream)?,
                // The worker drops its watchers when it lets the pedal go.
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    anyhow::bail!("TonePush is no longer connected to the HX device")
                }
            }
        }
    }

    let (send, receive) = mpsc::channel();
    commands
        .send(Cmd::Cli(request, send))
        .map_err(|_| anyhow::anyhow!("TonePush closed"))?;
    receive.recv()?.write(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A private directory with a short path: a socket's is limited to about
    /// a hundred bytes.
    fn private_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tp-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    /// An editor that crashed leaves its socket file behind, and the next
    /// one has to be able to listen at the same path. One that is still
    /// running keeps it.
    #[test]
    fn a_leftover_socket_is_taken_over_and_a_live_one_is_not() {
        let dir = private_dir("stale");
        let path = dir.join("cli.sock");

        // Dropping a listener closes it and leaves the file, as a crash does.
        drop(bind(&path).unwrap());
        assert!(path.exists());

        let live = bind(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        let error = bind(&path).unwrap_err().to_string();
        assert!(error.contains("already listening"), "{error}");
        assert!(path.exists(), "the live editor's socket was removed");
        drop(live);

        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "not a socket").unwrap();
        assert!(bind(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"not a socket");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
