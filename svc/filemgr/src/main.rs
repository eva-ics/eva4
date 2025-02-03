use eva_common::common_payloads::ValueOrList;
use eva_common::prelude::*;
use eva_common::time::Time;
use eva_sdk::fs as sdkfs;
use eva_sdk::http;
use eva_sdk::prelude::*;
use once_cell::sync::OnceCell;
use openssl::sha::Sha256;
use parking_lot::Mutex as SyncMutex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::{HashMap, HashSet};
use std::iter::FromIterator;
use std::mem;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

mod terminal;

err_logger!();

const DEFAULT_COLS: usize = 120;
const DEFAULT_ROWS: usize = 38;

lazy_static::lazy_static! {
    static ref TIMEOUT: OnceCell<Duration> = <_>::default();
    static ref TERMINAL: OnceCell<Mutex<TerminalProcess>> = <_>::default();
    static ref TERMINAL_GLOBAL_INPUT_TX: OnceCell<async_channel::Sender<Option<Vec<u8>>>> = <_>::default();
    static ref TERMINAL_DIMENSIONS: SyncMutex<(usize, usize)> = SyncMutex::new((DEFAULT_COLS, DEFAULT_ROWS));
}

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "File management service";

const DEFAULT_MIME: &str = "application/octet-stream";

const TERMINAL_BUFFER_SIZE: usize = 500_000;

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

struct TerminalProcess {
    pid: Option<u32>,
    output: Vec<terminal::Output>,
    input_tx: Option<async_channel::Sender<Vec<u8>>>,
}

impl Default for TerminalProcess {
    fn default() -> Self {
        TerminalProcess {
            pid: None,
            output: Vec::new(),
            input_tx: None,
        }
    }
}

impl TerminalProcess {
    async fn terminate(&mut self) {
        {
            self.input_tx.take();
        }
        self.output.clear();
        if let Some(pid) = self.pid {
            bmart::process::kill_pstree(pid, Some(Duration::from_secs(1)), true).await;
        }
    }
}

fn terminal_process() -> &'static Mutex<TerminalProcess> {
    TERMINAL.get().unwrap()
}

#[derive(Deserialize, Eq, PartialEq, Copy, Clone, Default)]
#[serde(rename_all = "lowercase")]
enum Extract {
    #[default]
    No,
    Tar,
    Txz,
    Tgz,
    Tbz2,
    Zip,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum TerminalInput {
    Text(String),
    Binary(Vec<u8>),
}

impl From<TerminalInput> for Vec<u8> {
    fn from(input: TerminalInput) -> Self {
        match input {
            TerminalInput::Text(v) => v.into_bytes(),
            TerminalInput::Binary(v) => v,
        }
    }
}

struct Handlers {
    info: ServiceInfo,
    runtime_dir: PathBuf,
    protected: HashSet<PathBuf>,
    mime_types: HashMap<String, String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum Mode {
    #[serde(alias = "t")]
    Text,
    #[serde(alias = "b")]
    Binary,
    #[default]
    #[serde(alias = "i")]
    Info,
    #[serde(alias = "x", rename = "extended_info")]
    ExtendedInfo,
}

#[derive(Deserialize, Copy, Clone, Default)]
#[serde(rename_all = "lowercase")]
enum Caller {
    #[serde(alias = "h")]
    Human,
    #[default]
    #[serde(alias = "m")]
    Machine,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Permissions {
    Oct(String),
    Dec(u32),
    Executable(bool),
}

#[derive(Serialize, Deserialize)]
struct Executable {
    #[serde(alias = "x")]
    executable: bool,
}

impl Permissions {
    fn new(p: u32, caller: Caller) -> Self {
        match caller {
            Caller::Human => Permissions::Oct(format!("{:#o}", p)),
            Caller::Machine => Permissions::Dec(p),
        }
    }
}

impl TryFrom<Permissions> for u32 {
    type Error = Error;
    fn try_from(value: Permissions) -> EResult<u32> {
        match value {
            Permissions::Dec(v) => Ok(v),
            Permissions::Oct(v) => Ok(parse_int::parse(&v)?),
            Permissions::Executable(v) => Ok(if v { 0o755 } else { 0o644 }),
        }
    }
}

#[inline]
fn sha256sum(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finish()
}

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
enum Sha256Checksum {
    Str(String),
    Bytes([u8; 32]),
}

impl Sha256Checksum {
    fn new(digest: [u8; 32], caller: Caller) -> Self {
        match caller {
            Caller::Human => Sha256Checksum::Str(hex::encode(digest)),
            Caller::Machine => Sha256Checksum::Bytes(digest),
        }
    }
    fn compare(&self, content: &[u8]) -> EResult<()> {
        let sum = sha256sum(content);
        if match self {
            Sha256Checksum::Str(v) => hex::decode(v).map_err(Error::invalid_data)? == sum,
            Sha256Checksum::Bytes(v) => v == &sum,
        } {
            Ok(())
        } else {
            Err(Error::invalid_data("checksum mismatch (sha256)"))
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Content {
    Binary(Vec<u8>),
    Text(String),
}

impl Content {
    #[inline]
    fn as_bytes(&self) -> &[u8] {
        match self {
            Content::Binary(v) => v.as_slice(),
            Content::Text(v) => v.as_bytes(),
        }
    }
    #[inline]
    fn as_str(&self) -> EResult<&str> {
        match self {
            Content::Binary(v) => std::str::from_utf8(v).map_err(Into::into),
            Content::Text(v) => Ok(v.as_str()),
        }
    }
}

#[inline]
fn default_permissions() -> Permissions {
    Permissions::Dec(33188)
}

async fn extract_archive(src: &Path, extract: Extract, dest: &Path) -> EResult<()> {
    let src_file = src.to_string_lossy();
    let dest_dir = dest.to_string_lossy();
    let (command, args) = match extract {
        Extract::No => return Ok(()),
        Extract::Tgz => ("tar", vec!["hxzf", &src_file, "-C", &dest_dir]),
        Extract::Txz | Extract::Tar => ("tar", vec!["hxf", &src_file, "-C", &dest_dir]),
        Extract::Tbz2 => ("tar", vec!["hxjf", &src_file, "-C", &dest_dir]),
        Extract::Zip => {
            let s: &str = &src_file;
            ("unzip", vec!["-f", s, "-d", &dest_dir])
        }
    };
    let timeout = *TIMEOUT.get().unwrap();
    let res =
        bmart::process::command(command, args, timeout, bmart::process::Options::default()).await?;
    if !res.out.is_empty() {
        info!("{}", res.out.join("\n"));
    }
    if !res.err.is_empty() {
        error!("{}", res.err.join("\n"));
    }
    if res.ok() {
        Ok(())
    } else {
        Err(Error::failed(format!(
            "command {} exit code: {}",
            command,
            res.code.unwrap_or(-1)
        )))
    }
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    #[allow(clippy::too_many_lines)]
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        let method = event.parse_method()?;
        let payload = event.payload();
        match method {
            "terminal.send_input" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct Params {
                        input: TerminalInput,
                    }
                    let p: Params = unpack(payload)?;
                    if let Some(tx) = TERMINAL_GLOBAL_INPUT_TX.get() {
                        tx.send(Some(p.input.into())).await.map_err(Error::failed)?;
                        Ok(None)
                    } else {
                        Err(Error::unsupported("terminal is not enabled").into())
                    }
                }
            }
            "terminal.restart" => {
                if let Some(tx) = TERMINAL_GLOBAL_INPUT_TX.get() {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct Payload {
                        dimensions: Option<(usize, usize)>,
                    }
                    if !payload.is_empty() {
                        let p: Payload = unpack(payload)?;
                        *TERMINAL_DIMENSIONS.lock() =
                            p.dimensions.unwrap_or((DEFAULT_COLS, DEFAULT_ROWS));
                    }
                    tx.send(None).await.map_err(Error::failed)?;
                    Ok(None)
                } else {
                    Err(Error::unsupported("terminal is not enabled").into())
                }
            }
            "terminal.take_output" => {
                if payload.is_empty() {
                    #[derive(Serialize)]
                    struct Output {
                        output: Vec<terminal::Output>,
                    }
                    let output = mem::take(&mut terminal_process().lock().await.output);
                    terminal_process().lock().await.output.clear();
                    Ok(Some(pack(&Output { output })?))
                } else {
                    Err(RpcError::params(None))
                }
            }
            "sh" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct CmdParams {
                        c: String,
                        #[serde(
                            default,
                            deserialize_with = "eva_common::tools::de_opt_float_as_duration"
                        )]
                        timeout: Option<Duration>,
                        stdin: Option<String>,
                        #[serde(default)]
                        check_exit_code: bool,
                    }
                    #[derive(Serialize)]
                    struct CmdResult {
                        exitcode: i32,
                        out: String,
                        err: String,
                    }
                    let mut p: CmdParams = unpack(payload)?;
                    let mut opts = bmart::process::Options::default();
                    if let Some(s) = p.stdin.take() {
                        opts = opts.input(std::borrow::Cow::Owned(s.as_bytes().to_vec()));
                    }
                    let result = bmart::process::command(
                        "sh",
                        ["-c", &p.c],
                        p.timeout.unwrap_or_else(|| *TIMEOUT.get().unwrap()),
                        opts,
                    )
                    .await?;
                    let exitcode = result.code.unwrap_or(-15);
                    if exitcode == 0 || !p.check_exit_code {
                        Ok(Some(pack(&CmdResult {
                            exitcode,
                            out: result.out.join("\n"),
                            err: result.err.join("\n"),
                        })?))
                    } else {
                        Err(Error::failed(format!(
                            "command exit code {}\n{}",
                            exitcode,
                            result.err.join("\n")
                        ))
                        .into())
                    }
                }
            }
            "file.get" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct GetParams {
                        #[serde(alias = "i")]
                        path: String,
                        #[serde(default, alias = "m")]
                        mode: Mode,
                        #[serde(default, alias = "c")]
                        caller: Caller,
                    }
                    #[derive(Serialize)]
                    struct GetResult<'a> {
                        content_type: &'a str,
                        path: &'a str,
                        #[serde(skip_serializing_if = "Option::is_none")]
                        content: Option<Vec<u8>>,
                        #[serde(skip_serializing_if = "Option::is_none")]
                        text: Option<String>,
                        permissions: Permissions,
                        size: u64,
                        modified: Option<f64>,
                        #[serde(skip_serializing_if = "Option::is_none")]
                        sha256: Option<Sha256Checksum>,
                    }
                    let p: GetParams = unpack(payload)?;
                    let f = PathBuf::from(&p.path);
                    info!("file.get {:?}", f);
                    let fpath = self.format_path(&f).log_err()?;
                    if !fpath.exists() {
                        return Err(Error::not_found("file not found").into());
                    }
                    let relpath = fpath
                        .strip_prefix(&self.runtime_dir)
                        .map_err(Error::io)?
                        .to_str()
                        .ok_or_else(|| Error::io("unable to decode path"))?;
                    let metadata = fs::metadata(&fpath).await?;
                    let modified = if let Ok(m) = metadata.modified() {
                        Some(TryInto::<Time>::try_into(m)?.timestamp())
                    } else {
                        None
                    };
                    let content_type = fpath.extension().map_or(DEFAULT_MIME, |ext| {
                        self.mime_types
                            .get(ext.to_string_lossy().as_ref())
                            .map_or(DEFAULT_MIME, String::as_str)
                    });
                    let mut result = GetResult {
                        content_type,
                        path: relpath,
                        content: None,
                        text: None,
                        permissions: Permissions::new(metadata.permissions().mode(), p.caller),
                        size: metadata.len(),
                        modified,
                        sha256: None,
                    };
                    match p.mode {
                        Mode::Binary => {
                            let content = fs::read(&fpath).await?;
                            result.sha256 =
                                Some(Sha256Checksum::new(sha256sum(&content), p.caller));
                            result.content = Some(content);
                        }
                        Mode::Text => {
                            let text = fs::read_to_string(&fpath).await?;
                            result.sha256 =
                                Some(Sha256Checksum::new(sha256sum(text.as_bytes()), p.caller));
                            result.text = Some(text);
                        }
                        Mode::Info => {}
                        Mode::ExtendedInfo => {
                            result.sha256 = Some(Sha256Checksum::new(
                                sha256sum(&fs::read(&fpath).await?),
                                p.caller,
                            ));
                        }
                    }
                    Ok(Some(pack(&result)?))
                }
            }
            "file.put" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct PutParams {
                        #[serde(alias = "i")]
                        path: String,
                        #[serde(alias = "c")]
                        content: Content,
                        #[serde(default = "default_permissions", alias = "x")]
                        permissions: Permissions,
                        #[serde(default)]
                        sha256: Option<Sha256Checksum>,
                        #[serde(default)]
                        extract: Extract,
                        #[serde(default)]
                        download: bool,
                    }
                    let p: PutParams = unpack(payload)?;
                    let f = PathBuf::from(&p.path);
                    info!("file.put {:?}", f);
                    let perm: u32 = p.permissions.try_into()?;
                    let http_resp = if p.download {
                        let client =
                            http::Client::new(1, *TIMEOUT.get().unwrap()).follow_redirects(true);
                        let url = p.content.as_str()?;
                        info!("downloading {}", url);
                        Some(client.get_response(url).await?)
                    } else {
                        None
                    };
                    let content = if let Some(ref resp) = http_resp {
                        if resp.status() != 200 {
                            return Err(
                                Error::failed(format!("HTTP response {}", resp.status())).into()
                            );
                        }
                        resp.body()
                    } else {
                        p.content.as_bytes()
                    };
                    if let Some(s) = p.sha256 {
                        s.compare(content)?;
                    }
                    let fpath = self.format_path(&f).log_err()?;
                    if p.extract == Extract::No {
                        if let Some(parent) = fpath.parent() {
                            fs::create_dir_all(parent).await?;
                        }
                        let _r = fs::remove_file(&fpath).await;
                        let mut fh = fs::OpenOptions::new()
                            .create(true)
                            .write(true)
                            .truncate(true)
                            .open(&fpath)
                            .await?;
                        let metadata = fh.metadata().await?;
                        let mut mp = metadata.permissions();
                        mp.set_mode(perm);
                        fs::set_permissions(&fpath, mp).await?;
                        fh.write_all(content).await?;
                        fh.flush().await?;
                    } else {
                        fs::create_dir_all(&fpath).await?;
                        let temp_file = tempfile::NamedTempFile::new()?;
                        let (std_fh, temp_path) = temp_file.keep().map_err(Error::io)?;
                        let mut fh = tokio::fs::File::from_std(std_fh);
                        let metadata = fh.metadata().await?;
                        let mut mp = metadata.permissions();
                        mp.set_mode(0o600);
                        fs::set_permissions(&temp_path, mp).await?;
                        fh.write_all(content).await?;
                        fh.flush().await?;
                        let res = extract_archive(&temp_path, p.extract, &fpath).await;
                        let _r = tokio::fs::remove_file(temp_path).await;
                        res?;
                    }
                    Ok(None)
                }
            }
            "file.unlink" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct UnlinkParams {
                        #[serde(alias = "i")]
                        path: String,
                    }
                    let p: UnlinkParams = unpack(payload)?;
                    let f = PathBuf::from(&p.path);
                    info!("file.unlink {:?}", f);
                    let fpath = self.format_path(&f).log_err()?;
                    if let Err(e) = fs::remove_file(&fpath).await {
                        if e.kind() != std::io::ErrorKind::NotFound {
                            return Err(e.into());
                        }
                    }
                    Ok(None)
                }
            }
            "list" => {
                #[derive(Deserialize, Debug)]
                struct Params {
                    #[serde(alias = "i")]
                    path: Option<String>,
                    #[serde(alias = "m")]
                    masks: Option<ValueOrList<String>>,
                    #[serde(default, alias = "p")]
                    kind: sdkfs::Kind,
                    #[serde(default, alias = "r")]
                    recursive: bool,
                }
                let params = if payload.is_empty() {
                    Params {
                        path: None,
                        masks: None,
                        kind: sdkfs::Kind::Any,
                        recursive: false,
                    }
                } else {
                    unpack(payload)?
                };
                let mut rel_path = "/";
                if let Some(ref p) = params.path {
                    if !p.is_empty() {
                        rel_path = p;
                    }
                };
                let path = self.format_path(Path::new(rel_path))?;
                let masks: Vec<String> = if let Some(m) = params.masks {
                    m.to_vec()
                } else {
                    vec!["*".to_owned()]
                };
                let entries = sdkfs::list(
                    &path,
                    &masks.iter().map(String::as_str).collect::<Vec<&str>>(),
                    params.kind,
                    params.recursive,
                    true,
                )
                .await?;
                let s = path.to_string_lossy();
                let result: Vec<sdkfs::Entry> = entries
                    .into_iter()
                    .filter_map(|r| {
                        let mut p = path.clone();
                        p.push(&r.path);
                        for prot in &self.protected {
                            if p.starts_with(prot) {
                                return None;
                            }
                        }
                        let p = r.path.to_string_lossy();
                        if s.len() + 1 < p.len() {
                            Some(sdkfs::Entry {
                                path: Path::new(&p[s.len() + 1..]).to_owned(),
                                meta: r.meta,
                                kind: r.kind,
                            })
                        } else {
                            None
                        }
                    })
                    .collect();
                Ok(Some(pack(&result)?))
            }
            _ => svc_handle_default_rpc(method, &self.info),
        }
    }
    async fn handle_notification(&self, _event: RpcEvent) {}
    async fn handle_frame(&self, _frame: Frame) {}
}

impl Handlers {
    fn format_path(&self, fpath: &Path) -> EResult<PathBuf> {
        let s = fpath.to_string_lossy();
        if s == "/" {
            Ok(self.runtime_dir.clone())
        } else if fpath.is_absolute() {
            Err(Error::access(
                "the path must be relative to the runtime directory",
            ))
        } else if s.contains("../") {
            Err(Error::access("the path can not contain ../"))
        } else if s.contains("/./") || s.starts_with("./") {
            Err(Error::access("the path can not contain or start with ./"))
        } else {
            let mut path = self.runtime_dir.clone();
            path.extend(fpath);
            for p in &self.protected {
                if path.starts_with(p) {
                    return Err(Error::access("the path is protected"));
                }
            }
            Ok(path)
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    protected: HashSet<PathBuf>,
    #[serde(default)]
    mime_types: Option<String>,
    #[serde(default)]
    terminal: bool,
}

async fn terminal_handler(global_input_rx: async_channel::Receiver<Option<Vec<u8>>>) {
    let mut cmd = Path::new(&eva_common::tools::get_eva_dir()).join("venv/bin/eva");
    if cmd.exists() {
        cmd = Path::new(&eva_common::tools::get_eva_dir()).join("bin/eva");
    } else {
        info!("eva-shell not installed, falling back to the system shell");
        cmd =
            std::env::var("SHELL").map_or_else(|_| Path::new("/bin/sh").to_owned(), PathBuf::from);
    }
    if !cmd.exists() {
        error!("unable to find a working shell, aborting terminal handler");
        return;
    }
    loop {
        let cmd = cmd.clone();
        let (out_tx, out_rx) = async_channel::bounded(TERMINAL_BUFFER_SIZE);
        let (in_tx, in_rx) = async_channel::bounded(TERMINAL_BUFFER_SIZE);
        info!("starting terminal process: {}", cmd.to_string_lossy());
        let dimensions = *TERMINAL_DIMENSIONS.lock();
        let terminal_fut = tokio::spawn(async move {
            terminal::Process::default()
                .run(
                    cmd,
                    &[] as &[&str],
                    BTreeMap::<&str, &str>::new(),
                    eva_common::tools::get_eva_dir(),
                    "xterm-256color",
                    dimensions,
                    out_tx,
                    in_rx,
                )
                .await;
        });
        loop {
            tokio::select! {
                v = global_input_rx.recv() => {
                    if let Ok(input) = v {
                        let need_terminate = input.is_none();
                        if let Err(e) = in_tx.send(input).await {
                            error!("unable to send input to terminal: {}", e);
                            break;
                        }
                        if need_terminate {
                            info!("Terminal process terminated by request");
                            break;
                        }
                    }
                }
                o = out_rx.recv() => {
                    match o {
                      Ok(v) => {
                        match v {
                            terminal::Output::Pid(pid) => {
                                info!("terminal process id: {}", pid);
                                terminal_process().lock().await.pid = Some(pid);
                            }
                            terminal::Output::Stdout(_) | terminal::Output::Stderr(_) => {
                                terminal_process().lock().await.output.push(v);
                            }
                            terminal::Output::Error(e) => {
                                error!("Terminal process error: {}", e);
                                break;
                            }
                            terminal::Output::Terminated(code) => {
                                warn!("Terminal process terminated with code: {:?}", code);
                                terminal_process().lock().await.output.push(v);
                            }
                        }
                    }
                      Err(_) => break,
                    }
                }
            }
        }
        terminal_fut.abort();
        terminal_process().lock().await.terminate().await;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[allow(clippy::too_many_lines)]
#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    TIMEOUT
        .set(initial.timeout())
        .map_err(|_| Error::core("Unable to set TIMEOUT"))?;
    TERMINAL
        .set(TerminalProcess::default().into())
        .map_err(|_| Error::core("Unable to set TERMINAL"))?;
    let eva_dir = Path::new(initial.eva_dir());
    let mut runtime_dir = PathBuf::from(&eva_dir);
    runtime_dir.push("runtime");
    let protected = HashSet::from_iter(
        config
            .protected
            .iter()
            .map(|p| {
                let mut path = runtime_dir.clone();
                path.extend(p);
                path
            })
            .collect::<Vec<PathBuf>>(),
    );
    let mime_types = if let Some(mime_types_path) = config.mime_types {
        let types = tokio::fs::read_to_string(eva_common::tools::format_path(
            initial.eva_dir(),
            Some(&mime_types_path),
            None,
        ))
        .await
        .map_err(|e| {
            error!("unable to read {}", mime_types_path);
            Into::<Error>::into(e)
        })?;
        serde_yaml::from_str(&types).map_err(|e| {
            error!("unable to parse {}", mime_types_path);
            Error::invalid_data(e)
        })?
    } else {
        HashMap::new()
    };
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    info.add_method(
        ServiceMethod::new("sh")
            .required("c")
            .optional("timeout")
            .optional("stdin"),
    );
    info.add_method(ServiceMethod::new("terminal.send_input").required("input"));
    info.add_method(ServiceMethod::new("terminal.take_output"));
    info.add_method(ServiceMethod::new("terminal.restart").optional("dimensions"));
    info.add_method(
        ServiceMethod::new("file.get")
            .required("path")
            .optional("mode")
            .optional("caller"),
    );
    info.add_method(
        ServiceMethod::new("file.put")
            .required("path")
            .optional("content")
            .optional("permissions"),
    );
    info.add_method(ServiceMethod::new("file.unlink").required("path"));
    info.add_method(
        ServiceMethod::new("list")
            .optional("path")
            .optional("masks")
            .optional("kind")
            .optional("recursive"),
    );
    let rpc = initial
        .init_rpc(Handlers {
            info,
            runtime_dir,
            protected,
            mime_types,
        })
        .await?;
    let client = rpc.client().clone();
    svc_init_logs(&initial, client.clone())?;
    svc_start_signal_handlers();
    if config.terminal {
        let (global_tx, global_rx) = async_channel::bounded(200_000);
        TERMINAL_GLOBAL_INPUT_TX
            .set(global_tx)
            .map_err(|_| Error::core("Unable to set TERMINAL_GLOBAL_INPUT_TX"))?;
        tokio::spawn(terminal_handler(global_rx));
    }
    svc_mark_ready(&client).await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    svc_block(&rpc).await;
    svc_mark_terminating(&client).await?;
    terminal_process().lock().await.terminate().await;
    Ok(())
}
