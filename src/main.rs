mod hyprland;
mod model;

use model::{Configuration, Motion, Snapshot};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
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
const BUFFER_POOL_SIZE_BYTES: usize = 8 * 1024 * 1024;

fn main() -> Result<()> {
    let arguments: Vec<_> = std::env::args_os().collect();
    if arguments.len() != 2 {
        return Err("usage: workspace-indicator CONFIG.json".into());
    }
    let configuration = Configuration::load(&arguments[1])?;
    let connection = Connection::connect_to_env()?;
    let (globals, queue) = registry_queue_init(&connection)?;
    let handle = queue.handle();
    let compositor = CompositorState::bind(&globals, &handle)?;
    let shell = LayerShell::bind(&globals, &handle)?;
    let shm = Shm::bind(&globals, &handle)?;
    let pool = SlotPool::new(BUFFER_POOL_SIZE_BYTES, &shm)?;
    let mut app = App {
        registry: RegistryState::new(&globals),
        outputs: OutputState::new(&globals, &handle),
        compositor,
        shell,
        shm,
        pool,
        configuration,
        snapshot: None,
        visible: None,
        epoch: Instant::now(),
        motion: None,
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
                app.update_snapshot(snapshot, &event_handle);
            }
            channel::Event::Msg(Err(error)) => app.failure = Some(error),
            channel::Event::Closed => app.exit = true,
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
        event_loop.dispatch(None, &mut app)?;
        if let Some(error) = app.failure.take() {
            return Err(error.into());
        }
        app.draw(&handle)?;
        connection.flush()?;
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
    configuration: Configuration,
    snapshot: Option<Snapshot>,
    visible: Option<IndicatorSurface>,
    epoch: Instant,
    motion: Option<Motion>,
    failure: Option<String>,
    exit: bool,
}

struct IndicatorSurface {
    layer: LayerSurface,
    output: wl_output::WlOutput,
    configured: bool,
    scale: i32,
    buffers: Vec<Buffer>,
    dirty: bool,
    frame_pending: bool,
}

impl App {
    fn update_snapshot(&mut self, snapshot: Snapshot, handle: &QueueHandle<Self>) {
        let center = self.configuration.active_center(snapshot.active);
        let now = self.epoch.elapsed();
        match center {
            Some(center) => match &mut self.motion {
                Some(motion) => motion.retarget(center, now),
                None => {
                    self.motion = Some(Motion::new(
                        center,
                        Duration::from_millis(self.configuration.animation_ms),
                    ));
                }
            },
            None => self.motion = None,
        }
        self.snapshot = Some(snapshot);
        self.refresh(handle);
    }

    fn refresh(&mut self, handle: &QueueHandle<Self>) {
        if let Err(error) = self.refresh_surface(handle) {
            self.failure = Some(error.to_string());
        }
        if let Some(visible) = &mut self.visible {
            visible.dirty = true;
        }
    }

    fn refresh_surface(&mut self, handle: &QueueHandle<Self>) -> Result<()> {
        let Some(snapshot) = &self.snapshot else {
            return Ok(());
        };
        let output = self.outputs.outputs().find(|output| {
            self.outputs
                .info(output)
                .is_some_and(|info| info.name.as_deref() == Some(&snapshot.monitor))
        });
        let Some(output) = output else {
            self.visible = None;
            return Ok(());
        };
        if self
            .visible
            .as_ref()
            .is_some_and(|visible| visible.output == output)
        {
            return Ok(());
        }
        self.visible = None;
        let scale = self
            .outputs
            .info(&output)
            .map_or(1, |info| info.scale_factor);
        if !(1..=4).contains(&scale) {
            return Err("output scale must be between 1 and 4".into());
        }
        let surface = self.compositor.create_surface(handle);
        let region = Region::new(&self.compositor)?;
        surface.set_input_region(Some(region.wl_region()));
        let layer = self.shell.create_layer_surface(
            handle,
            surface,
            Layer::Overlay,
            Some("workspace-indicator"),
            Some(&output),
        );
        layer.set_anchor(Anchor::TOP);
        layer.set_margin(self.configuration.margin_top, 0, 0, 0);
        // Ignore Waybar's exclusive zone: the overlay shares its row, not the desktop below it.
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_size(self.configuration.width(), self.configuration.height);
        layer.commit();
        self.visible = Some(IndicatorSurface {
            layer,
            output,
            configured: false,
            scale,
            buffers: Vec::with_capacity(2),
            dirty: true,
            frame_pending: false,
        });
        Ok(())
    }

    fn draw(&mut self, handle: &QueueHandle<Self>) -> Result<()> {
        let Some(visible) = &mut self.visible else {
            return Ok(());
        };
        if !visible.configured {
            return Ok(());
        }
        if !visible.dirty {
            return Ok(());
        }
        if visible.frame_pending {
            return Ok(());
        }
        let width = self.configuration.width() as i32 * visible.scale;
        let height = self.configuration.height as i32 * visible.scale;
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
                // Fail rather than allow growth under pathological buffer-release delays.
                if self.pool.len() > BUFFER_POOL_SIZE_BYTES {
                    return Err("render buffer pool exhausted".into());
                }
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
        let now = self.epoch.elapsed();
        model::render(
            canvas,
            visible.scale as u32,
            self.motion.as_ref().map(|motion| motion.sample(now)),
            &self.configuration,
            snapshot,
        );
        let surface = visible.layer.wl_surface();
        surface.set_buffer_scale(visible.scale);
        surface.damage_buffer(0, 0, width, height);
        buffer.attach_to(surface)?;
        if self
            .motion
            .as_ref()
            .is_some_and(|motion| motion.running(now))
        {
            surface.frame(handle, surface.clone());
            visible.frame_pending = true;
        }
        visible.dirty = false;
        visible.layer.commit();
        Ok(())
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        factor: i32,
    ) {
        let Some(visible) = self
            .visible
            .as_mut()
            .filter(|visible| visible.layer.wl_surface() == surface)
        else {
            return;
        };
        if !(1..=4).contains(&factor) {
            self.failure = Some("output scale must be between 1 and 4".into());
            return;
        }
        if visible.scale != factor {
            visible.scale = factor;
            visible.buffers.clear();
            visible.dirty = true;
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
    fn frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        if let Some(visible) = self
            .visible
            .as_mut()
            .filter(|visible| visible.layer.wl_surface() == surface)
        {
            visible.frame_pending = false;
            visible.dirty = true;
        }
    }
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
    fn new_output(&mut self, _: &Connection, handle: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.refresh(handle);
    }
    fn update_output(
        &mut self,
        _: &Connection,
        handle: &QueueHandle<Self>,
        _: wl_output::WlOutput,
    ) {
        self.refresh(handle);
    }
    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        if self
            .visible
            .as_ref()
            .is_some_and(|visible| visible.output == output)
        {
            self.visible = None;
        }
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
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let Some(visible) = self
            .visible
            .as_mut()
            .filter(|visible| &visible.layer == layer)
        else {
            return;
        };
        let (width, height) = configure.new_size;
        if ![0, self.configuration.width()].contains(&width) {
            self.failure = Some("compositor rejected indicator width".into());
            return;
        }
        if ![0, self.configuration.height].contains(&height) {
            self.failure = Some("compositor rejected indicator height".into());
            return;
        }
        visible.configured = true;
        visible.dirty = true;
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
