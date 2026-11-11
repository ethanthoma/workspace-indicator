mod hyprland;
mod model;

use model::{Config, Snapshot};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{EventLoop, channel},
        calloop_wayland_source::WaylandSource,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{
        Shm, ShmHandler,
        slot::{Buffer, SlotPool},
    },
};
use std::{
    error::Error,
    time::{Duration, Instant},
};
use wayland_client::{
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_output, wl_shm, wl_surface},
};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

fn main() -> Result<()> {
    let arguments: Vec<_> = std::env::args_os().collect();
    if arguments.len() != 2 {
        return Err("usage: workspace-indicator CONFIG.json".into());
    }
    let config = Config::load(&arguments[1])?;
    let connection = Connection::connect_to_env()?;
    let (globals, queue) = registry_queue_init(&connection)?;
    let handle = queue.handle();
    let compositor = CompositorState::bind(&globals, &handle)?;
    let shell = LayerShell::bind(&globals, &handle)?;
    let shm = Shm::bind(&globals, &handle)?;
    let pool = SlotPool::new(1024 * 1024, &shm)?;
    let mut app = App {
        registry: RegistryState::new(&globals),
        outputs: OutputState::new(&globals, &handle),
        compositor,
        shell,
        shm,
        pool,
        config,
        snapshot: None,
        visible: None,
        failure: None,
        exit: false,
    };
    let mut event_loop: EventLoop<App> = EventLoop::try_new()?;
    WaylandSource::new(connection.clone(), queue).insert(event_loop.handle())?;
    let (sender, receiver) = channel::sync_channel::<std::result::Result<Snapshot, String>>(1);
    let event_handle = handle.clone();
    event_loop
        .handle()
        .insert_source(receiver, move |event, _, app| match event {
            channel::Event::Msg(Ok(snapshot)) => {
                let changed = app
                    .snapshot
                    .as_ref()
                    .is_some_and(|previous| previous.focus() != snapshot.focus());
                app.snapshot = Some(snapshot);
                if changed {
                    app.failure = app.show(&event_handle).err().map(|error| error.to_string());
                }
            }
            channel::Event::Msg(Err(error)) => {
                app.failure = Some(error);
            }
            channel::Event::Closed => {
                app.exit = true;
            }
        })
        .map_err(|error| error.to_string())?;
    std::thread::Builder::new()
        .name("hyprland".into())
        .spawn(move || {
            if let Err(error) = hyprland::watch(&sender) {
                let _ = sender.send(Err(error.to_string()));
            }
        })?;
    while !app.exit {
        if let Some(error) = app.failure.take() {
            return Err(error.into());
        }
        let timeout = app.visible.as_ref().map(|visible| {
            let elapsed = visible.started.elapsed();
            let hold = Duration::from_millis(app.config.display_ms);
            if elapsed < hold {
                hold - elapsed
            } else {
                Duration::from_millis(16)
            }
        });
        event_loop.dispatch(timeout, &mut app)?;
        app.draw()?;
        connection.flush()?;
    }
    if let Some(error) = app.failure {
        return Err(error.into());
    }
    Ok(())
}

struct App {
    registry: RegistryState,
    outputs: OutputState,
    compositor: CompositorState,
    shell: LayerShell,
    shm: Shm,
    pool: SlotPool,
    config: Config,
    snapshot: Option<Snapshot>,
    visible: Option<Visible>,
    failure: Option<String>,
    exit: bool,
}

struct Visible {
    layer: LayerSurface,
    started: Instant,
    configured: bool,
    scale: i32,
    buffers: Vec<Buffer>,
}

impl App {
    fn show(&mut self, handle: &QueueHandle<Self>) -> Result<()> {
        self.visible = None;
        let Some(snapshot) = &self.snapshot else {
            return Ok(());
        };
        let output = self.outputs.outputs().find(|output| {
            self.outputs
                .info(output)
                .is_some_and(|info| info.name.as_deref() == Some(&snapshot.monitor))
        });
        let Some(output) = output else {
            return Ok(());
        };
        let scale = self
            .outputs
            .info(&output)
            .map_or(1, |info| info.scale_factor)
            .clamp(1, 4);
        let surface = self.compositor.create_surface(handle);
        let region = smithay_client_toolkit::compositor::Region::new(&self.compositor)?;
        surface.set_input_region(Some(region.wl_region()));
        let layer = self.shell.create_layer_surface(
            handle,
            surface,
            Layer::Overlay,
            Some("workspace-indicator"),
            Some(&output),
        );
        layer.set_anchor(Anchor::BOTTOM);
        layer.set_margin(0, 0, self.config.margin, 0);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_size(self.config.width(), 40);
        layer.commit();
        self.visible = Some(Visible {
            layer,
            started: Instant::now(),
            configured: false,
            scale,
            buffers: Vec::with_capacity(2),
        });
        Ok(())
    }

    fn draw(&mut self) -> Result<()> {
        let Some(visible) = &mut self.visible else {
            return Ok(());
        };
        let opacity = model::opacity(visible.started.elapsed(), &self.config);
        if opacity == 0.0 {
            self.visible = None;
            return Ok(());
        }
        if !visible.configured {
            return Ok(());
        }
        let width = self.config.width() as i32 * visible.scale;
        let height = 40 * visible.scale;
        let index = visible
            .buffers
            .iter()
            .position(|buffer| buffer.canvas(&mut self.pool).is_some());
        let index = match index {
            Some(index) => index,
            None if visible.buffers.len() < 2 => {
                let (buffer, _) =
                    self.pool
                        .create_buffer(width, height, width * 4, wl_shm::Format::Argb8888)?;
                visible.buffers.push(buffer);
                visible.buffers.len() - 1
            }
            None => return Ok(()),
        };
        let buffer = &visible.buffers[index];
        let Some(canvas) = buffer.canvas(&mut self.pool) else {
            return Ok(());
        };
        let snapshot = self
            .snapshot
            .as_ref()
            .expect("visible surface has a snapshot");
        model::render(
            canvas,
            visible.scale as u32,
            opacity,
            &self.config,
            snapshot,
        );
        let surface = visible.layer.wl_surface();
        surface.set_buffer_scale(visible.scale);
        surface.damage_buffer(0, 0, width, height);
        buffer.attach_to(surface)?;
        visible.layer.commit();
        Ok(())
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        factor: i32,
    ) {
        if let Some(visible) = &mut self.visible {
            visible.scale = factor.clamp(1, 4);
            visible.buffers.clear();
        }
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}
impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.visible = None;
    }
}
impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        if self
            .visible
            .as_ref()
            .is_some_and(|visible| &visible.layer == layer)
        {
            self.visible = None;
        }
    }
    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        _: LayerSurfaceConfigure,
        _: u32,
    ) {
        if let Some(visible) = self
            .visible
            .as_mut()
            .filter(|visible| &visible.layer == layer)
        {
            visible.configured = true;
        }
    }
}
impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}
impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState];
}
delegate_compositor!(App);
delegate_output!(App);
delegate_shm!(App);
delegate_layer!(App);
delegate_registry!(App);
