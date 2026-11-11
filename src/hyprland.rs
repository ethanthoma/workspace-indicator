use crate::{Result, model::Snapshot};
use serde::Deserialize;
use smithay_client_toolkit::reexports::calloop::channel::SyncSender;
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

const RESPONSE_BYTES_MAX: u64 = 1024 * 1024;

pub fn watch(sender: &SyncSender<std::result::Result<Snapshot, String>>) -> Result<()> {
    let directory =
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is missing")?)
            .join("hypr")
            .join(
                std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")
                    .ok_or("HYPRLAND_INSTANCE_SIGNATURE is missing")?,
            );
    // Subscribe before reading state so a switch during initialization is not lost.
    let mut events = BufReader::new(UnixStream::connect(directory.join(".socket2.sock"))?);
    sender.send(Ok(snapshot(&directory)?))?;
    loop {
        let mut line = Vec::new();
        let count = events.by_ref().take(4097).read_until(b'\n', &mut line)?;
        if count == 0 {
            return Err("Hyprland event socket closed".into());
        }
        if line.len() > 4096 {
            return Err("Hyprland event exceeds 4096 bytes".into());
        }
        let event = line.split(|byte| *byte == b'>').next().unwrap_or_default();
        if matches!(
            event,
            b"workspacev2"
                | b"focusedmonv2"
                | b"createworkspacev2"
                | b"destroyworkspacev2"
                | b"moveworkspacev2"
                | b"openwindow"
                | b"closewindow"
                | b"movewindowv2"
                | b"monitoraddedv2"
                | b"monitorremoved"
        ) {
            sender.send(Ok(snapshot(&directory)?))?;
        }
    }
}

#[derive(Deserialize)]
struct Workspace {
    id: i32,
    windows: u32,
}
#[derive(Deserialize)]
struct ActiveWorkspace {
    id: i32,
}
#[derive(Deserialize)]
struct Monitor {
    name: String,
    focused: bool,
    #[serde(rename = "activeWorkspace")]
    active_workspace: ActiveWorkspace,
}

fn query<T: serde::de::DeserializeOwned>(directory: &Path, command: &[u8]) -> Result<T> {
    let mut socket = UnixStream::connect(directory.join(".socket.sock"))?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    socket.write_all(command)?;
    let mut bytes = Vec::new();
    socket
        .take(RESPONSE_BYTES_MAX + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > RESPONSE_BYTES_MAX {
        return Err("Hyprland response exceeds 1 MiB".into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn snapshot(directory: &Path) -> Result<Snapshot> {
    let monitors: Vec<Monitor> = query(directory, b"j/monitors")?;
    let workspaces: Vec<Workspace> = query(directory, b"j/workspaces")?;
    let monitor = monitors
        .into_iter()
        .find(|monitor| monitor.focused)
        .ok_or("Hyprland has no focused monitor")?;
    let mut occupied = [false; 32];
    for workspace in workspaces {
        if (1..=32).contains(&workspace.id) {
            occupied[(workspace.id - 1) as usize] = workspace.windows > 0;
        }
    }
    Ok(Snapshot {
        monitor: monitor.name,
        active: monitor.active_workspace.id,
        occupied,
    })
}
