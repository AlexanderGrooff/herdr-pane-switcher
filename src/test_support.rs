use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub const PLUGIN_TIMEOUT: Duration = Duration::from_secs(15);

pub fn generate_panes(size: usize) -> Vec<Value> {
    const STATUSES: [&str; 4] = ["blocked", "done", "idle", "working"];
    (0..size)
        .map(|i| {
            json!({
                "pane_id": format!("pane-{i:04}"),
                "agent_status": STATUSES[i % STATUSES.len()],
                "title": format!("Pane {i}"),
            })
        })
        .collect()
}

#[derive(Default)]
struct ServerState {
    panes: Vec<Value>,
    current_pane_id: Option<String>,
    focus_calls: Vec<String>,
}

pub struct MockHerdrServer {
    socket_path: PathBuf,
    state: Arc<Mutex<ServerState>>,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

#[allow(dead_code)]
impl MockHerdrServer {
    pub fn new(
        socket_path: PathBuf,
        panes: Vec<Value>,
        current_pane_id: Option<String>,
    ) -> Result<Self> {
        let listener = UnixListener::bind(&socket_path)
            .with_context(|| format!("failed to bind mock socket {}", socket_path.display()))?;
        listener.set_nonblocking(true)?;
        let state = Arc::new(Mutex::new(ServerState {
            panes,
            current_pane_id,
            focus_calls: Vec::new(),
        }));
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_state = Arc::clone(&state);
        let thread_shutdown = Arc::clone(&shutdown);
        let thread = thread::spawn(move || {
            while !thread_shutdown.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => handle_connection(stream, &thread_state),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            socket_path,
            state,
            shutdown,
            thread: Some(thread),
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn focus_calls(&self) -> Vec<String> {
        self.state.lock().unwrap().focus_calls.clone()
    }

    pub fn clear_focus_calls(&self) {
        self.state.lock().unwrap().focus_calls.clear();
    }

    pub fn set_panes(&self, panes: Vec<Value>) {
        self.state.lock().unwrap().panes = panes;
    }
}

impl Drop for MockHerdrServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let _ = UnixStream::connect(&self.socket_path);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.socket_path);
    }
}

fn handle_connection(stream: UnixStream, state: &Arc<Mutex<ServerState>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }

    let request: Value = match serde_json::from_str(&line) {
        Ok(request) => request,
        Err(error) => json!({"error": error.to_string()}),
    };
    let response = dispatch(&request, state);
    let mut stream = stream;
    let _ = writeln!(stream, "{}", response);
}

fn dispatch(request: &Value, state: &Arc<Mutex<ServerState>>) -> Value {
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
    let mut state = state.lock().unwrap();
    match method {
        "pane.list" => json!({"result": {"panes": state.panes}}),
        "pane.focus" => {
            if let Some(pane_id) = params.get("pane_id").and_then(Value::as_str) {
                state.current_pane_id = Some(pane_id.to_string());
                state.focus_calls.push(pane_id.to_string());
            }
            json!({"result": "ok"})
        }
        "pane.current" => json!({"result": {"pane_id": state.current_pane_id}}),
        _ => json!({"error": format!("unknown method: {method}")}),
    }
}

pub fn base_env(socket_path: &Path, state_dir: &Path) -> HashMap<String, String> {
    let mut env: HashMap<_, _> = std::env::vars().collect();
    env.insert(
        "HERDR_SOCKET_PATH".into(),
        socket_path.display().to_string(),
    );
    env.insert(
        "HERDR_PLUGIN_STATE_DIR".into(),
        state_dir.display().to_string(),
    );
    env.remove("HERDR_PLUGIN_ACTION_ID");
    env.remove("HERDR_PANE_ID");
    env.remove("HERDR_PLUGIN_EVENT");
    env.remove("HERDR_PLUGIN_EVENT_JSON");
    env
}

pub fn action_env(
    mut env: HashMap<String, String>,
    action: &str,
    pane_id: Option<&str>,
) -> HashMap<String, String> {
    env.insert("HERDR_PLUGIN_ACTION_ID".into(), action.into());
    if let Some(pane_id) = pane_id {
        env.insert("HERDR_PANE_ID".into(), pane_id.into());
    }
    env
}

pub fn event_env(
    mut env: HashMap<String, String>,
    event: &str,
    event_json: &Value,
) -> HashMap<String, String> {
    env.insert("HERDR_PLUGIN_EVENT".into(), event.into());
    env.insert("HERDR_PLUGIN_EVENT_JSON".into(), event_json.to_string());
    env
}

pub fn run_plugin(binary: &Path, env: &HashMap<String, String>) -> Result<Output> {
    let mut child = Command::new(binary)
        .env_clear()
        .envs(env)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start plugin {}", binary.display()))?;
    wait_for_plugin(&mut child)?;
    child
        .wait_with_output()
        .context("failed to collect plugin output")
}

fn wait_for_plugin(child: &mut Child) -> Result<()> {
    let deadline = Instant::now() + PLUGIN_TIMEOUT;
    loop {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("plugin timed out after {}s", PLUGIN_TIMEOUT.as_secs());
        }
        thread::sleep(Duration::from_millis(10));
    }
}
