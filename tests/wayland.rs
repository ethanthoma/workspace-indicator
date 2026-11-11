use std::{
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI32, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

// Exercises real Wayland configure/frame/buffer-release handling without switching
// the user's actual workspaces. Only the Hyprland IPC state is simulated.
#[test]
#[ignore = "requires a running Hyprland session"]
fn persistent_surface_survives_animation_reversal_and_idle() {
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap());
    let signature = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").unwrap();
    let mut socket =
        UnixStream::connect(runtime.join("hypr").join(signature).join(".socket.sock")).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    socket.write_all(b"j/monitors").unwrap();
    let mut response = Vec::new();
    socket.take(1024 * 1024).read_to_end(&mut response).unwrap();
    let monitors: serde_json::Value = serde_json::from_slice(&response).unwrap();
    let monitor = monitors
        .as_array()
        .unwrap()
        .iter()
        .find(|monitor| monitor["focused"] == true)
        .unwrap()["name"]
        .as_str()
        .unwrap()
        .to_owned();

    let directory =
        std::env::temp_dir().join(format!("workspace-indicator-test-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let mut fixture = Fixture {
        directory,
        child: None,
        running: Arc::new(AtomicBool::new(true)),
        server: None,
    };
    let ipc = fixture.directory.join("hypr/test");
    std::fs::create_dir_all(&ipc).unwrap();
    let commands = UnixListener::bind(ipc.join(".socket.sock")).unwrap();
    let events = UnixListener::bind(ipc.join(".socket2.sock")).unwrap();
    commands.set_nonblocking(true).unwrap();
    events.set_nonblocking(true).unwrap();
    let active = Arc::new(AtomicI32::new(1));
    let server_active = active.clone();
    let running = fixture.running.clone();
    fixture.server = Some(thread::spawn(move || {
        while running.load(Ordering::Relaxed) {
            match commands.accept() {
                Ok((mut socket, _)) => {
                    socket
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    socket
                        .set_write_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut command = [0; 64];
                    let count = socket.read(&mut command).unwrap();
                    let active = server_active.load(Ordering::Relaxed);
                    let value = match &command[..count] {
                        b"j/monitors" => {
                            serde_json::json!([{ "name": monitor, "focused": true, "activeWorkspace": { "id": active } }])
                        }
                        b"j/workspaces" => {
                            serde_json::json!([{ "id": 1, "windows": 1 }, { "id": 5, "windows": 2 }])
                        }
                        b"j/clients" => {
                            serde_json::json!([{ "address": "0xabc", "workspace": { "id": 5 } }])
                        }
                        command => panic!("unexpected IPC command: {command:?}"),
                    };
                    socket.write_all(value.to_string().as_bytes()).unwrap();
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(error) => panic!("IPC accept: {error}"),
            }
        }
    }));
    let stderr = std::fs::File::create(fixture.directory.join("stderr")).unwrap();
    let display = PathBuf::from(std::env::var_os("WAYLAND_DISPLAY").unwrap());
    let display = if display.is_absolute() {
        display
    } else {
        runtime.join(display)
    };
    fixture.child = Some(
        Command::new(env!("CARGO_BIN_EXE_workspace-indicator"))
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/config.json"))
            .env("XDG_RUNTIME_DIR", &fixture.directory)
            .env("HYPRLAND_INSTANCE_SIGNATURE", "test")
            .env("WAYLAND_DISPLAY", display)
            .stdout(Stdio::null())
            .stderr(stderr)
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut events = loop {
        match events.accept() {
            Ok((socket, _)) => break socket,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "child did not subscribe");
                fixture.assert_alive();
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("event accept: {error}"),
        }
    };
    events
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    thread::sleep(Duration::from_millis(500));
    fixture.assert_alive();
    for workspace in [5, 2, 4, 1] {
        active.store(workspace, Ordering::Relaxed);
        writeln!(events, "workspacev2>>{workspace},{workspace}").unwrap();
        thread::sleep(Duration::from_millis(70));
        fixture.assert_alive();
    }
    writeln!(events, "urgent>>abc").unwrap();
    thread::sleep(Duration::from_millis(1200));
    fixture.assert_alive();
    assert_eq!(
        std::fs::read_to_string(fixture.directory.join("stderr")).unwrap(),
        ""
    );
}

struct Fixture {
    directory: PathBuf,
    child: Option<Child>,
    running: Arc<AtomicBool>,
    server: Option<thread::JoinHandle<()>>,
}
impl Fixture {
    fn assert_alive(&mut self) {
        let status = self.child.as_mut().unwrap().try_wait().unwrap();
        assert!(
            status.is_none(),
            "indicator exited: {status:?}: {}",
            std::fs::read_to_string(self.directory.join("stderr")).unwrap()
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
        self.running.store(false, Ordering::Relaxed);
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
