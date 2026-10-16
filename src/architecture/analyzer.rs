//! A bounded rust-analyzer LSP client. Build scripts, macros, checks and dependency fetching are disabled.
use super::{identity, require, Result};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

pub struct Analyzer {
    child: Child,
    input: ChildStdin,
    messages: Receiver<std::result::Result<Value, String>>,
    next: u64,
    deadline: Instant,
    pub identity: Value,
    pub witnesses: Vec<Value>,
}
fn bounded_version(executable: &Path) -> Result<Vec<u8>> {
    let mut child = Command::new(executable)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait()? {
            require(status.success(), "ANALYZER", "Version probe failed")?;
            let mut bytes = Vec::new();
            child
                .stdout
                .take()
                .unwrap()
                .take(65536)
                .read_to_end(&mut bytes)?;
            return Ok(bytes);
        }
        if Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            return Err("ANALYZER: Version probe timed out".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}
pub fn uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~:".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}
pub fn path_from_uri(uri: &str) -> Result<std::path::PathBuf> {
    let encoded = uri
        .strip_prefix("file://")
        .ok_or("ANALYZER: Non-file target URI")?;
    let mut out = Vec::new();
    let mut chars = encoded.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let h = [
                chars.next().ok_or("URI escape")?,
                chars.next().ok_or("URI escape")?,
            ];
            out.push(u8::from_str_radix(std::str::from_utf8(&h)?, 16)?);
        } else {
            out.push(b);
        }
    }
    Ok(String::from_utf8(out)?.into())
}
impl Analyzer {
    pub fn start(
        executable: &Path,
        root: &Path,
        timeout: Duration,
        features: &[String],
    ) -> Result<Self> {
        let version = String::from_utf8(bounded_version(executable)?)?
            .trim()
            .to_owned();
        let implementation = identity::bytes_digest(include_bytes!("analyzer.rs"));
        let launcher = if executable.is_file() {
            Some(executable.to_path_buf())
        } else {
            std::env::var_os("PATH").and_then(|path| {
                std::env::split_paths(&path)
                    .map(|p| p.join(executable))
                    .find(|p| p.is_file())
            })
        };
        let launcher_digest = launcher
            .and_then(|p| std::fs::read(p).ok())
            .map(|bytes| identity::bytes_digest(&bytes));
        let config = json!({"cargo":{"buildScripts":{"enable":false},"noDeps":true,"features":features,"allTargets":false,"sysroot":null},"procMacro":{"enable":false},"checkOnSave":false,"cachePriming":{"enable":false},"diagnostics":{"enable":false}});
        let mut child = Command::new(executable)
            .current_dir(root)
            .env("CARGO_NET_OFFLINE", "true")
            .env("RUSTC_WRAPPER", "")
            .env("RUSTC_WORKSPACE_WRAPPER", "")
            .env("RA_LOG", "error")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::sync_channel(256);
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut size = None;
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(0) => return,
                        Ok(_) => {}
                        Err(e) => {
                            let _ = tx.send(Err(e.to_string()));
                            return;
                        }
                    }
                    if line.trim().is_empty() {
                        break;
                    }
                    if let Some(s) = line.strip_prefix("Content-Length:") {
                        size = s.trim().parse::<usize>().ok();
                    }
                }
                let Some(size) = size.filter(|n| *n <= 16 * 1024 * 1024) else {
                    let _ = tx.send(Err("Invalid LSP frame size".into()));
                    return;
                };
                let mut bytes = vec![0; size];
                if reader.read_exact(&mut bytes).is_err() {
                    return;
                }
                let result = identity::parse(&bytes).map_err(|e| e.to_string());
                if tx.send(result).is_err() {
                    return;
                }
            }
        });
        let mut a = Self {
            child,
            input,
            messages: rx,
            next: 0,
            deadline: Instant::now() + timeout,
            identity: json!({"name":"rust-analyzer-lsp","version":version,"adapter_digest":implementation,"launcher_digest":launcher_digest,"settings":config}),
            witnesses: Vec::new(),
        };
        a.request("initialize",json!({"processId":null,"rootUri":uri(root),"capabilities":{"general":{"positionEncodings":["utf-16"]},"textDocument":{"definition":{"linkSupport":true}},"window":{"workDoneProgress":true}},"initializationOptions":config,"workspaceFolders":[{"uri":uri(root),"name":"captured-source"}]}))?;
        a.notify("initialized", json!({}))?;
        Ok(a)
    }
    fn send(&mut self, v: &Value) -> Result<()> {
        let bytes = serde_json::to_vec(v)?;
        write!(self.input, "Content-Length: {}\r\n\r\n", bytes.len())?;
        self.input.write_all(&bytes)?;
        self.input.flush()?;
        Ok(())
    }
    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(&json!({"jsonrpc":"2.0","method":method,"params":params}))
    }
    pub fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next += 1;
        let id = self.next;
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?;
        loop {
            let duration = self
                .deadline
                .checked_duration_since(Instant::now())
                .ok_or("ANALYZER: Deadline exceeded")?;
            let message = self
                .messages
                .recv_timeout(duration)
                .map_err(|_| "ANALYZER: Deadline exceeded or server exited")?
                .map_err(|e| format!("ANALYZER: {e}"))?;
            if !message["method"].is_null() && !message["id"].is_null() {
                let result = if message["method"] == "workspace/configuration" {
                    json!(message["params"]["items"]
                        .as_array()
                        .map(|items| items
                            .iter()
                            .map(|_| self.identity["settings"].clone())
                            .collect::<Vec<_>>())
                        .unwrap_or_default())
                } else {
                    Value::Null
                };
                self.send(&json!({"jsonrpc":"2.0","id":message["id"],"result":result}))?;
                continue;
            }
            if message["id"].as_u64() == Some(id) {
                if !message["error"].is_null() {
                    return Err(format!("ANALYZER: {}", message["error"]).into());
                }
                return Ok(message["result"].clone());
            }
        }
    }
    pub fn open(&mut self, path: &Path, source: &str) -> Result<()> {
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument":{"uri":uri(path),"languageId":"rust","version":1,"text":source}}),
        )
    }
    pub fn definition(&mut self, path: &Path, source: &str, byte: usize) -> Result<Value> {
        require(
            source.is_char_boundary(byte),
            "ANALYZER",
            "Invalid request byte position",
        )?;
        let prefix = &source[..byte];
        let line = prefix.bytes().filter(|b| *b == b'\n').count();
        let column = prefix
            .rsplit('\n')
            .next()
            .unwrap_or("")
            .encode_utf16()
            .count();
        let params =
            json!({"textDocument":{"uri":uri(path)},"position":{"line":line,"character":column}});
        // ContentModified/-32801 and early null results are expected while workspace discovery finishes.
        let retry_deadline = (Instant::now() + Duration::from_secs(5)).min(self.deadline);
        loop {
            let response = self.request("textDocument/definition", params.clone());
            match response {
                Ok(v) if !v.is_null() && v != json!([]) => {
                    self.witnesses.push(json!({"source_uri":uri(path),"position":params["position"],"definitions":v}));
                    return Ok(v);
                }
                Ok(_) if Instant::now() < retry_deadline => {
                    thread::sleep(Duration::from_millis(80))
                }
                Err(e) if e.to_string().contains("-32801") && Instant::now() < retry_deadline => {
                    thread::sleep(Duration::from_millis(80))
                }
                Ok(v) => return Ok(v),
                Err(e) => return Err(e),
            }
        }
    }
}
impl Drop for Analyzer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
