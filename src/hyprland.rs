use crate::{Result, model::Snapshot};
use serde::Deserialize;
use smithay_client_toolkit::reexports::calloop::channel::SyncSender;
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

const RESPONSE_SIZE_BYTES_MAX: u64 = 1024 * 1024;

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
    let mut urgent = [false; 32];
    sender.send(Ok(read_snapshot(&directory, &mut urgent)?))?;
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
        if event == b"urgent" {
            let address = line
                .strip_prefix(b"urgent>>")
                .ok_or("malformed urgent event")?;
            if let Some(workspace) = find_urgent_workspace(&directory, address)? {
                urgent[(workspace - 1) as usize] = true;
            }
        }
        if matches!(
            event,
            b"urgent"
                | b"workspacev2"
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
            sender.send(Ok(read_snapshot(&directory, &mut urgent)?))?;
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
        .take(RESPONSE_SIZE_BYTES_MAX + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > RESPONSE_SIZE_BYTES_MAX {
        return Err("Hyprland response exceeds 1 MiB".into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}

#[derive(Deserialize)]
struct Client {
    address: String,
    workspace: ActiveWorkspace,
}

fn find_urgent_workspace(directory: &Path, address: &[u8]) -> Result<Option<i32>> {
    let address = std::str::from_utf8(address)?.trim();
    let address = address.strip_prefix("0x").unwrap_or(address);
    if address.is_empty() {
        return Err("empty urgent window address".into());
    }
    if address.len() > 32 {
        return Err("urgent window address is too long".into());
    }
    if !address.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid urgent window address".into());
    }
    let clients: Vec<Client> = query(directory, b"j/clients")?;
    Ok(clients
        .into_iter()
        .find(|client| {
            client
                .address
                .trim_start_matches("0x")
                .eq_ignore_ascii_case(address)
        })
        .map(|client| client.workspace.id)
        .filter(|workspace| (1..=32).contains(workspace)))
}

fn read_snapshot(directory: &Path, urgent: &mut [bool; 32]) -> Result<Snapshot> {
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
    for (index, occupied) in occupied.iter().enumerate() {
        if !occupied {
            urgent[index] = false;
        }
    }
    let active = monitor.active_workspace.id;
    if (1..=32).contains(&active) {
        urgent[(active - 1) as usize] = false;
    }
    Ok(Snapshot {
        monitor: monitor.name,
        active,
        occupied,
        urgent: *urgent,
    })
}
