//! Opt-in local control. The socket thread never touches V8: requests are
//! consumed by the JavaScript frame hooks and replies cross a bounded channel.
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use base64::Engine;
use clap::Parser;
use deno_core::{OpState, op2};
use deno_error::JsErrorBox;
use image::ImageEncoder;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::host::SharedHost;

const MAX_REQUEST: u64 = 1024 * 1024;
const MAX_RESPONSE: u64 = 32 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    version: u32,
    session_id: String,
    address: SocketAddr,
    token: String,
    pid: u32,
}

struct Request {
    value: Value,
    deadline: Instant,
    reply: SyncSender<Value>,
}

pub struct ControlState {
    requests: Receiver<Request>,
    active: Option<Request>,
    enabled: bool,
}

impl ControlState {
    fn poll(&mut self) -> Option<Value> {
        if self.active.is_some() {
            return None;
        }
        while let Ok(request) = self.requests.try_recv() {
            if Instant::now() < request.deadline {
                let value = request.value.clone();
                self.active = Some(request);
                return Some(value);
            }
        }
        None
    }

    fn complete(&mut self, value: Value) {
        if let Some(request) = self.active.take() {
            let _ = request.reply.try_send(value);
        }
    }
}

pub struct ControlServer {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    path: PathBuf,
    session: Session,
}

impl ControlServer {
    pub fn start(path: Option<&Path>, host: SharedHost) -> Result<(Option<Self>, ControlState)> {
        Self::start_with_wake(path, Arc::new(move || host.request_redraw()))
    }

    fn start_with_wake(
        path: Option<&Path>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(Option<Self>, ControlState)> {
        let (send, requests) = mpsc::sync_channel::<Request>(32);
        let state = ControlState {
            requests,
            active: None,
            enabled: path.is_some(),
        };
        let Some(path) = path else {
            return Ok((None, state));
        };
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let mut random = [0u8; 32];
        getrandom::fill(&mut random)
            .map_err(|error| anyhow::anyhow!("session entropy: {error}"))?;
        let token: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let session = Session {
            version: 1,
            session_id: token[..16].to_owned(),
            address: listener.local_addr()?,
            token,
            pid: std::process::id(),
        };
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path).with_context(|| {
            format!(
                "creating control session {} (must not already exist)",
                path.display()
            )
        })?;
        let stop = Arc::new(AtomicBool::new(false));
        let mut server = Self {
            stop: stop.clone(),
            thread: None,
            path: path.to_owned(),
            session,
        };
        let contents = serde_json::to_vec_pretty(&server.session)?;
        let written = file.write_all(&contents).and_then(|()| file.sync_all());
        drop(file);
        if let Err(error) = written {
            let _ = std::fs::remove_file(path);
            return Err(error.into());
        }
        let token = server.session.token.clone();
        let session_id = server.session.session_id.clone();
        server.thread = Some(
            std::thread::Builder::new()
                .name("peregrust-control".into())
                .spawn(move || {
                    for connection in listener.incoming() {
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                        let Ok(mut stream) = connection else {
                            break;
                        };
                        let result = serve(&mut stream, &token, &send, &stop, &*wake);
                        let mut response = result.unwrap_or_else(|error| {
                            failure("CONTROL_ERROR", &format!("{error:#}"))
                        });
                        response["sessionId"] = json!(session_id);
                        if let Ok(bytes) = serde_json::to_vec(&response) {
                            let _ = stream.write_all(&bytes);
                            let _ = stream.write_all(b"\n");
                        }
                    }
                })?,
        );
        Ok((Some(server), state))
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Wake blocking accept without a polling thread.
        let _ = TcpStream::connect_timeout(&self.session.address, Duration::from_millis(200));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let owned = std::fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Session>(&bytes).ok())
            .is_some_and(|session| session.token == self.session.token);
        if owned {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn read_message(stream: &TcpStream, max_bytes: u64) -> Result<Value> {
    let mut line = String::new();
    BufReader::new(stream)
        .take(max_bytes + 1)
        .read_line(&mut line)?;
    if line.len() as u64 > max_bytes || !line.ends_with('\n') {
        bail!("message exceeds limit or is missing its newline");
    }
    Ok(serde_json::from_str(&line)?)
}

fn serve(
    stream: &mut TcpStream,
    token: &str,
    requests: &SyncSender<Request>,
    stop: &AtomicBool,
    wake: &dyn Fn(),
) -> Result<Value> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut value = read_message(stream, MAX_REQUEST)?;
    if value.get("token").and_then(Value::as_str) != Some(token) {
        return Ok(failure("UNAUTHORIZED", "invalid session token"));
    }
    value
        .as_object_mut()
        .context("request must be an object")?
        .remove("token");
    let timeout = value
        .get("timeoutMs")
        .and_then(Value::as_u64)
        .unwrap_or(10_000);
    if !(1..=60_000).contains(&timeout) {
        bail!("timeoutMs must be between 1 and 60000");
    }
    let deadline = Instant::now() + Duration::from_millis(timeout);
    let (reply, receive) = mpsc::sync_channel(1);
    requests
        .try_send(Request {
            value,
            deadline,
            reply,
        })
        .context("control queue unavailable")?;
    wake();
    loop {
        if stop.load(Ordering::Acquire) {
            return Ok(failure("STOPPED", "runtime stopped"));
        }
        if Instant::now() >= deadline {
            return Ok(failure(
                "TIMEOUT",
                "request expired; an action already started may have taken effect",
            ));
        }
        match receive.recv_timeout(Duration::from_millis(25)) {
            Ok(value) => return Ok(value),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Ok(failure("STOPPED", "runtime stopped"));
            }
        }
    }
}

fn failure(code: &str, message: &str) -> Value {
    json!({"ok": false, "error": {"code": code, "message": message}})
}

#[op2]
#[serde]
fn op_peregrust_control_poll(state: &mut OpState) -> Option<Value> {
    state.borrow_mut::<ControlState>().poll()
}

#[op2]
fn op_peregrust_control_reply(state: &mut OpState, #[serde] value: serde_json::Value) {
    state.borrow_mut::<ControlState>().complete(value);
}

#[op2(fast)]
fn op_peregrust_control_enabled(state: &OpState) -> bool {
    state.borrow::<ControlState>().enabled
}

#[op2(fast)]
fn op_peregrust_control_cancelled(state: &OpState) -> bool {
    state
        .borrow::<ControlState>()
        .active
        .as_ref()
        .is_some_and(|request| Instant::now() >= request.deadline)
}

#[op2]
#[string]
fn op_peregrust_control_png(
    width: u32,
    height: u32,
    #[buffer] pixels: &[u8],
) -> Result<String, JsErrorBox> {
    encode_png(width, height, pixels)
}

fn encode_png(width: u32, height: u32, pixels: &[u8]) -> Result<String, JsErrorBox> {
    if width == 0
        || height == 0
        || width > 2048
        || height > 2048
        || pixels.len() != width as usize * height as usize * 4
    {
        return Err(JsErrorBox::range_error(
            "capture requires RGBA8 pixels and dimensions between 1 and 2048",
        ));
    }
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(pixels, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|error| JsErrorBox::generic(error.to_string()))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

deno_core::extension!(
    peregrust_control,
    ops = [
        op_peregrust_control_poll,
        op_peregrust_control_reply,
        op_peregrust_control_enabled,
        op_peregrust_control_cancelled,
        op_peregrust_control_png,
    ]
);

#[derive(Parser)]
#[command(
    name = "peregrust ctl",
    about = "Control an existing game; stdout is JSON"
)]
struct ControlCli {
    /// Session file created by peregrust game.ts --control SESSION.
    #[arg(long)]
    session: PathBuf,
    /// Operation name. Start with control.describe.
    method: String,
    /// JSON object of operation parameters.
    #[arg(long, default_value = "{}", conflicts_with = "params_file")]
    params: String,
    /// Read parameters from a JSON file (avoids shell quoting).
    #[arg(long)]
    params_file: Option<PathBuf>,
    /// Maximum request duration in milliseconds.
    #[arg(long, default_value_t = 10000, value_parser = clap::value_parser!(u64).range(1..=60000))]
    timeout_ms: u64,
    /// Save result.capture as PNG and replace its base64 data with the file path.
    #[arg(long)]
    output: Option<PathBuf>,
}

fn call(cli: &ControlCli) -> Result<Value> {
    let session: Session = serde_json::from_slice(&std::fs::read(&cli.session)?)?;
    if session.version != 1 || !session.address.ip().is_loopback() {
        bail!("unsupported or nonlocal session");
    }
    let params: Value = serde_json::from_str(&match &cli.params_file {
        Some(path) => std::fs::read_to_string(path)?,
        None => cli.params.clone(),
    })?;
    if !params.is_object() {
        bail!("params must be a JSON object");
    }
    let request = json!({"token": session.token, "method": cli.method, "params": params, "timeoutMs": cli.timeout_ms});
    let bytes = serde_json::to_vec(&request)?;
    if bytes.len() as u64 >= MAX_REQUEST {
        bail!("request exceeds 1 MiB");
    }
    let mut stream = TcpStream::connect_timeout(&session.address, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_millis(cli.timeout_ms + 5000)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(&bytes)?;
    stream.write_all(b"\n")?;
    let mut response = read_message(&stream, MAX_RESPONSE)?;
    if response.get("sessionId").and_then(Value::as_str) != Some(&session.session_id) {
        bail!("response session mismatch");
    }
    if response["ok"] == true
        && let Some(path) = &cli.output
    {
        let capture = response
            .get_mut("result")
            .and_then(|value| value.get_mut("capture"))
            .context("operation did not return a capture")?;
        let data = capture["data"]
            .as_str()
            .context("capture data is missing")?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
        std::fs::write(path, bytes)?;
        capture
            .as_object_mut()
            .context("invalid capture")?
            .remove("data");
        capture["path"] = json!(std::fs::canonicalize(path)?.to_string_lossy());
    }
    Ok(response)
}

pub fn run_cli() -> i32 {
    let cli = match ControlCli::try_parse_from(
        std::iter::once(std::ffi::OsString::from("peregrust ctl"))
            .chain(std::env::args_os().skip(2)),
    ) {
        Ok(cli) => cli,
        Err(error) if error.use_stderr() => {
            println!("{}", failure("INVALID_ARGUMENT", &error.to_string()));
            return 2;
        }
        Err(error) => {
            let _ = error.print();
            return 0;
        }
    };
    let response =
        call(&cli).unwrap_or_else(|error| failure("CONTROL_ERROR", &format!("{error:#}")));
    let code = if response["ok"] == true { 0 } else { 1 };
    println!("{response}");
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_is_real_rgba_and_rejects_invalid_buffers() {
        let encoded = encode_png(1, 1, &[255, 0, 0, 255]).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        assert_eq!(
            image::load_from_memory(&bytes)
                .unwrap()
                .into_rgba8()
                .as_raw(),
            &[255, 0, 0, 255]
        );
        assert!(encode_png(1, 2, &[0; 4]).is_err());
    }

    #[test]
    fn expired_requests_never_reach_javascript() {
        let (send, receive) = mpsc::sync_channel(1);
        let (reply, _) = mpsc::sync_channel(1);
        send.send(Request {
            value: json!({"method":"scene.update"}),
            deadline: Instant::now() - Duration::from_secs(1),
            reply,
        })
        .unwrap();
        let mut state = ControlState {
            requests: receive,
            active: None,
            enabled: true,
        };
        assert!(state.poll().is_none());
    }

    #[test]
    fn session_refuses_overwrite_authenticates_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let (server, _state) =
            ControlServer::start_with_wake(Some(&path), Arc::new(|| {})).unwrap();
        assert!(ControlServer::start_with_wake(Some(&path), Arc::new(|| {})).is_err());
        let address = server.as_ref().unwrap().session.address;
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        stream.write_all(b"{\"token\":\"wrong\"}\n").unwrap();
        assert_eq!(
            read_message(&stream, MAX_RESPONSE).unwrap()["error"]["code"],
            "UNAUTHORIZED"
        );
        drop(server);
        assert!(!path.exists());
    }
}
