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
    warmup_done: bool,
    quiescent: bool,
    pub workspace_status: Value,
    pub identity: Value,
    pub witnesses: Vec<Value>,
}
fn bounded_version(executable: &Path, budget:Instant) -> Result<Vec<u8>> {
    require(Instant::now()<budget,"ANALYZER","Deadline exceeded before version probe")?;
    let mut child = Command::new(executable)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = (Instant::now() + Duration::from_secs(5)).min(budget);
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
        thread::sleep(Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())));
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
        let deadline=Instant::now()+timeout;
        let version = String::from_utf8(bounded_version(executable,deadline)?)?
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
        // Discover installed standard-library sources; dependencies remain
        // offline and build scripts, checks and procedural macros stay disabled.
        let config = json!({"cargo":{"buildScripts":{"enable":false},"noDeps":true,"features":features,"allTargets":true,"sysroot":"discover"},"procMacro":{"enable":false},"checkOnSave":false,"cachePriming":{"enable":false},"diagnostics":{"enable":false}});
        require(Instant::now()<deadline,"ANALYZER","Deadline exceeded before server start")?;
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
            deadline,
            warmup_done:false,
            quiescent:false,
            workspace_status:json!({"state":"not_observed"}),
            identity: json!({"name":"rust-analyzer-lsp","version":version,"adapter_digest":implementation,"launcher_digest":launcher_digest,"settings":config}),
            witnesses: Vec::new(),
        };
        a.request("initialize",json!({"processId":null,"rootUri":uri(root),"capabilities":{"general":{"positionEncodings":["utf-16"]},"experimental":{"serverStatusNotification":true},"textDocument":{"definition":{"linkSupport":true}},"window":{"workDoneProgress":true}},"initializationOptions":config,"workspaceFolders":[{"uri":uri(root),"name":"captured-source"}]}))?;
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
        self.request_until(method,params,self.deadline)
    }
    pub fn expired(&self) -> bool {Instant::now()>=self.deadline}
    pub fn workspace_ready(&self)->bool {self.quiescent && self.workspace_status["health"]!="error"}
    fn handle_server_message(&mut self,message:&Value) -> Result<()> {
        if message["method"]=="experimental/serverStatus" {
            self.quiescent=message["params"]["quiescent"]==true;
            self.workspace_status=message["params"].clone();
        }
        if !message["method"].is_null() && !message["id"].is_null() {
            let result=if message["method"]=="workspace/configuration" {
                json!(message["params"]["items"].as_array().map(|items|items.iter().map(|_|
                    self.identity["settings"].clone()).collect::<Vec<_>>()).unwrap_or_default())
            } else {Value::Null};
            self.send(&json!({"jsonrpc":"2.0","id":message["id"],"result":result}))?;
        }
        Ok(())
    }
    /// One readiness window per process, never one wait for every null lookup.
    fn warmup(&mut self) -> Result<()> {
        if self.warmup_done {return Ok(());}
        self.warmup_done=true;
        let deadline=(Instant::now()+Duration::from_secs(5)).min(self.deadline);
        while !self.quiescent && Instant::now()<deadline {
            match self.messages.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(Ok(message))=>self.handle_server_message(&message)?,
                Ok(Err(error))=>return Err(format!("ANALYZER: {error}").into()),
                Err(mpsc::RecvTimeoutError::Timeout)=>break,
                Err(mpsc::RecvTimeoutError::Disconnected)=>return Err("ANALYZER: Server exited during workspace loading".into()),
            }
        }
        if !self.quiescent {self.workspace_status=json!({"state":"readiness_unconfirmed","bounded_warmup_seconds":5});}
        Ok(())
    }
    fn request_until(&mut self,method:&str,params:Value,deadline:Instant) -> Result<Value> {
        let deadline=deadline.min(self.deadline);
        require(Instant::now()<deadline,"ANALYZER","Deadline exceeded")?;
        self.next += 1;
        let id = self.next;
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?;
        loop {
            let duration = self
                .deadline.min(deadline)
                .checked_duration_since(Instant::now())
                .ok_or("ANALYZER: Deadline exceeded")?;
            let received=self.messages.recv_timeout(duration);
            if received.is_err() {
                let _=self.notify("$/cancelRequest",json!({"id":id}));
                return Err("ANALYZER: Deadline exceeded or server exited".into());
            }
            let message=received.unwrap().map_err(|e|format!("ANALYZER: {e}"))?;
            self.handle_server_message(&message)?;
            if !message["method"].is_null() && !message["id"].is_null() {
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
        self.warmup()?;
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
        let retry_deadline = (Instant::now() + Duration::from_secs(2)).min(self.deadline);
        for attempt in 0..3 {
            let response = self.request_until("textDocument/definition", params.clone(),retry_deadline);
            match response {
                Ok(v) if !v.is_null() && v != json!([]) => {
                    self.witnesses.push(json!({"source_uri":uri(path),"position":params["position"],"definitions":v}));
                    return Ok(v);
                }
                Err(e) if e.to_string().contains("-32801") && attempt<2 && Instant::now() < retry_deadline => {
                    thread::sleep(Duration::from_millis(20).min(retry_deadline.saturating_duration_since(Instant::now())))
                }
                Ok(v) => return Ok(v),
                Err(e) => return Err(e),
            }
        }
        unreachable!("final request attempt always returns")
    }
}
impl Drop for Analyzer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(all(test,unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn server(mode:&str)->(tempfile::TempDir,std::path::PathBuf) {
        let dir=tempfile::tempdir().unwrap();let executable=dir.path().join("fake-analyzer");
        let script=r#"#!/usr/bin/env python3
import json,sys,time
MODE = '__MODE__'
if '--version' in sys.argv:
    if MODE == 'slow_version': time.sleep(30)
    print('rust-analyzer fixture');sys.exit(0)
def send(message):
    raw=json.dumps(message).encode();sys.stdout.buffer.write(('Content-Length: %d\r\n\r\n'%len(raw)).encode()+raw);sys.stdout.buffer.flush()
while True:
    headers={}
    while True:
        line=sys.stdin.buffer.readline()
        if not line:sys.exit(0)
        if line in (b'\r\n',b'\n'):break
        key,value=line.decode().split(':',1);headers[key.lower()]=value.strip()
    message=json.loads(sys.stdin.buffer.read(int(headers['content-length'])))
    method=message.get('method')
    if method == 'initialized':
        send({'jsonrpc':'2.0','method':'experimental/serverStatus','params':{'health':'ok','quiescent':True}})
    if 'id' not in message:continue
    if method == 'textDocument/definition' and MODE == 'stalled':time.sleep(30)
    send({'jsonrpc':'2.0','id':message['id'],'result':{'capabilities':{}} if method=='initialize' else None})
"#.replace("__MODE__",mode);
        std::fs::write(&executable,script).unwrap();std::fs::set_permissions(&executable,std::fs::Permissions::from_mode(0o700)).unwrap();
        (dir,executable)
    }
    #[test]
    fn ready_workspace_null_definitions_are_single_misses_without_per_site_waits() {
        let (dir,exe)=server("null");let started=Instant::now();
        let mut analyzer=Analyzer::start(&exe,dir.path(),Duration::from_secs(10),&[]).unwrap();
        let file=dir.path().join("lib.rs");analyzer.open(&file,"fn a() {}\n").unwrap();
        for _ in 0..50 {assert!(analyzer.definition(&file,"fn a() {}\n",3).unwrap().is_null());}
        assert_eq!(analyzer.next,51,"initialize plus exactly one native request per definition");
        assert!(started.elapsed()<Duration::from_secs(3));assert!(analyzer.quiescent);
        assert_eq!(analyzer.identity["settings"]["cargo"]["sysroot"],"discover");
        assert_eq!(analyzer.identity["settings"]["cargo"]["buildScripts"]["enable"],false);
    }
    #[test]
    fn one_deadline_bounds_version_startup_and_a_stalled_definition() {
        for mode in ["slow_version","stalled"] {
            let (dir,exe)=server(mode);let started=Instant::now();
            let result=Analyzer::start(&exe,dir.path(),Duration::from_millis(250),&[]);
            if mode=="slow_version" {assert!(result.is_err());}
            else {
                let mut analyzer=result.unwrap();let file=dir.path().join("lib.rs");
                analyzer.open(&file,"fn a() {}\n").unwrap();assert!(analyzer.definition(&file,"fn a() {}\n",3).is_err());
            }
            assert!(started.elapsed()<Duration::from_secs(2),"{mode}");
        }
    }
}
