//! Single-threaded V8 runtime driven by the native window event loop.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Waker};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use deno_core::error::{CoreError, JsError};
use deno_core::{JsRuntime, ModuleSpecifier, PollEventLoopOptions, RuntimeOptions, v8};
use serde_json::Value;

use crate::host::SharedHost;
use crate::loader::ProjectModuleLoader;

const WEB_BOOTSTRAP: &str = r#"
(() => {
  const core = Deno.core;
  const { performance } = core.loadExtScript('ext:deno_web/15_performance.js');
  const timers = core.loadExtScript('ext:deno_web/02_timers.js');
  const encoding = core.loadExtScript('ext:deno_web/08_text_encoding.js');
  const url = core.loadExtScript('ext:deno_web/00_url.js');
  const dom = core.loadExtScript('ext:deno_web/01_dom_exception.js');
  const events = core.loadExtScript('ext:deno_web/02_event.js');
  const abort = core.loadExtScript('ext:deno_web/03_abort_signal.js');
  const file = core.loadExtScript('ext:deno_web/09_file.js');
  const base64 = core.loadExtScript('ext:deno_web/05_base64.js');
  const { Console } = core.loadExtScript('ext:deno_web/01_console.js');
  globalThis.console = new Console((message, level) => {
    globalThis.__peregrustRecordLog?.(message, level);
    core.print(message, level >= 3);
  });
  globalThis.performance = performance;
  globalThis.setTimeout = timers.setTimeout;
  globalThis.clearTimeout = timers.clearTimeout;
  globalThis.setInterval = timers.setInterval;
  globalThis.clearInterval = timers.clearInterval;
  globalThis.TextEncoder = encoding.TextEncoder;
  globalThis.TextDecoder = encoding.TextDecoder;
  globalThis.URL = url.URL;
  globalThis.URLSearchParams = url.URLSearchParams;
  globalThis.DOMException = dom.DOMException;
  globalThis.Event = events.Event;
  globalThis.EventTarget = events.EventTarget;
  globalThis.CustomEvent = events.CustomEvent;
  globalThis.AbortController = abort.AbortController;
  globalThis.AbortSignal = abort.AbortSignal;
  globalThis.Blob = file.Blob;
  globalThis.File = file.File;
  globalThis.atob = base64.atob;
  globalThis.btoa = base64.btoa;
  const webgpu = core.loadExtScript('ext:deno_webgpu/00_init.js').loadWebGPU();
  webgpu.initGPU();
  for (const [name, value] of Object.entries(webgpu)) {
    if (name.startsWith('GPU') || name === 'WGSLLanguageFeatures') {
      globalThis[name] = value;
    }
  }
  globalThis.navigator ??= {};
  Object.defineProperty(globalThis.navigator, 'gpu', {
    configurable: false, enumerable: true, get: () => webgpu.gpu,
  });
})();
"#;

pub struct RuntimeConfig {
    pub control_file: Option<PathBuf>,
    pub extension_factory: fn() -> Vec<deno_core::Extension>,
    pub storage_dir: Option<PathBuf>,
    pub storage_namespace: Option<String>,
    pub legacy_storage_namespace: Option<String>,
    pub project_root: PathBuf,
    pub import_map: Option<PathBuf>,
    pub host: SharedHost,
    pub execution_timeout: Option<Duration>,
    pub max_heap_bytes: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeStatus {
    Created,
    Evaluating,
    Ready,
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimePoll {
    /// A frame callback settled, including a recoverable surface failure.
    pub frame_finished: bool,
    pub frame_completed: bool,
    pub frame_presented: bool,
}

type ModuleEvaluation = Pin<Box<dyn Future<Output = Result<(), CoreError>>>>;
type FrameEvaluation = Pin<Box<dyn Future<Output = Result<v8::Global<v8::Value>, Box<JsError>>>>>;

pub struct Runtime {
    control_server: Option<crate::control::ControlServer>,
    // Watchdog must stop before any V8 handles or the isolate are destroyed.
    watchdog: Option<ExecutionWatchdog>,
    loader: Rc<ProjectModuleLoader>,
    status: RuntimeStatus,
    module_evaluation: Option<ModuleEvaluation>,
    dispatch_event_fn: v8::Global<v8::Function>,
    dispatch_frame_fn: v8::Global<v8::Function>,
    frame_future: Option<FrameEvaluation>,
    pending_resize: Option<(u32, u32)>,
    // JsRuntime must be the last field so V8 globals/futures drop first.
    js: JsRuntime,
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.control_server.take();
        self.watchdog.take();
        self.module_evaluation.take();
        self.frame_future.take();
        if let Err(error) = crate::gpu::shutdown(&mut self.js) {
            eprintln!("WebGPU shutdown failed: {error}");
        }
    }
}

struct ExecutionWatchdog {
    stop: Arc<(Mutex<bool>, Condvar)>,
    expired: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ExecutionWatchdog {
    fn start(js: &mut JsRuntime, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() {
            bail!("execution timeout must be greater than zero");
        }
        let isolate = js.v8_isolate().thread_safe_handle();
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let expired = Arc::new(AtomicBool::new(false));
        let wait_stop = Arc::clone(&stop);
        let wait_expired = Arc::clone(&expired);
        let thread = std::thread::Builder::new()
            .name("peregrust-v8-watchdog".into())
            .spawn(move || {
                let (lock, wake) = &*wait_stop;
                let guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let (stopped, result) = wake
                    .wait_timeout_while(guard, timeout, |stopped| !*stopped)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if !*stopped && result.timed_out() {
                    wait_expired.store(true, Ordering::SeqCst);
                    isolate.terminate_execution();
                }
            })
            .context("starting V8 execution watchdog")?;
        Ok(Self {
            stop,
            expired,
            thread: Some(thread),
        })
    }

    fn expired(&self) -> bool {
        self.expired.load(Ordering::SeqCst)
    }
}

impl Drop for ExecutionWatchdog {
    fn drop(&mut self) {
        let (lock, wake) = &*self.stop;
        *lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        wake.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Runtime {
    pub fn new(config: RuntimeConfig) -> Result<Self> {
        if config
            .max_heap_bytes
            .is_some_and(|bytes| bytes < 16 * 1024 * 1024)
        {
            bail!("V8 heap limit must be at least 16 MiB");
        }
        let mut loader = ProjectModuleLoader::new(&config.project_root)
            .map_err(|error| anyhow!(error.to_string()))?;
        if let Some(path) = &config.import_map {
            loader
                .read_import_map(path)
                .map_err(|error| anyhow!(error.to_string()))?;
        }
        let loader = Rc::new(loader);
        let create_params = config
            .max_heap_bytes
            .map(|bytes| v8::Isolate::create_params().heap_limits(0, bytes));
        let mut extensions = vec![
            deno_webidl::deno_webidl::init(),
            deno_web::deno_web::init(
                deno_web::BlobStore::default_arc(),
                None,
                false,
                Default::default(),
            ),
            deno_webgpu::deno_webgpu::init(),
            crate::gpu_upload::peregrust_gpu_upload::init(),
            crate::gpu_mapping::peregrust_gpu_mapping::init(),
            deno_image::deno_image::init(),
            crate::host::peregrust_host::init(),
            crate::web_assets::peregrust_web_assets::init(),
            crate::audio::peregrust_audio::init(),
            crate::gamepad::peregrust_gamepad::init(),
            crate::storage::peregrust_storage::init(),
            crate::control::peregrust_control::init(),
        ];
        extensions.extend((config.extension_factory)());
        let mut js = JsRuntime::try_new(RuntimeOptions {
            module_loader: Some(loader.clone()),
            create_params,
            extensions,
            ..Default::default()
        })
        .context("initializing V8 and WebGPU extensions")?;
        let watchdog = config
            .execution_timeout
            .map(|timeout| ExecutionWatchdog::start(&mut js, timeout))
            .transpose()?;
        let (control_server, control_state) = crate::control::ControlServer::start(
            config.control_file.as_deref(),
            config.host.clone(),
        )?;
        js.op_state().borrow_mut().put(control_state);
        js.op_state().borrow_mut().put(config.host);
        js.op_state()
            .borrow_mut()
            .put(crate::storage::StorageBackend::open(
                &config.project_root,
                config.storage_dir.as_deref(),
                config.storage_namespace.as_deref(),
                config.legacy_storage_namespace.as_deref(),
            )?);
        js.execute_script("peregrust:web-bootstrap", WEB_BOOTSTRAP)
            .context("initializing browser primitives and WebGPU")?;
        js.execute_script("peregrust:bootstrap", include_str!("../js/bootstrap.js"))
            .context("initializing Peregrust canvas and events")?;
        js.execute_script("peregrust:control", include_str!("../js/control.js"))
            .context("initializing agent control")?;
        js.execute_script("peregrust:storage", include_str!("../js/storage.js"))
            .context("initializing persistent game storage")?;
        js.execute_script("peregrust:web-assets", include_str!("../js/web_assets.js"))
            .context("initializing asset loading and image textures")?;
        js.execute_script("peregrust:gpu-uploads", include_str!("../js/gpu_upload.js"))
            .context("initializing batched GPU uploads")?;
        js.execute_script("peregrust:gpu-mapping", include_str!("../js/gpu_mapping.js"))
            .context("initializing creation-mapped GPU buffers")?;
        let dispatch_event_fn = Self::lookup_function(&mut js, "__peregrustDispatchEvent")?;
        let dispatch_frame_fn = Self::lookup_function(&mut js, "__peregrustDispatchFrame")?;
        Ok(Self {
            control_server,
            watchdog,
            loader,
            status: RuntimeStatus::Created,
            module_evaluation: None,
            dispatch_event_fn,
            dispatch_frame_fn,
            frame_future: None,
            pending_resize: None,
            js,
        })
    }

    pub fn is_ready(&self) -> bool {
        self.status == RuntimeStatus::Ready
    }
    /// Safe to move to another thread; only permits termination of this V8 isolate.
    pub fn isolate_handle(&mut self) -> v8::IsolateHandle {
        self.js.v8_isolate().thread_safe_handle()
    }

    /// Starts an ES module graph. File reads and TS compilation happen during
    /// this call; top-level await and asynchronous ops progress in `tick`.
    pub fn start_entry(&mut self, path: impl AsRef<Path>) -> Result<()> {
        if self.status != RuntimeStatus::Created {
            bail!("the application entry point has already been started");
        }
        let specifier = self
            .loader
            .entry_specifier(path)
            .map_err(|error| anyhow!(error.to_string()))?;
        self.start_module(specifier)
    }

    fn start_module(&mut self, specifier: ModuleSpecifier) -> Result<()> {
        // This loader uses only synchronous local reads. Deno's async API is
        // driven here to finish graph instantiation, then evaluation is polled
        // independently by the winit loop so top-level await never blocks it.
        let module_id =
            deno_core::futures::executor::block_on(self.js.load_main_es_module(&specifier))
                .with_context(|| format!("loading entry module {specifier}"))?;
        self.module_evaluation = Some(Box::pin(self.js.mod_evaluate(module_id)));
        self.status = RuntimeStatus::Evaluating;
        Ok(())
    }

    /// Runs one V8 event-loop turn. Pass a waker backed by the winit
    /// `EventLoopProxy` so completed GPU and timer operations wake the window.
    pub fn tick(&mut self, waker: &Waker) -> Result<RuntimePoll> {
        self.check_timeout()?;
        let mut cx = Context::from_waker(waker);
        let event_loop_result = self
            .js
            .poll_event_loop(&mut cx, PollEventLoopOptions::default());
        self.check_timeout()?;
        match event_loop_result {
            Poll::Ready(result) => {
                result.context("running JavaScript event loop")?;
            }
            Poll::Pending => {}
        }

        if let Some(evaluation) = self.module_evaluation.as_mut()
            && let Poll::Ready(result) = evaluation.as_mut().poll(&mut cx)
        {
            self.module_evaluation = None;
            result.context("evaluating application module")?;
            self.status = RuntimeStatus::Ready;
        }

        let mut frame_finished = false;
        let mut frame_completed = false;
        let mut frame_presented = false;
        if let Some(frame_future) = self.frame_future.as_mut()
            && let Poll::Ready(result) = frame_future.as_mut().poll(&mut cx)
        {
            self.frame_future = None;
            frame_finished = true;
            match result {
                Ok(_) => {
                    frame_completed = true;
                    frame_presented =
                        crate::gpu::present(&mut self.js).context("presenting WebGPU frame")?;
                }
                Err(error) if error.to_string().contains("Invalid Surface Status") => {
                    crate::gpu::recover_surface(&mut self.js)
                        .context("recovering WebGPU surface")?;
                    let host = self.js.op_state().borrow().borrow::<SharedHost>().clone();
                    host.request_redraw();
                }
                Err(error) => return Err(error).context("application frame callback failed"),
            }
        }

        if self.frame_future.is_none()
            && let Some((width, height)) = self.pending_resize.take()
        {
            crate::gpu::resize_surface(&mut self.js, width, height)
                .context("resizing WebGPU surface after frame")?;
        }

        Ok(RuntimePoll {
            frame_finished,
            frame_completed,
            frame_presented,
        })
    }

    /// Dispatches a host event to the canvas/window shims as a V8 value.
    pub fn dispatch_event(&mut self, event: &Value) -> Result<()> {
        self.check_timeout()?;
        let arg = {
            deno_core::scope!(scope, self.js);
            let local = deno_core::serde_v8::to_v8(scope, event)
                .context("converting native event to JavaScript")?;
            v8::Global::new(scope, local)
        };
        let mut result = Box::pin(self.js.call_with_args(&self.dispatch_event_fn, &[arg]));
        let mut cx = Context::from_waker(Waker::noop());
        match result.as_mut().poll(&mut cx) {
            Poll::Ready(Ok(_)) => Ok(()),
            Poll::Ready(Err(error)) => Err(error).context("dispatching native event"),
            Poll::Pending => bail!("native event listener returned an asynchronous Promise"),
        }
    }

    /// Begins a frame. Returns `false` while a prior async frame is still
    /// awaiting completion or while the app module is still initializing.
    pub fn dispatch_frame(&mut self, timestamp_ms: f64) -> Result<bool> {
        self.check_timeout()?;
        if !self.is_ready() || self.frame_future.is_some() {
            return Ok(false);
        }
        let size = self
            .js
            .op_state()
            .borrow()
            .borrow::<SharedHost>()
            .physical_size();
        if size.width == 0 || size.height == 0 {
            return Ok(false);
        }
        if !timestamp_ms.is_finite() {
            bail!("frame timestamp must be finite");
        }
        let arg = {
            deno_core::scope!(scope, self.js);
            let number = v8::Number::new(scope, timestamp_ms);
            let value: v8::Local<v8::Value> = number.into();
            v8::Global::new(scope, value)
        };
        self.frame_future = Some(Box::pin(
            self.js.call_with_args(&self.dispatch_frame_fn, &[arg]),
        ));
        Ok(true)
    }

    pub fn resize_surface(&mut self, width: u32, height: u32) -> Result<()> {
        self.check_timeout()?;
        if self.frame_future.is_some() {
            let host = self.js.op_state().borrow().borrow::<SharedHost>().clone();
            host.set_physical_size(winit::dpi::PhysicalSize::new(width, height));
            self.pending_resize = Some((width, height));
            return Ok(());
        }
        crate::gpu::resize_surface(&mut self.js, width, height).context("resizing WebGPU surface")
    }

    pub fn shutdown(self) {
        drop(self);
    }

    fn check_timeout(&self) -> Result<()> {
        if self
            .watchdog
            .as_ref()
            .is_some_and(ExecutionWatchdog::expired)
        {
            bail!("JavaScript execution exceeded its time limit");
        }
        Ok(())
    }

    fn lookup_function(js: &mut JsRuntime, name: &'static str) -> Result<v8::Global<v8::Function>> {
        let global = js.execute_script("peregrust:lookup", format!("globalThis.{name}"))?;
        deno_core::scope!(scope, js);
        let local = v8::Local::new(scope, global);
        let function = v8::Local::<v8::Function>::try_from(local)
            .map_err(|_| anyhow!("bootstrap did not install {name} as a function"))?;
        Ok(v8::Global::new(scope, function))
    }
}
