use clap::Parser;
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use log::{log, Level as LL};
use once_cell::sync::{Lazy, OnceCell};
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::ffi::CString;
use std::path::Path;
use std::ptr;
use std::sync::{mpsc, Arc};
use std::time::Duration;

const ABI_VERSION: u16 = 1;

err_logger!();

macro_rules! addr {
    ($v: expr) => {
        ptr::addr_of!($v) as *mut i32
    };
}

macro_rules! addr_mut {
    ($v: expr) => {
        ptr::addr_of_mut!($v) as *mut i32
    };
}

static SVC_FFI_LIB: OnceCell<libloading::Library> = OnceCell::new();
static SVC_FFI_GET_RESULT: OnceCell<FfiGetResult> = OnceCell::new();
static SVC_FFI_ON_FRAME: OnceCell<FfiOnFrame> = OnceCell::new();
static SVC_FFI_ON_RPC_CALL: OnceCell<FfiOnRpcCall> = OnceCell::new();

static RPC: OnceCell<Arc<RpcClient>> = OnceCell::new();
static TIMEOUT: OnceCell<Duration> = OnceCell::new();
static BUS_TX: OnceCell<async_channel::Sender<BusTask>> = OnceCell::new();

#[derive(Parser, Debug)]
struct Args {
    library_path: String,
    #[clap(long, default_value = "8192")]
    command_queue_size: usize,
    #[clap(long, help = "blocking frame processing (one-by-one)")]
    blocking_fp: bool,
}

#[allow(clippy::upper_case_acronyms)]
#[repr(i16)]
enum SvcOp {
    IsActive = 1,
    SubscribeTopic = 10,
    UnsubscribeTopic = 11,
    PublishTopic = 12,
    RpcCall = 20,
    GetRpcResult = 29,
    LogTrace = 100,
    LogDebug = 110,
    LogInfo = 120,
    LogWarn = 130,
    LogError = 140,
    Terminate = -99,
    POC = -100,
}

macro_rules! check_ptr {
    ($ptr: expr ) => {
        if (unsafe { $ptr.as_ref() }.is_none()) {
            return Err(Error::new0(ErrorKind::InvalidParameter));
        }
    };
}

#[allow(clippy::ptr_as_ptr, clippy::transmute_ptr_to_ref)]
unsafe fn str_from_ebuf_raw<'a>(ebuffer: *mut i32) -> EResult<&'a str> {
    check_ptr!(ebuffer);
    let buf_fb: &EBuffer = std::mem::transmute(ebuffer as *mut u8);
    let mut buf: &[u8] = std::slice::from_raw_parts(buf_fb.data as *const u8, buf_fb.len);
    if buf.len() > 0 && buf[buf.len() - 1] == 0 {
        buf = &buf[0..buf.len() - 1];
    }
    std::str::from_utf8(buf).map_err(Into::into)
}

#[allow(clippy::ptr_as_ptr, clippy::transmute_ptr_to_ref)]
unsafe fn str_vec_from_ebuf_raw<'a>(ebuffer: *mut i32) -> EResult<Vec<&'a str>> {
    check_ptr!(ebuffer);
    let buf_fb: &EBuffer = std::mem::transmute(ebuffer as *mut u8);
    let mut buf: &[u8] = std::slice::from_raw_parts(buf_fb.data as *const u8, buf_fb.len);
    let mut result = Vec::new();
    while let Some(pos) = buf.iter().position(|s| *s == 0u8) {
        let s = std::str::from_utf8(&buf[..pos])?;
        if !s.is_empty() {
            result.push(s);
            buf = &buf[pos + 1..];
        }
    }
    if !buf.is_empty() {
        result.push(std::str::from_utf8(buf)?);
    }
    Ok(result)
}

#[allow(clippy::ptr_as_ptr, clippy::transmute_ptr_to_ref)]
unsafe fn str_and_slice_from_ebuf_raw<'a>(ebuffer: *mut i32) -> EResult<(&'a str, &'a [u8])> {
    check_ptr!(ebuffer);
    let buf_fb: &EBuffer = std::mem::transmute(ebuffer as *mut u8);
    let buf: &[u8] = std::slice::from_raw_parts(buf_fb.data as *const u8, buf_fb.len);
    let pos = buf.iter().position(|s| *s == 0u8).unwrap_or(buf.len());
    let s = std::str::from_utf8(&buf[..pos])?;
    let b = if pos < buf.len() {
        &buf[pos + 1..]
    } else {
        &[]
    };
    Ok((s, b))
}

thread_local! {
    pub static RPC_RESULT: RefCell<Option<RpcEvent>> = <_>::default();
}

#[allow(
    clippy::ptr_as_ptr,
    clippy::transmute_ptr_to_ref,
    clippy::too_many_lines
)]
unsafe fn svc_op(op_code: i16, payload: *mut i32) -> i32 {
    macro_rules! log_message {
        ($lvl: expr) => {
            return match str_from_ebuf_raw(payload) {
                Ok(s) => {
                    log!($lvl, "{}", s);
                    0
                }
                Err(e) => e.code().into(),
            }
        };
    }
    // functions with no bus (logging is muted until connected)
    match op_code {
        x if x == SvcOp::IsActive as i16 => return svc_is_active().into(),
        x if x == SvcOp::LogTrace as i16 => log_message!(LL::Trace),
        x if x == SvcOp::LogDebug as i16 => log_message!(LL::Debug),
        x if x == SvcOp::LogInfo as i16 => log_message!(LL::Info),
        x if x == SvcOp::LogWarn as i16 => log_message!(LL::Warn),
        x if x == SvcOp::LogError as i16 => log_message!(LL::Error),
        x if x == SvcOp::POC as i16 => {
            error!("critical error, terminating");
            eva_sdk::service::poc();
            return 0;
        }
        _ => {}
    };
    // functions which require active bus connection
    let Some(bus_tx) = BUS_TX.get() else {
        return eva_common::ERR_CODE_NOT_READY.into();
    };
    macro_rules! bus_task {
        ($task: expr) => {
            if bus_tx.try_send($task).is_ok() {
                0
            } else {
                warn!("command queue full, bus task/event ignored");
                eva_common::ERR_CODE_BUSY.into()
            }
        };
    }
    match op_code {
        x if x == SvcOp::SubscribeTopic as i16 => match str_vec_from_ebuf_raw(payload) {
            Ok(topics) => {
                let task = BusTask::Subscribe(topics.into_iter().map(ToOwned::to_owned).collect());
                bus_task!(task)
            }
            Err(e) => e.code().into(),
        },
        x if x == SvcOp::UnsubscribeTopic as i16 => match str_vec_from_ebuf_raw(payload) {
            Ok(topics) => {
                let task =
                    BusTask::Unsubscribe(topics.into_iter().map(ToOwned::to_owned).collect());
                bus_task!(task)
            }
            Err(e) => e.code().into(),
        },
        x if x == SvcOp::PublishTopic as i16 => match str_and_slice_from_ebuf_raw(payload) {
            Ok((topic, payload)) => {
                let task = BusTask::Publish(topic.to_owned(), payload.to_vec());
                bus_task!(task)
            }
            Err(e) => e.code().into(),
        },
        x if x == SvcOp::RpcCall as i16 => match str_and_slice_from_ebuf_raw(payload) {
            Ok((target, buf)) => {
                let pos = buf.iter().position(|s| *s == 0u8).unwrap_or(buf.len());
                let Ok(method) = std::str::from_utf8(&buf[..pos]) else {
                    return eva_common::ERR_CODE_INVALID_PARAMS.into();
                };
                let params = if pos < buf.len() {
                    &buf[pos + 1..]
                } else {
                    &[]
                };
                let (tx, rx) = mpsc::channel();
                let task =
                    BusTask::RpcCall(target.to_owned(), method.to_owned(), params.to_vec(), tx);
                if bus_tx.try_send(task).is_err() {
                    warn!("command queue full, bus rpc call ignored");
                    return eva_common::ERR_CODE_BUSY.into();
                }
                let Ok(r) = rx.recv() else {
                    return eva_common::ERR_CODE_CORE_ERROR.into();
                };
                match r {
                    Ok(ev) => {
                        let event_payload = ev.payload();
                        if event_payload.is_empty() {
                            0
                        } else {
                            let Ok(size) = i32::try_from(event_payload.len()) else {
                                return eva_common::ERR_CODE_CORE_ERROR.into();
                            };
                            RPC_RESULT.with(|c| {
                                c.borrow_mut().replace(ev);
                            });
                            size
                        }
                    }
                    Err(e) => e.code().into(),
                }
            }
            Err(e) => e.code().into(),
        },
        x if x == SvcOp::GetRpcResult as i16 => RPC_RESULT.with(|c| {
            if unsafe { payload.as_ref() }.is_none() {
                return eva_common::ERR_CODE_INVALID_PARAMS.into();
            }
            if let Some(ev) = c.borrow().as_ref() {
                let event_payload = ev.payload();
                let buf_fb: &mut EBuffer = std::mem::transmute(payload as *mut u8);
                buf_fb.len = event_payload.len();
                buf_fb.data = addr!(*event_payload);
                buf_fb.max = event_payload.len();
                0
            } else {
                eva_common::ERR_CODE_NOT_FOUND.into()
            }
        }),
        x if x == SvcOp::Terminate as i16 => {
            svc_terminate();
            0
        }
        _ => eva_common::ERR_CODE_METHOD_NOT_FOUND.into(),
    }
}

macro_rules! svc_ffi_lib {
    () => {
        SVC_FFI_LIB.get().unwrap()
    };
}

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

struct Handlers {
    info: ServiceInfo,
}

#[repr(C)]
struct EFrame<'a> {
    kind: u8,
    primary_sender: *mut i32,
    topic: *mut i32,
    payload_size: usize,
    payload: *mut i32,
    _src: &'a Frame,
    _primary_sender: CString,
    _topic: CString,
}

impl<'a> From<&'a Frame> for EFrame<'a> {
    fn from(frame: &'a Frame) -> Self {
        let sender_s = CString::new(frame.primary_sender()).unwrap_or_default();
        let topic_s = CString::new(frame.topic().unwrap_or_default()).unwrap_or_default();
        let payload = frame.payload();
        Self {
            kind: frame.kind() as u8,
            primary_sender: addr!(*sender_s.as_c_str()),
            topic: addr!(*topic_s.as_c_str()),
            payload_size: payload.len(),
            payload: addr!(*payload),
            _src: frame,
            _primary_sender: sender_s,
            _topic: topic_s,
        }
    }
}

#[repr(C)]
struct ERpcEvent<'a> {
    primary_sender: *mut i32,
    method_size: usize,
    method: *mut i32,
    payload_size: usize,
    payload: *mut i32,
    _src: &'a RpcEvent,
    _primary_sender: CString,
}

impl<'a> From<&'a RpcEvent> for ERpcEvent<'a> {
    fn from(ev: &'a RpcEvent) -> Self {
        let sender_s = CString::new(ev.primary_sender()).unwrap_or_default();
        let payload = ev.payload();
        let method = ev.method();
        Self {
            primary_sender: addr!(*sender_s.as_c_str()),
            method_size: method.len(),
            method: addr!(*method),
            payload_size: payload.len(),
            payload: addr!(*payload),
            _src: ev,
            _primary_sender: sender_s,
        }
    }
}

#[repr(C)]
struct EBuffer<'a> {
    len: usize,
    data: *mut i32,
    max: usize,
    _src: &'a [u8],
}

type FfiSetOpFunc<'a> =
    libloading::Symbol<'a, unsafe extern "C" fn(version: u16, op_func: *mut i32) -> i16>;
type FfiInitFunc<'a> = libloading::Symbol<'a, unsafe extern "C" fn(initial: *mut i32) -> i32>;
type FfiEmptyFunc<'a> = libloading::Symbol<'a, unsafe extern "C" fn() -> i16>;
type FfiGetResult<'a> = libloading::Symbol<'a, unsafe extern "C" fn(buf: *mut i32) -> i16>;
type FfiOnFrame<'a> = libloading::Symbol<'a, unsafe extern "C" fn(frame: *mut i32)>;
type FfiOnRpcCall<'a> = libloading::Symbol<'a, unsafe extern "C" fn(ev: *mut i32) -> i32>;

const DEFAULT_METHODS_STRS: &[&[u8]] = &[b"test", b"info", b"stop"];

static DEFAULT_METHODS: Lazy<BTreeSet<&'static [u8]>> = Lazy::new(|| {
    DEFAULT_METHODS_STRS
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
});

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        if DEFAULT_METHODS.contains(&event.method()) {
            svc_handle_default_rpc(event.parse_method()?, &self.info)
        } else if SVC_FFI_ON_RPC_CALL.get().is_some() {
            let result = tokio::task::spawn_blocking(move || {
                let ev: ERpcEvent = (&event).into();
                let f = SVC_FFI_ON_RPC_CALL.get().unwrap();
                unsafe { f(addr!(ev)).into_result_vec() }
            })
            .await
            .map_err(|e| Error::core(format!("unable to process rpc call handler: {}", e)))??;
            Ok(result)
        } else {
            Err(RpcError::method(None))
        }
    }
    async fn handle_frame(&self, frame: Frame) {
        if SVC_FFI_ON_FRAME.get().is_some() {
            tokio::task::spawn_blocking(move || {
                let e_frame = EFrame::from(&frame);
                let f = SVC_FFI_ON_FRAME.get().unwrap();
                unsafe { f(addr!(e_frame)) };
            });
        }
    }
}

trait IntoResult {
    fn into_result(self) -> EResult<()>;
}

trait IntoResultVec {
    fn into_result_vec(self) -> EResult<Option<Vec<u8>>>;
}

impl IntoResult for i16 {
    fn into_result(self) -> EResult<()> {
        let code = self;
        if code == 0 {
            Ok(())
        } else {
            Err(Error::new0(ErrorKind::from(code)))
        }
    }
}

impl IntoResultVec for i32 {
    fn into_result_vec(self) -> EResult<Option<Vec<u8>>> {
        let code = self;
        match code.cmp(&0) {
            Ordering::Greater => {
                let mut buf = vec![0u8; usize::try_from(code).unwrap()];
                let mut buf_fb = buf.as_ffi_buf_mut()?;
                unsafe {
                    #[allow(clippy::ptr_as_ptr)]
                    SVC_FFI_GET_RESULT.get().unwrap()(addr_mut!(buf_fb)).into_result()?;
                }
                let len = buf_fb.len;
                buf.truncate(len);
                Ok(Some(buf))
            }
            Ordering::Equal => Ok(None),
            Ordering::Less => Err(Error::new0(ErrorKind::from(
                i16::try_from(code).unwrap_or(eva_common::ERR_CODE_OTHER),
            ))),
        }
    }
}

trait FfiBuf {
    fn as_ffi_buf(&self) -> EResult<EBuffer>;
    fn as_ffi_buf_mut(&mut self) -> EResult<EBuffer>;
}

#[allow(clippy::cast_ptr_alignment, clippy::ptr_as_ptr)]
impl FfiBuf for Vec<u8> {
    fn as_ffi_buf(&self) -> EResult<EBuffer> {
        Ok(EBuffer {
            len: self.len(),
            data: self.as_ptr() as *mut i32,
            max: self.capacity(),
            _src: self,
        })
    }
    fn as_ffi_buf_mut(&mut self) -> EResult<EBuffer> {
        Ok(EBuffer {
            len: self.len(),
            data: self.as_mut_ptr() as *mut i32,
            max: self.capacity(),
            _src: self,
        })
    }
}

enum BusTask {
    Subscribe(Vec<String>),
    Unsubscribe(Vec<String>),
    Publish(String, Vec<u8>),
    RpcCall(
        String,
        String,
        Vec<u8>,
        mpsc::Sender<Result<RpcEvent, Error>>,
    ),
}

static ARGS: OnceCell<Args> = OnceCell::new();

fn main() -> EResult<()> {
    let args = Args::parse();
    ARGS.set(args)
        .map_err(|_| Error::core("unable to set ARGS"))?;
    eva_sdk::service::svc_launch(eva_svc_main)
}

#[allow(clippy::too_many_lines)]
async fn eva_svc_main(mut initial: Initial) -> EResult<()> {
    initial = initial.into_legacy_compat();
    let args = ARGS.get().unwrap();
    let timeout = initial.timeout();
    TIMEOUT
        .set(timeout)
        .map_err(|_| Error::core("unable to set TIMEOUT"))?;
    eva_sdk::service::set_poc(Some(Duration::from_secs(1)));
    macro_rules! call_maybe_ufn {
        ($f: expr) => {
            if let Some(f) = $f {
                unsafe {
                    f().into_result()?;
                }
            }
        };
    }
    let lib_path = if args.library_path.starts_with('/') {
        Path::new(&args.library_path).to_owned()
    } else {
        let mut p = Path::new(initial.eva_dir()).to_owned();
        p.push(&args.library_path);
        p
    };
    let lib = unsafe { libloading::Library::new(lib_path).map_err(Error::failed)? };
    SVC_FFI_LIB
        .set(lib)
        .map_err(|_| Error::core("unable to set SVC_FFI_LIB"))?;
    let ffi_init: FfiInitFunc = unsafe { svc_ffi_lib!().get(b"svc_init").map_err(Error::failed)? };
    let ffi_get_result: FfiGetResult = unsafe {
        svc_ffi_lib!()
            .get(b"eva_svc_get_result")
            .map_err(Error::failed)?
    };
    SVC_FFI_GET_RESULT
        .set(ffi_get_result)
        .map_err(|_| Error::core("unable to set SVC_FFI_GET_RESULT"))?;
    let ffi_eva_set_op_fn: FfiSetOpFunc = unsafe {
        svc_ffi_lib!()
            .get(b"eva_svc_set_op_fn")
            .map_err(Error::failed)?
    };
    unsafe {
        ffi_eva_set_op_fn(ABI_VERSION, svc_op as *const () as *mut i32).into_result()?;
    }
    let ffi_prepare: Option<FfiEmptyFunc> = unsafe { svc_ffi_lib!().get(b"svc_prepare").ok() };
    let ffi_launch: Option<FfiEmptyFunc> = unsafe { svc_ffi_lib!().get(b"svc_launch").ok() };
    let ffi_terminate: Option<FfiEmptyFunc> = unsafe { svc_ffi_lib!().get(b"svc_terminate").ok() };
    if let Ok(ffi_on_frame) = unsafe { svc_ffi_lib!().get(b"svc_on_frame") } {
        SVC_FFI_ON_FRAME
            .set(ffi_on_frame)
            .map_err(|_| Error::core("unable to set SVC_FFI_ON_FRAME"))?;
    }
    if let Ok(ffi_on_rpc_call) = unsafe { svc_ffi_lib!().get(b"svc_on_rpc_call") } {
        SVC_FFI_ON_RPC_CALL
            .set(ffi_on_rpc_call)
            .map_err(|_| Error::core("unable to set SVC_FFI_ON_RPC_CALL"))?;
    }
    let initial_buf = pack(&initial)?;
    let initial_fb = initial_buf.as_ffi_buf()?;
    let buf = unsafe { ffi_init(addr!(initial_fb)).into_result_vec()? };
    let info: ServiceInfo = buf.map_or_else(
        || Ok(ServiceInfo::new("", "", "FFI launcher dummy")),
        |b| unpack(&b),
    )?;
    macro_rules! set_rpc {
        ($rpc: expr) => {
            RPC.set($rpc)
                .map_err(|_| Error::core("unable to set RPC"))?;
        };
    }
    let description = info.description.clone();
    let (rpc, client, client_bw) = if args.blocking_fp {
        let (rpc, secondary) = initial
            .init_rpc_blocking_with_secondary(Handlers { info })
            .await?;
        let client_secondary = secondary.client().clone();
        let client = rpc.client().clone();
        let client_bw = client_secondary.clone();
        set_rpc!(secondary);
        (rpc, client, client_bw) // bus worker client uses an additional RPC channel
    } else {
        let rpc = initial.init_rpc(Handlers { info }).await?;
        let client = rpc.client().clone();
        let client_bw = client.clone();
        set_rpc!(rpc.clone());
        (rpc, client, client_bw) // bus worker client is the same as the primary one
    };
    initial.drop_privileges()?;
    svc_init_logs(&initial, client.clone())?;
    let (buscc_tx, rx) = async_channel::bounded::<BusTask>(args.command_queue_size);
    BUS_TX
        .set(buscc_tx.clone())
        .map_err(|_| Error::core("unable to set BUS_TX"))?;
    tokio::spawn(async move {
        while let Ok(task) = rx.recv().await {
            let mut c = client_bw.lock().await;
            match task {
                BusTask::Subscribe(topics) => {
                    c.subscribe_bulk(
                        &topics.iter().map(String::as_str).collect::<Vec<&str>>(),
                        QoS::No,
                    )
                    .await
                    .log_ef();
                }
                BusTask::Unsubscribe(topics) => {
                    c.unsubscribe_bulk(
                        &topics.iter().map(String::as_str).collect::<Vec<&str>>(),
                        QoS::No,
                    )
                    .await
                    .log_ef();
                }
                BusTask::Publish(topic, payload) => {
                    c.publish(&topic, payload.into(), QoS::No).await.log_ef();
                }
                BusTask::RpcCall(target, method, params, tx) => {
                    tokio::spawn(async move {
                        let res = safe_rpc_call(
                            RPC.get().unwrap(),
                            &target,
                            &method,
                            params.into(),
                            QoS::No,
                            *TIMEOUT.get().unwrap(),
                        )
                        .await;
                        let _ = tx.send(res);
                    });
                }
            }
        }
    });
    svc_start_signal_handlers();
    call_maybe_ufn!(ffi_prepare);
    svc_mark_ready(&client).await?;
    info!("{} started ({})", description, initial.id());
    call_maybe_ufn!(ffi_launch);
    svc_block(&rpc).await;
    svc_mark_terminating(&client).await?;
    call_maybe_ufn!(ffi_terminate);
    while !buscc_tx.is_empty() {
        tokio::time::sleep(eva_common::SLEEP_STEP).await;
    }
    Ok(())
}
