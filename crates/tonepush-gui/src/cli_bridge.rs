//! An opt-in local listener. Only the editor worker touches the HX session.

use crate::Cmd;
use anyhow::Result;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;
use tonepush_cli::bridge::{Reply, Request, event, private_parent};

pub struct Bridge(PathBuf);

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub fn listen(path: PathBuf, commands: Sender<Cmd>) -> Result<Bridge> {
    private_parent(&path)?;
    let listener = UnixListener::bind(&path)?;
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

fn serve(stream: &mut UnixStream, commands: &Sender<Cmd>) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let request = Request::read(stream)?;
    if request.watch() {
        let (send, receive) = mpsc::sync_channel(64);
        let (ack, status) = mpsc::channel();
        commands
            .send(Cmd::CliWatch(send, ack))
            .map_err(|_| anyhow::anyhow!("TonePush closed"))?;
        status.recv()?.write(stream)?;
        loop {
            match receive.recv_timeout(Duration::from_secs(2)) {
                Ok(line) => event(line).write(stream)?,
                Err(mpsc::RecvTimeoutError::Timeout) => Reply::ok().write(stream)?,
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            }
        }
    }

    let (send, receive) = mpsc::channel();
    commands
        .send(Cmd::Cli(request, send))
        .map_err(|_| anyhow::anyhow!("TonePush closed"))?;
    receive.recv()?.write(stream)
}
