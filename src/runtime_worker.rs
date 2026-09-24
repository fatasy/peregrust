//! Main-thread facade for the single-threaded V8 runtime.
//!
//! Deno's local async task spawning requires a Tokio current-thread runtime.
//! The worker owns V8, its module graph, and that executor for their entire
//! lifetime. The winit thread only queues commands and drains completion
//! messages, so neither JavaScript nor asset I/O blocks window handling.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};
use std::thread::JoinHandle;

use anyhow::{Context as _, Result, anyhow, bail};
use deno_core::v8;
use serde_json::Value;
use tokio::sync::mpsc::{
    self as tokio_mpsc, Receiver as CommandReceiver, Sender as CommandSender, error::TrySendError,
};

use crate::runtime::Runtime as CoreRuntime;
pub use crate::runtime::{RuntimeConfig, RuntimePoll};

const COMMAND_CAPACITY: usize = 4096;

enum Command {
    Start(PathBuf),
    Event(Value),
    Frame(f64),
    Resize(u32, u32),
    Shutdown,
}

enum Response {
    Ready,
    FrameSkipped,
    Poll(RuntimePoll),
    Failed(anyhow::Error),
}

pub struct Runtime {
    commands: Option<CommandSender<Command>>,
    responses: Receiver<Response>,
    worker: Option<JoinHandle<()>>,
    isolate: Arc<Mutex<Option<v8::IsolateHandle>>>,
    stopping: Arc<AtomicBool>,
    started: bool,
    ready: bool,
    frame_pending: bool,
}

impl Runtime {
    pub fn new(config: RuntimeConfig, waker: Waker) -> Result<Self> {
        let (commands, command_rx) = tokio_mpsc::channel(COMMAND_CAPACITY);
        let (response_tx, responses) = mpsc::channel();
        let isolate = Arc::new(Mutex::new(None));
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_isolate = Arc::clone(&isolate);
        let worker_stopping = Arc::clone(&stopping);
        let worker = std::thread::Builder::new()
            .name("peregrust-js".into())
            .spawn(move || {
                if let Err(error) = worker_main(
                    config,
                    command_rx,
                    &response_tx,
                    &waker,
                    &worker_isolate,
                    &worker_stopping,
                ) {
                    let _ = response_tx.send(Response::Failed(error));
                    waker.wake_by_ref();
                }
            })
            .context("starting JavaScript runtime thread")?;
        Ok(Self {
            commands: Some(commands),
            responses,
            worker: Some(worker),
            isolate,
            stopping,
            started: false,
            ready: false,
            frame_pending: false,
        })
    }

    pub fn start_entry(&mut self, path: impl AsRef<Path>) -> Result<()> {
        if self.started {
            bail!("the application entry point has already been started");
        }
        self.send(Command::Start(path.as_ref().to_owned()))?;
        self.started = true;
        Ok(())
    }

    pub fn dispatch_event(&mut self, event: &Value) -> Result<()> {
        let motion = matches!(
            event.get("type").and_then(Value::as_str),
            Some("pointermove" | "pointerrawupdate")
        );
        let sender = self
            .commands
            .as_ref()
            .ok_or_else(|| anyhow!("JavaScript runtime has shut down"))?;
        match sender.try_send(Command::Event(event.clone())) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) if motion => Ok(()),
            Err(error) => Err(anyhow!("JavaScript command queue is unavailable: {error}")),
        }
    }

    pub fn dispatch_frame(&mut self, timestamp_ms: f64) -> Result<bool> {
        if !timestamp_ms.is_finite() {
            bail!("frame timestamp must be finite");
        }
        if !self.ready || self.frame_pending {
            return Ok(false);
        }
        self.send(Command::Frame(timestamp_ms))?;
        self.frame_pending = true;
        Ok(true)
    }

    pub fn resize_surface(&mut self, width: u32, height: u32) -> Result<()> {
        self.send(Command::Resize(width, height))
    }

    pub fn tick(&mut self, _waker: &Waker) -> Result<RuntimePoll> {
        let mut combined = RuntimePoll {
            frame_finished: false,
            frame_completed: false,
            frame_presented: false,
        };
        loop {
            match self.responses.try_recv() {
                Ok(Response::Ready) => self.ready = true,
                Ok(Response::FrameSkipped) => self.frame_pending = false,
                Ok(Response::Poll(poll)) => {
                    combined.frame_finished |= poll.frame_finished;
                    combined.frame_completed |= poll.frame_completed;
                    combined.frame_presented |= poll.frame_presented;
                    if poll.frame_finished {
                        self.frame_pending = false;
                    }
                }
                Ok(Response::Failed(error)) => return Err(error),
                Err(TryRecvError::Empty) => return Ok(combined),
                Err(TryRecvError::Disconnected) => {
                    return Err(anyhow!("JavaScript runtime thread stopped unexpectedly"));
                }
            }
        }
    }

    pub fn is_ready(&self) -> bool {
        self.ready
    }

    pub fn frame_pending(&self) -> bool {
        self.frame_pending
    }

    pub fn shutdown(mut self) -> Result<()> {
        self.stop_join()
    }

    fn send(&self, command: Command) -> Result<()> {
        self.commands
            .as_ref()
            .ok_or_else(|| anyhow!("JavaScript runtime has shut down"))?
            .try_send(command)
            .map_err(|error| anyhow!("JavaScript command queue is unavailable: {error}"))
    }

    fn stop_join(&mut self) -> Result<()> {
        self.stopping.store(true, Ordering::SeqCst);
        if let Some(commands) = self.commands.take() {
            let _ = commands.try_send(Command::Shutdown);
            drop(commands);
        }
        if let Some(isolate) = self
            .isolate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
        {
            isolate.terminate_execution();
        }
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|panic| match panic.downcast::<String>() {
                    Ok(message) => anyhow!("JavaScript runtime thread panicked: {}", *message),
                    Err(panic) => match panic.downcast::<&'static str>() {
                        Ok(message) => anyhow!("JavaScript runtime thread panicked: {}", *message),
                        Err(_) => anyhow!("JavaScript runtime thread panicked"),
                    },
                })?;
        }
        Ok(())
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if let Err(error) = self.stop_join() {
            eprintln!("{error:#}");
        }
    }
}

fn worker_main(
    config: RuntimeConfig,
    mut commands: CommandReceiver<Command>,
    responses: &mpsc::Sender<Response>,
    wake_main: &Waker,
    isolate_slot: &Arc<Mutex<Option<v8::IsolateHandle>>>,
    stopping: &AtomicBool,
) -> Result<()> {
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(4)
        .build()
        .context("starting JavaScript async executor")?;
    executor.block_on(async {
        let mut runtime = CoreRuntime::new(config)?;
        *isolate_slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(runtime.isolate_handle());
        let mut reported_ready = false;
        loop {
            if stopping.load(Ordering::SeqCst) {
                break;
            }
            enum Next {
                Command(Option<Command>),
                Poll(RuntimePoll, bool),
            }
            let next = tokio::select! {
                command = commands.recv() => Next::Command(command),
                result = std::future::poll_fn(|cx| {
                    match runtime.tick(cx.waker()) {
                        Err(error) => Poll::Ready(Err(error)),
                        Ok(poll) => {
                            let ready = runtime.is_ready();
                            if poll.frame_finished || ready != reported_ready {
                                Poll::Ready(Ok((poll, ready)))
                            } else {
                                Poll::Pending
                            }
                        }
                    }
                }) => {
                    let (poll, ready) = result?;
                    Next::Poll(poll, ready)
                },
            };
            match next {
                Next::Command(Some(Command::Start(path))) => runtime.start_entry(path)?,
                Next::Command(Some(Command::Event(event))) => runtime.dispatch_event(&event)?,
                Next::Command(Some(Command::Frame(timestamp))) => {
                    if !runtime.dispatch_frame(timestamp)? {
                        report(responses, wake_main, Response::FrameSkipped);
                    }
                }
                Next::Command(Some(Command::Resize(width, height))) => {
                    runtime.resize_surface(width, height)?;
                }
                Next::Command(Some(Command::Shutdown) | None) => break,
                Next::Poll(poll, ready) => {
                    if ready != reported_ready {
                        reported_ready = ready;
                        if ready {
                            report(responses, wake_main, Response::Ready);
                        }
                    }
                    if poll.frame_finished {
                        report(responses, wake_main, Response::Poll(poll));
                    }
                }
            }
        }
        runtime.shutdown();
        Ok(())
    })
}

fn report(responses: &mpsc::Sender<Response>, wake_main: &Waker, message: Response) {
    let _ = responses.send(message);
    wake_main.wake_by_ref();
}
