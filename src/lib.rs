mod audio;
mod control;
mod gamepad;
mod gpu;
mod host;
mod loader;
mod mcp;
mod runtime;
mod runtime_worker;
pub mod storage;
mod web_assets;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Wake, Waker};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use clap::Parser;
use host::SharedHost;
use runtime_worker::{Runtime, RuntimeConfig};
use serde_json::{Value, json};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};

const MAX_FRAME_SAMPLES: usize = 8192;

/// Defaults supplied by a native game host. CLI storage options can override
/// these without changing how native extensions or the rendering host run.
pub struct ApplicationOptions {
    pub extension_factory: fn() -> Vec<deno_core::Extension>,
    pub storage_namespace: Option<&'static str>,
    pub legacy_storage_namespace: Option<&'static str>,
}

impl Default for ApplicationOptions {
    fn default() -> Self {
        Self {
            extension_factory: Vec::new,
            storage_namespace: None,
            legacy_storage_namespace: None,
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "peregrust",
    version,
    about = "Native TypeScript game runtime",
    after_help = "Agent control: peregrust ctl --session SESSION control.describe\nMCP stdio: peregrust mcp --session SESSION\nUse peregrust ctl --help or peregrust mcp --help for options."
)]
struct Cli {
    /// Entry JavaScript or TypeScript module, relative to --root.
    entry: PathBuf,
    /// Root directory for modules and assets.
    #[arg(long, default_value = ".")]
    root: PathBuf,
    /// Override the per-user persistent storage directory.
    #[arg(long)]
    storage_dir: Option<PathBuf>,
    /// Stable game identifier for persistent saves across updates.
    #[arg(long)]
    app_id: Option<String>,
    /// Read-only fallback namespace for importing existing Mystral saves.
    #[arg(long)]
    legacy_storage_namespace: Option<String>,
    /// Project-local JSON import map.
    #[arg(long)]
    import_map: Option<PathBuf>,
    #[arg(long, default_value = "Peregrust")]
    title: String,
    /// Initial logical client width.
    #[arg(long, default_value_t = 1280)]
    width: u32,
    /// Initial logical client height.
    #[arg(long, default_value_t = 720)]
    height: u32,
    #[arg(long)]
    fullscreen: bool,
    #[arg(long)]
    hidden: bool,
    /// Maximum frames per second.
    #[arg(long)]
    fps: Option<f64>,
    /// Exit after this many completed frame callbacks, including frames without drawing.
    #[arg(long)]
    frames: Option<u64>,
    /// Exit after this many frames have actually been presented to the window.
    #[arg(long)]
    presented_frames: Option<u64>,
    /// Maximum runtime in seconds; exits with code 124 on timeout.
    #[arg(long)]
    timeout: Option<f64>,
    /// V8 heap limit in MiB (defaults to V8's own limit).
    #[arg(long)]
    heap_mb: Option<u64>,
    /// Write run statistics as JSON on exit.
    #[arg(long)]
    stats: Option<PathBuf>,
    /// Enable local agent control and write connection details to this new file.
    #[arg(long)]
    control: Option<PathBuf>,
    /// Arguments exposed to the game as Peregrust.args.
    #[arg(last = true)]
    game_args: Vec<String>,
}

impl Cli {
    fn validate(&self) -> Result<()> {
        if self.width == 0 || self.height == 0 || self.width > 16_384 || self.height > 16_384 {
            bail!("--width and --height must be between 1 and 16384");
        }
        if self.fps.is_some_and(|fps| !fps.is_finite() || fps <= 0.0) {
            bail!("--fps must be a positive finite number");
        }
        if self.fps.is_some_and(|fps| fps < 0.01) {
            bail!("--fps must be at least 0.01");
        }
        if self.fps.is_some_and(|fps| fps > 1000.0) {
            bail!("--fps cannot exceed 1000");
        }
        if self.frames == Some(0) {
            bail!("--frames must be positive");
        }
        if self.presented_frames == Some(0) {
            bail!("--presented-frames must be positive");
        }
        if self
            .timeout
            .is_some_and(|seconds| !seconds.is_finite() || seconds <= 0.0)
        {
            bail!("--timeout must be a positive finite number of seconds");
        }
        if self.timeout.is_some_and(|seconds| seconds < 0.001) {
            bail!("--timeout must be at least 0.001 seconds");
        }
        if self.timeout.is_some_and(|seconds| seconds > 31_536_000.0) {
            bail!("--timeout cannot exceed one year");
        }
        if self.heap_mb.is_some_and(|mb| mb < 64) {
            bail!("--heap-mb must be at least 64");
        }
        Ok(())
    }
}

enum UserEvent {
    Wake,
    CreateSurface {
        instance: deno_webgpu::Instance,
        reply: std::sync::mpsc::Sender<Result<deno_webgpu::wgpu_core::id::SurfaceId, String>>,
    },
}

struct RuntimeWake {
    proxy: EventLoopProxy<UserEvent>,
    queued: AtomicBool,
}

impl RuntimeWake {
    fn notify(&self) {
        if !self.queued.swap(true, Ordering::AcqRel) {
            let _ = self.proxy.send_event(UserEvent::Wake);
        }
    }
}

impl Wake for RuntimeWake {
    fn wake(self: Arc<Self>) {
        self.notify();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.notify();
    }
}

struct App {
    cli: Cli,
    extension_factory: fn() -> Vec<deno_core::Extension>,
    project_root: PathBuf,
    window: Option<Arc<Window>>,
    host: Option<SharedHost>,
    runtime: Option<Runtime>,
    wake: Arc<RuntimeWake>,
    waker: Waker,
    start: Instant,
    deadline: Option<Instant>,
    next_frame_at: Instant,
    frame_interval: Option<Duration>,
    redraw_latched: bool,
    occluded: bool,
    modifiers: ModifiersState,
    pointer_position: (f64, f64),
    buttons: u16,
    frames_completed: u64,
    frames_presented: u64,
    first_completed: Option<Instant>,
    last_completed: Option<Instant>,
    frame_samples_ms: VecDeque<f64>,
    failure: Option<anyhow::Error>,
    exit_code: i32,
}

impl App {
    fn new(
        cli: Cli,
        project_root: PathBuf,
        proxy: EventLoopProxy<UserEvent>,
        extension_factory: fn() -> Vec<deno_core::Extension>,
    ) -> Self {
        let start = Instant::now();
        let deadline = cli
            .timeout
            .map(|seconds| start + Duration::from_secs_f64(seconds));
        let frame_interval = cli
            .fps
            .map(|fps| Duration::from_secs_f64(1.0 / fps).max(Duration::from_millis(1)));
        let wake = Arc::new(RuntimeWake {
            proxy,
            queued: AtomicBool::new(false),
        });
        let waker = Waker::from(wake.clone());
        Self {
            cli,
            extension_factory,
            project_root,
            window: None,
            host: None,
            runtime: None,
            wake,
            waker,
            start,
            deadline,
            next_frame_at: start,
            frame_interval,
            redraw_latched: true,
            occluded: false,
            modifiers: ModifiersState::empty(),
            pointer_position: (0.0, 0.0),
            buttons: 0,
            frames_completed: 0,
            frames_presented: 0,
            first_completed: None,
            last_completed: None,
            frame_samples_ms: VecDeque::with_capacity(MAX_FRAME_SAMPLES),
            failure: None,
            exit_code: 0,
        }
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: anyhow::Error) {
        if self.failure.is_none() {
            self.failure = Some(error);
        }
        self.exit_code = 1;
        event_loop.exit();
    }

    fn dispatch(&mut self, event_loop: &ActiveEventLoop, event: Value) {
        if let Some(runtime) = self.runtime.as_mut()
            && let Err(error) = runtime.dispatch_event(&event)
        {
            self.fail(event_loop, error);
        }
    }

    fn modifier_fields(&self) -> Value {
        json!({
            "altKey": self.modifiers.alt_key(),
            "ctrlKey": self.modifiers.control_key(),
            "metaKey": self.modifiers.super_key(),
            "shiftKey": self.modifiers.shift_key(),
        })
    }

    fn pointer_fields(&self, button: u8, movement: (f64, f64)) -> Value {
        let mut event = json!({
            "pointerId": 1, "pointerType": "mouse", "isPrimary": true,
            "clientX": self.pointer_position.0, "clientY": self.pointer_position.1,
            "movementX": movement.0, "movementY": movement.1,
            "button": button, "buttons": self.buttons,
        });
        event
            .as_object_mut()
            .unwrap()
            .extend(self.modifier_fields().as_object().unwrap().clone());
        event
    }

    fn scale_factor(&self) -> f64 {
        self.host
            .as_ref()
            .map_or(1.0, SharedHost::scale_factor)
            .max(0.01)
    }

    fn tick(&mut self, event_loop: &ActiveEventLoop) {
        let result = match self.runtime.as_mut() {
            Some(runtime) => runtime.tick(&self.waker),
            None => return,
        };
        match result {
            Ok(poll) => {
                if poll.frame_completed {
                    self.on_frame_completed(event_loop);
                }
                if poll.frame_presented {
                    self.frames_presented += 1;
                } else if poll.frame_completed && self.frame_interval.is_none() {
                    // A RAF loop waiting for assets has no swapchain/vsync to
                    // pace it. Avoid spinning thousands of empty frames.
                    self.next_frame_at = Instant::now() + Duration::from_millis(16);
                }
            }
            Err(error) => self.fail(event_loop, error),
        }
    }

    fn on_frame_completed(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if self.first_completed.is_none() {
            self.first_completed = Some(now);
        }
        if let Some(last) = self.last_completed {
            if self.frame_samples_ms.len() == MAX_FRAME_SAMPLES {
                self.frame_samples_ms.pop_front();
            }
            self.frame_samples_ms
                .push_back((now - last).as_secs_f64() * 1000.0);
        }
        self.last_completed = Some(now);
        self.frames_completed += 1;
        if self
            .cli
            .frames
            .is_some_and(|limit| self.frames_completed >= limit)
        {
            event_loop.exit();
        }
    }

    fn check_exit(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if self
            .cli
            .frames
            .is_some_and(|limit| self.frames_completed >= limit)
            || self
                .cli
                .presented_frames
                .is_some_and(|limit| self.frames_presented >= limit)
        {
            event_loop.exit();
            return true;
        }
        if let Some(code) = self.host.as_ref().and_then(SharedHost::exit_code) {
            self.exit_code = code;
            event_loop.exit();
            return true;
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.exit_code = 124;
            self.failure = Some(anyhow!("game exceeded --timeout"));
            event_loop.exit();
            return true;
        }
        false
    }

    fn schedule(&mut self, event_loop: &ActiveEventLoop) {
        if self.check_exit(event_loop) {
            return;
        }
        self.tick(event_loop);
        if self.failure.is_some() || self.check_exit(event_loop) {
            return;
        }
        if let Some(host) = &self.host {
            self.redraw_latched |= host.take_redraw_requested();
        }
        let can_draw = self.redraw_latched
            && (!self.occluded || self.cli.hidden)
            && self.host.as_ref().is_some_and(|host| {
                let size = host.physical_size();
                size.width > 0 && size.height > 0
            })
            && self.runtime.as_ref().is_some_and(Runtime::is_ready);
        let pending = self.runtime.as_ref().is_some_and(Runtime::frame_pending);
        if can_draw && !pending {
            if Instant::now() >= self.next_frame_at {
                if self.cli.hidden {
                    // Hidden windows need a software frame trigger: some
                    // platforms never send RedrawRequested for them.
                    let timestamp_ms = self.start.elapsed().as_secs_f64() * 1000.0;
                    let started = match self.runtime.as_mut().unwrap().dispatch_frame(timestamp_ms)
                    {
                        Ok(started) => started,
                        Err(error) => return self.fail(event_loop, error),
                    };
                    if started {
                        self.redraw_latched = false;
                        if let Some(interval) = self.frame_interval {
                            self.next_frame_at = Instant::now() + interval;
                        }
                        self.tick(event_loop);
                        if self.failure.is_some() || self.check_exit(event_loop) {
                            return;
                        }
                        if let Some(host) = &self.host {
                            self.redraw_latched |= host.take_redraw_requested();
                        }
                    }
                } else if let Some(window) = &self.window {
                    window.request_redraw();
                }
            } else {
                let wake_at = self.deadline.map_or(self.next_frame_at, |deadline| {
                    deadline.min(self.next_frame_at)
                });
                event_loop.set_control_flow(ControlFlow::WaitUntil(wake_at));
                return;
            }
        }
        if self.cli.hidden
            && self.redraw_latched
            && self.runtime.as_ref().is_some_and(Runtime::is_ready)
            && !self.runtime.as_ref().is_some_and(Runtime::frame_pending)
        {
            let wake_at = self.deadline.map_or(self.next_frame_at, |deadline| {
                deadline.min(self.next_frame_at)
            });
            event_loop.set_control_flow(ControlFlow::WaitUntil(wake_at));
            return;
        }
        event_loop.set_control_flow(match self.deadline {
            Some(deadline) => ControlFlow::WaitUntil(deadline),
            None => ControlFlow::Wait,
        });
    }

    fn write_stats(&self) -> Result<()> {
        let Some(path) = &self.cli.stats else {
            return Ok(());
        };
        let elapsed = self.start.elapsed().as_secs_f64();
        let active_seconds = match (self.first_completed, self.last_completed) {
            (Some(first), Some(last)) => (last - first).as_secs_f64(),
            _ => 0.0,
        };
        let mut samples: Vec<f64> = self.frame_samples_ms.iter().copied().collect();
        samples.sort_by(f64::total_cmp);
        let percentile = |p: f64| -> Option<f64> {
            if samples.is_empty() {
                None
            } else {
                let index = ((samples.len() - 1) as f64 * p).round() as usize;
                Some(samples[index])
            }
        };
        let stats = json!({
            "frames": self.frames_completed,
            "presentedFrames": self.frames_presented,
            "elapsedSeconds": elapsed,
            "activeSeconds": active_seconds,
            "averageFps": if active_seconds > 0.0 { (self.frames_completed - 1) as f64 / active_seconds } else { 0.0 },
            "frameTimeMs": { "p50": percentile(0.50), "p95": percentile(0.95), "p99": percentile(0.99), "samples": samples.len() },
            "exitCode": self.exit_code,
        });
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(&stats)?)
            .with_context(|| format!("writing {}", path.display()))
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = WindowAttributes::default()
            .with_title(self.cli.title.clone())
            .with_inner_size(LogicalSize::new(
                self.cli.width as f64,
                self.cli.height as f64,
            ))
            .with_visible(!self.cli.hidden);
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                return self.fail(event_loop, anyhow!(error).context("creating native window"));
            }
        };
        let host = match SharedHost::new(window.clone(), &self.project_root) {
            Ok(host) => host,
            Err(error) => {
                return self.fail(
                    event_loop,
                    anyhow!(error).context("initializing native host"),
                );
            }
        };
        host.set_title(self.cli.title.clone());
        host.set_waker(self.waker.clone());
        let surface_proxy = self.wake.proxy.clone();
        host.set_surface_factory(Arc::new(move |instance| {
            let (reply, receive) = std::sync::mpsc::channel();
            surface_proxy
                .send_event(UserEvent::CreateSurface { instance, reply })
                .map_err(|_| deno_error::JsErrorBox::generic("native event loop has closed"))?;
            receive
                .recv_timeout(Duration::from_secs(5))
                .map_err(|error| {
                    deno_error::JsErrorBox::generic(format!(
                        "native surface creation failed: {error}"
                    ))
                })?
                .map_err(deno_error::JsErrorBox::generic)
        }));
        host.set_args(self.cli.game_args.clone());
        host.set_visible(!self.cli.hidden);
        if self.cli.fullscreen {
            host.set_fullscreen(true);
        }
        let max_heap_bytes = match self.cli.heap_mb {
            Some(mb) => match usize::try_from(mb)
                .ok()
                .and_then(|value| value.checked_mul(1024 * 1024))
            {
                Some(bytes) => Some(bytes),
                None => return self.fail(event_loop, anyhow!("--heap-mb exceeds address space")),
            },
            None => None,
        };
        let mut runtime = match Runtime::new(
            RuntimeConfig {
                extension_factory: self.extension_factory,
                storage_dir: self.cli.storage_dir.clone(),
                storage_namespace: self.cli.app_id.clone(),
                legacy_storage_namespace: self.cli.legacy_storage_namespace.clone(),
                project_root: self.project_root.clone(),
                import_map: self.cli.import_map.clone(),
                host: host.clone(),
                execution_timeout: self.cli.timeout.map(Duration::from_secs_f64),
                max_heap_bytes,
                control_file: self.cli.control.clone(),
            },
            self.waker.clone(),
        ) {
            Ok(runtime) => runtime,
            Err(error) => return self.fail(event_loop, error.context("initializing game runtime")),
        };
        if let Err(error) = runtime.start_entry(&self.cli.entry) {
            return self.fail(event_loop, error.context("starting game module"));
        }
        self.host = Some(host);
        self.window = Some(window);
        self.runtime = Some(runtime);
        self.schedule(event_loop);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Wake => self.wake.queued.store(false, Ordering::Release),
            UserEvent::CreateSurface { instance, reply } => {
                let result = self
                    .window
                    .as_ref()
                    .ok_or_else(|| "native window is unavailable".to_string())
                    .and_then(|window| gpu::create_surface_on_main(window, instance));
                let _ = reply.send(result);
            }
        }
        self.schedule(event_loop);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self
            .window
            .as_ref()
            .is_none_or(|window| window.id() != window_id)
        {
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                self.dispatch(event_loop, json!({"type": "close"}));
                event_loop.exit();
                return;
            }
            WindowEvent::Resized(size) => {
                if let Some(host) = &self.host {
                    host.set_physical_size(size);
                }
                if let Some(runtime) = self.runtime.as_mut()
                    && let Err(error) = runtime.resize_surface(size.width, size.height)
                {
                    return self.fail(event_loop, error);
                }
                self.dispatch(event_loop, json!({"type":"resize", "width":size.width, "height":size.height, "devicePixelRatio":self.scale_factor()}));
                if let Some(host) = &self.host {
                    host.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(host) = &self.host {
                    host.set_scale_factor(scale_factor);
                }
                let size = self.window.as_ref().unwrap().inner_size();
                if let Some(host) = &self.host {
                    host.set_physical_size(size);
                }
                if let Some(runtime) = self.runtime.as_mut()
                    && let Err(error) = runtime.resize_surface(size.width, size.height)
                {
                    return self.fail(event_loop, error);
                }
                self.dispatch(event_loop, json!({"type":"resize", "width":size.width, "height":size.height, "devicePixelRatio":scale_factor}));
                if let Some(host) = &self.host {
                    host.request_redraw();
                }
            }
            WindowEvent::Focused(focused) => {
                if !focused {
                    if self.buttons != 0 {
                        self.buttons = 0;
                        let mut cancel = self.pointer_fields(0, (0.0, 0.0));
                        cancel["type"] = json!("pointercancel");
                        self.dispatch(event_loop, cancel);
                    }
                    self.modifiers = ModifiersState::empty();
                }
                if let Some(host) = &self.host {
                    host.set_focused(focused);
                }
                self.dispatch(
                    event_loop,
                    json!({"type": if focused {"focus"} else {"blur"}}),
                );
                if focused && let Some(host) = &self.host {
                    host.request_redraw();
                }
            }
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                if let Some(host) = &self.host {
                    host.set_visible(!occluded && !self.cli.hidden);
                }
                if !occluded && let Some(host) = &self.host {
                    host.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::CursorMoved { position, .. } => {
                if self
                    .host
                    .as_ref()
                    .is_some_and(|host| host.state().pointer_capture == "locked")
                {
                    return;
                }
                let dpr = self.scale_factor();
                let next = (position.x / dpr, position.y / dpr);
                let movement = (
                    next.0 - self.pointer_position.0,
                    next.1 - self.pointer_position.1,
                );
                self.pointer_position = next;
                let mut data = self.pointer_fields(0, movement);
                data["type"] = json!("pointermove");
                self.dispatch(event_loop, data);
            }
            WindowEvent::CursorLeft { .. } => {
                let mut data = self.pointer_fields(0, (0.0, 0.0));
                data["type"] = json!("pointerleave");
                self.dispatch(event_loop, data);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let (dom_button, mask) = mouse_button(button);
                if state == ElementState::Pressed {
                    self.buttons |= mask;
                } else {
                    self.buttons &= !mask;
                }
                let mut data = self.pointer_fields(dom_button, (0.0, 0.0));
                data["type"] = json!(if state == ElementState::Pressed {
                    "pointerdown"
                } else {
                    "pointerup"
                });
                self.dispatch(event_loop, data);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dpr = self.scale_factor();
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (-(x as f64) * 16.0, -(y as f64) * 16.0),
                    MouseScrollDelta::PixelDelta(position) => {
                        (-position.x / dpr, -position.y / dpr)
                    }
                };
                let mut data = self.pointer_fields(0, (0.0, 0.0));
                data["type"] = json!("wheel");
                data["deltaX"] = json!(dx);
                data["deltaY"] = json!(dy);
                data["deltaMode"] = json!(0);
                self.dispatch(event_loop, data);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let key = match &event.logical_key {
                    Key::Character(text) => text.to_string(),
                    Key::Named(winit::keyboard::NamedKey::Space) => " ".to_string(),
                    Key::Named(named) => format!("{named:?}"),
                    Key::Unidentified(_) => "Unidentified".to_string(),
                    Key::Dead(_) => "Dead".to_string(),
                };
                let code = match event.physical_key {
                    PhysicalKey::Code(code) => format!("{code:?}"),
                    PhysicalKey::Unidentified(_) => "Unidentified".to_string(),
                };
                let mut data = self.modifier_fields();
                data["type"] = json!(if event.state == ElementState::Pressed {
                    "keydown"
                } else {
                    "keyup"
                });
                data["key"] = json!(key);
                data["code"] = json!(code);
                data["repeat"] = json!(event.repeat);
                self.dispatch(event_loop, data);
            }
            WindowEvent::RedrawRequested => {
                if let Some(host) = &self.host {
                    self.redraw_latched |= host.take_redraw_requested();
                }
                if !self.redraw_latched || self.occluded || Instant::now() < self.next_frame_at {
                    return self.schedule(event_loop);
                }
                let Some(runtime) = self.runtime.as_mut() else {
                    return;
                };
                let timestamp_ms = self.start.elapsed().as_secs_f64() * 1000.0;
                match runtime.dispatch_frame(timestamp_ms) {
                    Ok(true) => {
                        self.redraw_latched = false;
                        if let Some(interval) = self.frame_interval {
                            self.next_frame_at = Instant::now() + interval;
                        }
                    }
                    Ok(false) => {}
                    Err(error) => return self.fail(event_loop, error),
                }
                self.schedule(event_loop);
                return;
            }
            _ => {}
        }
        self.schedule(event_loop);
    }

    fn device_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        if let DeviceEvent::MouseMotion { delta } = event {
            let locked = self
                .host
                .as_ref()
                .is_some_and(|host| host.state().pointer_capture == "locked");
            if locked {
                let dpr = self.scale_factor();
                let mut data = self.pointer_fields(0, (delta.0 / dpr, delta.1 / dpr));
                data["type"] = json!("pointerrawupdate");
                self.dispatch(event_loop, data.clone());
                data["type"] = json!("pointermove");
                self.dispatch(event_loop, data);
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.schedule(event_loop);
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(runtime) = self.runtime.take()
            && let Err(error) = runtime.shutdown()
        {
            self.exit_code = 1;
            if self.failure.is_none() {
                self.failure = Some(error);
            }
        }
        self.host.take();
        self.window.take();
    }
}

fn mouse_button(button: MouseButton) -> (u8, u16) {
    match button {
        MouseButton::Left => (0, 1),
        MouseButton::Middle => (1, 4),
        MouseButton::Right => (2, 2),
        MouseButton::Back => (3, 8),
        MouseButton::Forward => (4, 16),
        MouseButton::Other(value) => (value.min(u8::MAX as u16) as u8, 0),
    }
}

/// Run the desktop executable with the standard Peregrust APIs.
pub fn run() -> Result<i32> {
    run_with_extensions(Vec::new)
}

/// Run the same native window and V8 host with statically linked game extensions.
/// The factory executes on the JavaScript thread; extension state never crosses
/// threads. This lets a game link its native simulation without another V8 host.
pub fn run_with_extensions(extension_factory: fn() -> Vec<deno_core::Extension>) -> Result<i32> {
    run_with_options(ApplicationOptions {
        extension_factory,
        ..Default::default()
    })
}

/// Run a statically linked game host with stable save storage defaults.
pub fn run_with_options(options: ApplicationOptions) -> Result<i32> {
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "mcp") {
        return mcp::run();
    }
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "ctl") {
        return Ok(control::run_cli());
    }
    let mut cli = Cli::parse();
    if cli.app_id.is_none() {
        cli.app_id = options.storage_namespace.map(str::to_owned);
    }
    if cli.legacy_storage_namespace.is_none() {
        cli.legacy_storage_namespace = options.legacy_storage_namespace.map(str::to_owned);
    }
    cli.validate()?;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
    let project_root = std::fs::canonicalize(&cli.root)
        .with_context(|| format!("opening project root {}", cli.root.display()))?;
    if !project_root.is_dir() {
        bail!(
            "project root is not a directory: {}",
            project_root.display()
        );
    }
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .context("creating native event loop")?;
    let mut app = App::new(
        cli,
        project_root,
        event_loop.create_proxy(),
        options.extension_factory,
    );
    event_loop
        .run_app(&mut app)
        .context("running native event loop")?;
    if let Err(error) = app.write_stats() {
        eprintln!("Could not write run statistics: {error:#}");
        if app.exit_code == 0 {
            app.exit_code = 1;
        }
    }
    if let Some(error) = app.failure {
        eprintln!("Peregrust: {error:#}");
    }
    Ok(app.exit_code)
}
