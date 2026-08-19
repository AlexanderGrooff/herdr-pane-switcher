use anyhow::Result;

mod herdr {
    use anyhow::{bail, Context, Result};
    use serde::{Deserialize, Serialize};
    use serde_json::Value;
    use std::io::{BufRead, Write};
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Pane {
        pub pane_id: String,
        pub agent_status: Option<String>,
        pub title: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    struct PaneListEnvelope {
        #[serde(default)]
        result: Option<PaneListResult>,
        #[serde(default)]
        panes: Option<Vec<Pane>>,
        #[serde(default)]
        error: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    struct PaneListResult {
        #[serde(default)]
        panes: Option<Vec<Pane>>,
    }

    #[derive(Debug, Deserialize, Default)]
    struct ErrorOnly {
        #[serde(default)]
        error: Option<String>,
    }

    fn request_id(method: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("plugin:pane-switcher:{}:{}", method, nanos)
    }

    /// Recursively search a JSON value for the first occurrence of `key`.
    /// Mirrors Python's nested_value helper so the Rust client is tolerant
    /// of both the mock server's `{"result": {"panes": ...}}` shape and
    /// Herdr's internally-tagged response shapes.
    fn nested_value<'v>(value: &'v Value, key: &str) -> Option<&'v Value> {
        match value {
            Value::Object(map) => {
                if let Some(v) = map.get(key) {
                    return Some(v);
                }
                for child in map.values() {
                    if let Some(v) = nested_value(child, key) {
                        return Some(v);
                    }
                }
                None
            }
            Value::Array(arr) => {
                for child in arr {
                    if let Some(v) = nested_value(child, key) {
                        return Some(v);
                    }
                }
                None
            }
            _ => None,
        }
    }

    fn truthy_string(value: &Value) -> Option<String> {
        match value {
            Value::String(s) if !s.is_empty() => Some(s.clone()),
            _ => None,
        }
    }

    fn as_string(value: Option<&Value>) -> Option<String> {
        value.and_then(truthy_string)
    }

    fn herdr_request_raw(method: &str, params: Value) -> Result<String> {
        let socket_path = crate::env::herdr_socket_path()?;
        let payload = serde_json::json!({
            "id": request_id(method),
            "method": method,
            "params": params,
        });

        let mut stream = UnixStream::connect(&socket_path).with_context(|| {
            format!(
                "failed to connect to herdr socket at {}",
                socket_path.display()
            )
        })?;
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .context("failed to set socket read timeout")?;
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .context("failed to set socket write timeout")?;

        let line = serde_json::to_string(&payload)? + "\n";
        stream
            .write_all(line.as_bytes())
            .context("failed to write request")?;
        stream.flush().context("failed to flush request")?;

        let mut reader =
            std::io::BufReader::new(stream.try_clone().context("failed to clone socket")?);
        let mut response = String::new();
        reader
            .read_line(&mut response)
            .context("failed to read response")?;

        if response.is_empty() {
            bail!("no response from herdr for {}", method);
        }

        Ok(response)
    }

    fn check_error(response: &str) -> Result<()> {
        let err: ErrorOnly = serde_json::from_str(response)
            .with_context(|| format!("invalid response from herdr: {}", response.trim()))?;
        if let Some(e) = err.error {
            bail!("herdr failed: {}", e);
        }
        Ok(())
    }

    pub fn all_panes() -> Result<Vec<Pane>> {
        let response = herdr_request_raw("pane.list", serde_json::json!({}))?;
        let parsed: PaneListEnvelope = serde_json::from_str(&response)
            .with_context(|| format!("invalid pane.list response: {}", response.trim()))?;
        if let Some(err) = parsed.error {
            bail!("herdr pane.list failed: {}", err);
        }
        Ok(parsed
            .panes
            .or(parsed.result.and_then(|r| r.panes))
            .unwrap_or_default())
    }

    pub fn all_pane_ids() -> Result<Vec<String>> {
        Ok(all_panes()?.into_iter().map(|p| p.pane_id).collect())
    }

    pub fn focus_pane(pane_id: &str) -> Result<()> {
        let response = herdr_request_raw("pane.focus", serde_json::json!({"pane_id": pane_id}))?;
        check_error(&response)
    }

    pub fn current_pane() -> Result<Option<String>> {
        let response = herdr_request_raw("pane.current", serde_json::json!({}))?;
        let data: Value = serde_json::from_str(&response)
            .with_context(|| format!("invalid pane.current response: {}", response.trim()))?;
        if let Some(err) = data.get("error").and_then(Value::as_str) {
            bail!("herdr pane.current failed: {}", err);
        }
        Ok(as_string(nested_value(&data, "pane_id")))
    }
}

mod state {
    use anyhow::{bail, Context, Result};
    use std::fs::{create_dir_all, rename, File, OpenOptions};
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::raw::c_int;
    use std::path::Path;

    /// Compact binary state file. The `postcard` crate is unavailable in the
    /// current registry mirror, so we use a tiny tagged little-endian format
    /// that is still versioned and stable.
    const MAGIC: &[u8] = b"HMSC";
    const CURRENT_VERSION: u32 = 2;
    const STATE_FILE: &str = "state.bin";
    const STATE_TMP_FILE: &str = "state.bin.tmp";

    const LOCK_EX: c_int = 0x02;
    const LOCK_UN: c_int = 0x08;

    // SAFETY: flock(2) is declared directly because the rustix crate is not
    // available in the environment's registry mirror. It is a libc function
    // on both macOS and Linux, linked through the standard library.
    extern "C" {
        fn flock(fd: c_int, operation: c_int) -> c_int;
    }

    struct LockGuard {
        file: File,
    }

    impl LockGuard {
        fn new(lock_path: &Path) -> Result<Self> {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(lock_path)
                .context("failed to open state lock")?;
            let fd = file.as_raw_fd();
            // SAFETY: flock is the libc advisory locking function; fd is valid
            // and the lock is released when the file is closed/dropped.
            let rc = unsafe { flock(fd, LOCK_EX) };
            if rc != 0 {
                bail!("failed to acquire state lock");
            }
            Ok(Self { file })
        }
    }

    impl Drop for LockGuard {
        fn drop(&mut self) {
            // SAFETY: same fd used for LOCK_EX; errors on unlock are ignored
            // because the lock is also released when the fd is closed.
            let _ = unsafe { flock(self.file.as_raw_fd(), LOCK_UN) };
        }
    }

    #[derive(Debug, Clone, PartialEq)]
    pub struct State {
        pub version: u32,
        pub history: Vec<String>,
        pub cycle: Option<Cycle>,
    }

    impl Default for State {
        fn default() -> Self {
            Self {
                version: CURRENT_VERSION,
                history: Vec::new(),
                cycle: None,
            }
        }
    }

    #[derive(Debug, Clone, PartialEq)]
    pub struct Cycle {
        pub index: usize,
        pub target: String,
        pub last_at: f64,
    }

    fn write_u32(buf: &mut Vec<u8>, value: u32) {
        buf.extend_from_slice(&value.to_le_bytes());
    }

    fn write_u8(buf: &mut Vec<u8>, value: u8) {
        buf.push(value);
    }

    fn write_string(buf: &mut Vec<u8>, value: &str) {
        write_u32(buf, value.len() as u32);
        buf.extend_from_slice(value.as_bytes());
    }

    pub fn to_bytes(state: &State) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(MAGIC);
        write_u32(&mut buf, state.version);
        write_u32(&mut buf, state.history.len() as u32);
        for pane in &state.history {
            write_string(&mut buf, pane);
        }
        if let Some(cycle) = &state.cycle {
            write_u8(&mut buf, 1);
            write_u32(&mut buf, cycle.index as u32);
            write_string(&mut buf, &cycle.target);
            buf.extend_from_slice(&cycle.last_at.to_le_bytes());
        } else {
            write_u8(&mut buf, 0);
        }
        buf
    }

    fn read_u32(buf: &[u8], pos: &mut usize) -> anyhow::Result<u32> {
        if *pos + 4 > buf.len() {
            bail!("truncated state file");
        }
        let value = u32::from_le_bytes(buf[*pos..*pos + 4].try_into().unwrap());
        *pos += 4;
        Ok(value)
    }

    fn read_u8(buf: &[u8], pos: &mut usize) -> anyhow::Result<u8> {
        if *pos >= buf.len() {
            bail!("truncated state file");
        }
        let value = buf[*pos];
        *pos += 1;
        Ok(value)
    }

    fn read_string(buf: &[u8], pos: &mut usize) -> anyhow::Result<String> {
        let len = read_u32(buf, pos)? as usize;
        if *pos + len > buf.len() {
            bail!("truncated state file");
        }
        let s =
            std::str::from_utf8(&buf[*pos..*pos + len]).context("invalid UTF-8 in state file")?;
        *pos += len;
        Ok(s.to_string())
    }

    fn read_f64(buf: &[u8], pos: &mut usize) -> anyhow::Result<f64> {
        if *pos + 8 > buf.len() {
            bail!("truncated state file");
        }
        let bytes: [u8; 8] = buf[*pos..*pos + 8].try_into().unwrap();
        *pos += 8;
        Ok(f64::from_le_bytes(bytes))
    }

    pub fn from_bytes(buf: &[u8]) -> Result<State> {
        if buf.len() < MAGIC.len() + 4 || &buf[..MAGIC.len()] != MAGIC {
            bail!("invalid state file magic");
        }
        let mut pos = MAGIC.len();
        let version = read_u32(buf, &mut pos)?;
        if version != CURRENT_VERSION {
            bail!("unsupported state file version: {}", version);
        }
        let history_len = read_u32(buf, &mut pos)? as usize;
        let mut history = Vec::with_capacity(history_len.min(1_000_000));
        for _ in 0..history_len {
            history.push(read_string(buf, &mut pos)?);
        }
        let has_cycle = read_u8(buf, &mut pos)?;
        let cycle = if has_cycle != 0 {
            let index = read_u32(buf, &mut pos)? as usize;
            let target = read_string(buf, &mut pos)?;
            let last_at = read_f64(buf, &mut pos)?;
            Some(Cycle {
                index,
                target,
                last_at,
            })
        } else {
            None
        };
        Ok(State {
            version,
            history,
            cycle,
        })
    }

    pub fn with_state<F, R>(state_dir: &Path, f: F) -> Result<R>
    where
        F: FnOnce(&mut State) -> Result<R>,
    {
        create_dir_all(state_dir).context("failed to create state directory")?;
        let lock_path = state_dir.join("state.lock");
        let state_path = state_dir.join(STATE_FILE);

        let _guard = LockGuard::new(&lock_path).context("failed to acquire state lock")?;

        let mut state = if state_path.exists() {
            let mut buf = Vec::new();
            let mut state_file = File::open(&state_path).context("failed to open state file")?;
            state_file
                .read_to_end(&mut buf)
                .context("failed to read state file")?;
            from_bytes(&buf).unwrap_or_default()
        } else {
            State::default()
        };

        let result = f(&mut state)?;

        let tmp_path = state_dir.join(STATE_TMP_FILE);
        {
            let mut tmp = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp_path)
                .context("failed to open temporary state file")?;
            let bytes = to_bytes(&state);
            tmp.write_all(&bytes)
                .context("failed to write temporary state file")?;
        }
        rename(&tmp_path, &state_path).context("failed to replace state file")?;

        Ok(result)
    }
}

mod env {
    use anyhow::{bail, Context, Result};
    use serde_json::Value;
    use std::env;
    use std::path::PathBuf;

    pub fn herdr_socket_path() -> Result<PathBuf> {
        if let Some(path) = env::var_os("HERDR_SOCKET_PATH") {
            if !path.is_empty() {
                return Ok(PathBuf::from(path));
            }
        }

        let config_dir = if let Some(xdg) = env::var_os("XDG_CONFIG_HOME") {
            PathBuf::from(xdg)
        } else if let Some(home) = env::var_os("HOME") {
            PathBuf::from(home).join(".config")
        } else {
            bail!("could not determine herdr socket path; set HERDR_SOCKET_PATH or XDG_CONFIG_HOME or HOME");
        };

        Ok(config_dir.join("herdr").join("herdr.sock"))
    }

    pub fn state_dir() -> Result<PathBuf> {
        match env::var("HERDR_PLUGIN_STATE_DIR") {
            Ok(dir) if !dir.is_empty() => Ok(PathBuf::from(dir)),
            _ => bail!("HERDR_PLUGIN_STATE_DIR is required"),
        }
    }

    pub fn current_pane_id() -> Result<Option<String>> {
        let event = env::var("HERDR_PLUGIN_EVENT")
            .ok()
            .filter(|s| !s.is_empty());
        let raw_json = env::var("HERDR_PLUGIN_EVENT_JSON").ok();

        let payload = match raw_json {
            Some(raw) => parse_event_json(&raw)?,
            None => Value::Object(serde_json::Map::new()),
        };

        if event.is_some() {
            if let Some(id) = nested_value(&payload, "pane_id") {
                return Ok(Some(id));
            }
        }

        if let Some(pane_id) = env::var("HERDR_PANE_ID").ok().filter(|s| !s.is_empty()) {
            return Ok(Some(pane_id));
        }

        if let Some(id) = nested_value(&payload, "focused_pane_id") {
            return Ok(Some(id));
        }
        if let Some(id) = nested_value(&payload, "pane_id") {
            return Ok(Some(id));
        }

        Ok(None)
    }

    pub fn active_pane_id() -> Result<Option<String>> {
        if let Some(pane_id) = current_pane_id()? {
            return Ok(Some(pane_id));
        }

        match crate::herdr::current_pane() {
            Ok(pane_id) => Ok(pane_id),
            Err(_) => Ok(None),
        }
    }

    fn parse_event_json(raw: &str) -> Result<Value> {
        serde_json::from_str(raw)
            .with_context(|| format!("malformed HERDR_PLUGIN_EVENT_JSON: {}", raw))
    }

    fn nested_value(value: &Value, key: &str) -> Option<String> {
        match value {
            Value::Object(map) => {
                if let Some(v) = map.get(key) {
                    if let Some(s) = as_truthy_string(v) {
                        return Some(s);
                    }
                }
                for v in map.values() {
                    if let Some(s) = nested_value(v, key) {
                        return Some(s);
                    }
                }
                None
            }
            Value::Array(arr) => {
                for v in arr {
                    if let Some(s) = nested_value(v, key) {
                        return Some(s);
                    }
                }
                None
            }
            _ => None,
        }
    }

    fn as_truthy_string(value: &Value) -> Option<String> {
        match value {
            Value::String(s) if !s.is_empty() => Some(s.clone()),
            _ => None,
        }
    }
}

mod actions {
    use crate::herdr::Pane;
    use crate::state::{self, Cycle, State};
    use crate::{env, herdr};
    use anyhow::{Context, Result};
    use std::collections::HashMap;
    use std::sync::OnceLock;
    use std::time::{SystemTime, UNIX_EPOCH};

    pub const CYCLE_TIMEOUT_SECONDS: f64 = 1.0;

    fn status_priority() -> &'static HashMap<String, usize> {
        static MAP: OnceLock<HashMap<String, usize>> = OnceLock::new();
        MAP.get_or_init(|| {
            HashMap::from([
                ("blocked".to_string(), 0),
                ("done".to_string(), 1),
                ("idle".to_string(), 2),
                ("working".to_string(), 3),
            ])
        })
    }

    fn now_epoch() -> Result<f64> {
        Ok(SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before unix epoch")?
            .as_secs_f64())
    }

    pub fn promote(history: &[String], pane: &str) -> Vec<String> {
        let mut promoted = Vec::with_capacity(history.len().saturating_add(1));
        promoted.push(pane.to_string());
        for existing in history {
            if existing != pane {
                promoted.push(existing.clone());
            }
        }
        promoted
    }

    pub fn record_event(pane_id: &str, event: &str) -> Result<()> {
        let state_dir = env::state_dir()?;
        state::with_state(&state_dir, |state| {
            record_event_logic(pane_id, event, state);
            Ok(())
        })
    }

    pub fn record_event_logic(pane_id: &str, event: &str, state: &mut State) {
        if event == "pane.closed" {
            state.history.retain(|p| p != pane_id);
            state.cycle = None;
        } else {
            state.history = promote(&state.history, pane_id);
            if let Some(ref cycle) = state.cycle {
                if cycle.target != pane_id {
                    state.cycle = None;
                }
            }
        }
    }

    pub fn cycle_panes(current_pane: &str) -> Result<()> {
        let panes = herdr::all_pane_ids()?;
        if panes.len() < 2 {
            return Ok(());
        }
        let state_dir = env::state_dir()?;
        let now = now_epoch()?;
        let target = state::with_state(&state_dir, |state| {
            let mut chosen: Option<String> = None;
            cycle_panes_logic(current_pane, &panes, state, now, |t| {
                chosen = Some(t.to_string());
                Ok(())
            })?;
            Ok(chosen)
        })?;

        if let Some(target) = target {
            if target != current_pane {
                herdr::focus_pane(&target)?;
            }
        }
        Ok(())
    }

    pub fn cycle_panes_logic<F>(
        current_pane: &str,
        panes: &[String],
        state: &mut State,
        now: f64,
        mut focus: F,
    ) -> Result<()>
    where
        F: FnMut(&str) -> Result<()>,
    {
        if panes.len() < 2 {
            return Ok(());
        }

        let mut history: Vec<String> = state
            .history
            .iter()
            .filter(|p| panes.contains(p))
            .cloned()
            .collect();
        history = promote(&history, current_pane);

        let known: std::collections::HashSet<_> = history.iter().cloned().collect();
        for pane in panes {
            if !known.contains(pane) {
                history.push(pane.clone());
            }
        }

        let continuing = if let Some(ref cycle) = state.cycle {
            let elapsed = now - cycle.last_at;
            (0.0..=CYCLE_TIMEOUT_SECONDS).contains(&elapsed)
                && cycle.target == current_pane
                && panes.iter().all(|p| known.contains(p))
        } else {
            false
        };

        let order = history.clone();
        let index = if continuing {
            let cycle = state.cycle.as_ref().unwrap();
            (cycle.index + 1) % order.len()
        } else {
            1
        };

        let target = order
            .get(index)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("cycle index out of bounds"))?;

        state.history = promote(&history, &target);
        state.cycle = Some(Cycle {
            index,
            target: target.clone(),
            last_at: now,
        });

        if target != current_pane {
            focus(&target)?;
        }

        Ok(())
    }

    pub fn attention_panes(panes: &[Pane]) -> Vec<&Pane> {
        let priority = status_priority();
        let mut attention: Vec<&Pane> = panes
            .iter()
            .filter(|p| {
                p.agent_status
                    .as_ref()
                    .map(|s| priority.contains_key(s))
                    .unwrap_or(false)
            })
            .collect();
        attention.sort_by_key(|p| priority[p.agent_status.as_deref().unwrap()]);
        attention
    }

    pub fn focus_attention(next_pane: bool) -> Result<()> {
        let panes = herdr::all_panes()?;
        let attention = attention_panes(&panes);
        if attention.is_empty() {
            return Ok(());
        }

        let current = env::active_pane_id()?;
        let target = if next_pane {
            if let Some(current) = current.as_deref() {
                match attention.iter().position(|p| p.pane_id == current) {
                    Some(i) if i + 1 < attention.len() => attention[i + 1],
                    _ => attention[0],
                }
            } else {
                attention[0]
            }
        } else {
            attention[0]
        };

        if current.as_deref() != Some(target.pane_id.as_str()) {
            herdr::focus_pane(&target.pane_id)?;
        }

        Ok(())
    }
}

fn run() -> Result<()> {
    let event = std::env::var("HERDR_PLUGIN_EVENT")
        .ok()
        .filter(|s| !s.is_empty());
    let action = std::env::var("HERDR_PLUGIN_ACTION_ID")
        .ok()
        .filter(|s| !s.is_empty());

    if let Some(ref e) = event {
        if e == "pane.focused" || e == "pane.closed" {
            if let Some(pane_id) = env::current_pane_id()? {
                actions::record_event(&pane_id, e)?;
            }
            return Ok(());
        }
    }

    match action.as_deref() {
        Some("focus-attention") => actions::focus_attention(false),
        Some("cycle-attention") => actions::focus_attention(true),
        _ => {
            if let Some(pane_id) = env::active_pane_id()? {
                actions::cycle_panes(&pane_id)?;
            }
            Ok(())
        }
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("{e:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn promote_empty() {
        assert_eq!(actions::promote(&[], "p1"), vec!["p1".to_string()]);
    }

    #[test]
    fn promote_duplicate_not_added() {
        let history = vec!["a".to_string(), "b".to_string()];
        assert_eq!(actions::promote(&history, "a"), vec!["a", "b"]);
    }

    #[test]
    fn promote_already_first() {
        let history = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(actions::promote(&history, "a"), vec!["a", "b", "c"]);
    }

    #[test]
    fn promote_moves_to_front_and_removes_old() {
        let history = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(actions::promote(&history, "c"), vec!["c", "a", "b"]);
    }

    #[test]
    fn attention_sorting_priority_and_stability() {
        let panes = vec![
            herdr::Pane {
                pane_id: "p3".into(),
                agent_status: Some("working".into()),
                title: None,
            },
            herdr::Pane {
                pane_id: "p0".into(),
                agent_status: Some("blocked".into()),
                title: None,
            },
            herdr::Pane {
                pane_id: "p2".into(),
                agent_status: Some("idle".into()),
                title: None,
            },
            herdr::Pane {
                pane_id: "p1".into(),
                agent_status: Some("done".into()),
                title: None,
            },
            herdr::Pane {
                pane_id: "p0b".into(),
                agent_status: Some("blocked".into()),
                title: None,
            },
        ];
        let sorted = actions::attention_panes(&panes);
        let ids: Vec<_> = sorted.iter().map(|p| p.pane_id.as_str()).collect();
        assert_eq!(ids, vec!["p0", "p0b", "p1", "p2", "p3"]);
    }

    #[test]
    fn attention_ignores_unknown_status() {
        let panes = vec![
            herdr::Pane {
                pane_id: "p1".into(),
                agent_status: Some("unknown".into()),
                title: None,
            },
            herdr::Pane {
                pane_id: "p0".into(),
                agent_status: Some("blocked".into()),
                title: None,
            },
        ];
        let sorted = actions::attention_panes(&panes);
        assert_eq!(sorted.len(), 1);
        assert_eq!(sorted[0].pane_id, "p0");
    }

    fn make_state(history: Vec<String>, cycle: Option<state::Cycle>) -> state::State {
        state::State {
            version: 2,
            history,
            cycle,
        }
    }

    #[test]
    fn cycle_first_returns_second_mru_pane() {
        let panes = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let mut state = make_state(vec![], None);
        let mut focused = Vec::new();

        actions::cycle_panes_logic("a", &panes, &mut state, 0.0, |t| {
            focused.push(t.to_string());
            Ok(())
        })
        .unwrap();

        assert_eq!(focused, vec!["b"]);
        assert_eq!(state.history, vec!["b", "a", "c"]);
        let cycle = state.cycle.as_ref().unwrap();
        assert_eq!(cycle.index, 1);
        assert_eq!(cycle.target, "b");
    }

    #[test]
    fn cycle_repeated_within_timeout_advances() {
        let panes = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let mut state = make_state(vec![], None);

        actions::cycle_panes_logic("a", &panes, &mut state, 0.0, |_| Ok(())).unwrap();
        assert_eq!(state.cycle.as_ref().unwrap().target, "b");

        let mut focused = Vec::new();
        actions::cycle_panes_logic("b", &panes, &mut state, 0.2, |t| {
            focused.push(t.to_string());
            Ok(())
        })
        .unwrap();

        assert_eq!(focused, vec!["c"]);
        assert_eq!(state.cycle.as_ref().unwrap().index, 2);
        assert_eq!(state.cycle.as_ref().unwrap().target, "c");
        assert_eq!(state.history, vec!["c", "b", "a"]);
    }

    #[test]
    fn cycle_after_timeout_restarts() {
        let panes = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let mut state = make_state(vec![], None);

        actions::cycle_panes_logic("a", &panes, &mut state, 0.0, |_| Ok(())).unwrap();
        let mut focused = Vec::new();
        actions::cycle_panes_logic("b", &panes, &mut state, 2.0, |t| {
            focused.push(t.to_string());
            Ok(())
        })
        .unwrap();

        assert_eq!(focused, vec!["a"]);
        assert_eq!(state.cycle.as_ref().unwrap().index, 1);
        assert_eq!(state.cycle.as_ref().unwrap().target, "a");
    }

    #[test]
    fn cycle_empty_pane_list_is_no_op() {
        let panes: Vec<String> = vec![];
        let mut state = make_state(vec![], None);
        let mut focused = false;

        actions::cycle_panes_logic("a", &panes, &mut state, 0.0, |_t| {
            focused = true;
            Ok(())
        })
        .unwrap();

        assert!(!focused);
        assert!(state.cycle.is_none());
    }

    #[test]
    fn cycle_single_pane_is_no_op() {
        let panes = vec!["a".to_string()];
        let mut state = make_state(vec![], None);
        let mut focused = false;

        actions::cycle_panes_logic("a", &panes, &mut state, 0.0, |_t| {
            focused = true;
            Ok(())
        })
        .unwrap();

        assert!(!focused);
        assert!(state.cycle.is_none());
    }

    #[test]
    fn cycle_removed_pane_excluded() {
        let panes = vec!["a".to_string(), "c".to_string()];
        let mut state = make_state(
            vec!["b".to_string(), "a".to_string(), "c".to_string()],
            None,
        );
        let mut focused = Vec::new();

        actions::cycle_panes_logic("a", &panes, &mut state, 0.0, |t| {
            focused.push(t.to_string());
            Ok(())
        })
        .unwrap();

        assert_eq!(focused, vec!["c"]);
        assert_eq!(state.history, vec!["c", "a"]);
        assert!(state.history.iter().find(|p| *p == "b").is_none());
    }

    #[test]
    fn cycle_target_equals_current_skips_focus() {
        // When the next pane in the MRU order wraps around to the current pane,
        // no pane.focus call should be emitted.
        let panes = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        // previous cycle: order [a, b, c] with target c at index 2.
        // After the cycle, state.history is [c, a, b].
        let mut state = make_state(
            vec!["c".to_string(), "a".to_string(), "b".to_string()],
            Some(state::Cycle {
                index: 2,
                target: "c".to_string(),
                last_at: 0.0,
            }),
        );
        let mut focused = false;

        // current pane is c; next index (2 + 1) % 3 = 0 wraps to c itself.
        actions::cycle_panes_logic("c", &panes, &mut state, 0.2, |_t| {
            focused = true;
            Ok(())
        })
        .unwrap();

        assert!(!focused);
        assert_eq!(state.cycle.as_ref().unwrap().target, "c");
        assert_eq!(state.cycle.as_ref().unwrap().index, 0);
    }

    #[test]
    fn state_round_trip() {
        let tmp = std::env::temp_dir().join(format!("herdr-state-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _cleanup = defer::Defer::new(|| {
            std::fs::remove_dir_all(&tmp).ok();
        });

        state::with_state(&tmp, |s| {
            s.history = vec!["a".to_string(), "b".to_string()];
            s.cycle = Some(state::Cycle {
                index: 1,
                target: "b".to_string(),
                last_at: 1.0,
            });
            Ok(())
        })
        .unwrap();

        state::with_state(&tmp, |s| {
            assert_eq!(s.history, vec!["a", "b"]);
            assert_eq!(s.version, 2);
            assert!(s.cycle.is_some());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn state_corrupt_bytes_reset_to_default() {
        let tmp = std::env::temp_dir().join(format!("herdr-state-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _cleanup = defer::Defer::new(|| {
            std::fs::remove_dir_all(&tmp).ok();
        });

        let path = tmp.join("state.bin");
        std::fs::write(&path, b"not postcard data").unwrap();

        state::with_state(&tmp, |s| {
            assert_eq!(*s, state::State::default());
            s.history.push("a".to_string());
            Ok(())
        })
        .unwrap();

        state::with_state(&tmp, |s| {
            assert_eq!(s.history, vec!["a"]);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn state_version_mismatch_resets() {
        let tmp = std::env::temp_dir().join(format!("herdr-state-version-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let _cleanup = defer::Defer::new(|| {
            std::fs::remove_dir_all(&tmp).ok();
        });

        let bad = state::State {
            version: 42,
            history: vec!["x".to_string()],
            cycle: None,
        };
        let bytes = state::to_bytes(&bad);
        std::fs::write(tmp.join("state.bin"), bytes).unwrap();

        state::with_state(&tmp, |s| {
            assert_eq!(*s, state::State::default());
            Ok(())
        })
        .unwrap();
    }

    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    fn set_env(vars: &[(&str, &str)]) {
        for (k, v) in vars {
            // SAFETY: env-var tests are serialized by ENV_MUTEX and only run
            // in single-threaded unit tests. std::env::set_var is safe in
            // Rust 2021 and is wrapped in unsafe for forward-compatibility.
            unsafe { std::env::set_var(k, v) };
        }
    }

    fn clear_env(keys: &[&str]) {
        for k in keys {
            unsafe { std::env::remove_var(k) };
        }
    }

    #[test]
    fn env_missing_state_dir_errors() {
        let _guard = ENV_MUTEX.lock().unwrap();
        clear_env(&["HERDR_PLUGIN_STATE_DIR"]);
        let err = env::state_dir().unwrap_err().to_string();
        assert!(err.contains("HERDR_PLUGIN_STATE_DIR"));
    }

    #[test]
    fn env_malformed_event_json_errors() {
        let _guard = ENV_MUTEX.lock().unwrap();
        set_env(&[
            ("HERDR_PLUGIN_EVENT", "pane.focused"),
            ("HERDR_PLUGIN_EVENT_JSON", "not json"),
        ]);
        clear_env(&["HERDR_PANE_ID"]);

        let err = env::current_pane_id().unwrap_err().to_string();
        assert!(err.contains("malformed HERDR_PLUGIN_EVENT_JSON"));

        clear_env(&["HERDR_PLUGIN_EVENT", "HERDR_PLUGIN_EVENT_JSON"]);
    }

    #[test]
    fn env_current_pane_event_takes_precedence() {
        let _guard = ENV_MUTEX.lock().unwrap();
        set_env(&[
            ("HERDR_PLUGIN_EVENT", "pane.focused"),
            (
                "HERDR_PLUGIN_EVENT_JSON",
                r#"{"outer": {"pane_id": "event-pane"}}"#,
            ),
            ("HERDR_PANE_ID", "env-pane"),
        ]);

        assert_eq!(
            env::current_pane_id().unwrap(),
            Some("event-pane".to_string())
        );
        clear_env(&[
            "HERDR_PLUGIN_EVENT",
            "HERDR_PLUGIN_EVENT_JSON",
            "HERDR_PANE_ID",
        ]);
    }

    #[test]
    fn env_current_pane_env_var_overrides_json() {
        let _guard = ENV_MUTEX.lock().unwrap();
        set_env(&[
            ("HERDR_PLUGIN_EVENT_JSON", r#"{"pane_id": "json-pane"}"#),
            ("HERDR_PANE_ID", "env-pane"),
        ]);
        clear_env(&["HERDR_PLUGIN_EVENT"]);

        assert_eq!(
            env::current_pane_id().unwrap(),
            Some("env-pane".to_string())
        );
        clear_env(&["HERDR_PLUGIN_EVENT_JSON", "HERDR_PANE_ID"]);
    }

    #[test]
    fn env_current_pane_focused_pane_id_precedence() {
        let _guard = ENV_MUTEX.lock().unwrap();
        set_env(&[(
            "HERDR_PLUGIN_EVENT_JSON",
            r#"{"focused_pane_id": "focused", "pane_id": "json"}"#,
        )]);
        clear_env(&["HERDR_PLUGIN_EVENT", "HERDR_PANE_ID"]);

        assert_eq!(env::current_pane_id().unwrap(), Some("focused".to_string()));
        clear_env(&["HERDR_PLUGIN_EVENT_JSON"]);
    }

    #[test]
    fn env_current_pane_nested_pane_id() {
        let _guard = ENV_MUTEX.lock().unwrap();
        set_env(&[(
            "HERDR_PLUGIN_EVENT_JSON",
            r#"{"nested": {"pane_id": "deep"}}"#,
        )]);
        clear_env(&["HERDR_PLUGIN_EVENT", "HERDR_PANE_ID"]);

        assert_eq!(env::current_pane_id().unwrap(), Some("deep".to_string()));
        clear_env(&["HERDR_PLUGIN_EVENT_JSON"]);
    }

    mod defer {
        pub struct Defer<F: FnOnce()>(Option<F>);

        impl<F: FnOnce()> Defer<F> {
            pub fn new(f: F) -> Self {
                Self(Some(f))
            }
        }

        impl<F: FnOnce()> Drop for Defer<F> {
            fn drop(&mut self) {
                if let Some(f) = self.0.take() {
                    f();
                }
            }
        }
    }
}
