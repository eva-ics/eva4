use bmart_derive::Sorting;
use eva_common::common_payloads::ParamsIdOwned;
use eva_common::prelude::*;
use eva_sdk::prelude::*;
use eva_sdk::types::Fill;
use once_cell::sync::{Lazy, OnceCell};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

const AUTHOR: &str = "Bohemia Automation";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = "SIM Generator";

static RPC: OnceCell<Arc<RpcClient>> = OnceCell::new();
static REG: OnceCell<Registry> = OnceCell::new();

static TIMEOUT: OnceCell<Duration> = OnceCell::new();
static VERBOSE: atomic::AtomicBool = atomic::AtomicBool::new(false);
static SYSTEM_NAME: OnceCell<String> = OnceCell::new();

static SOURCES: Lazy<Mutex<BTreeMap<String, Source>>> = Lazy::new(<_>::default);

const DEFAULT_PLANNING_DURATION: Duration = Duration::from_secs(30);

mod generators;
mod target;

use generators::{GenData, GeneratorSource};
use target::Target;

#[cfg(not(feature = "std-alloc"))]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

err_logger!();

#[inline]
fn is_verbose() -> bool {
    VERBOSE.load(atomic::Ordering::Relaxed)
}

struct Handlers {
    info: ServiceInfo,
}

#[async_trait::async_trait]
impl RpcHandlers for Handlers {
    #[allow(clippy::too_many_lines)]
    async fn handle_call(&self, event: RpcEvent) -> RpcResult {
        macro_rules! reg {
            () => {{
                let reg = REG.get();
                if reg.is_none() {
                    return Err(Error::not_ready("not loaded yet").into());
                }
                reg.unwrap()
            }};
        }
        svc_rpc_need_ready!();
        let method = event.parse_method()?;
        let payload = event.payload();
        #[allow(clippy::match_single_binding)]
        match method {
            "source.plan" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct Payload {
                        source: Source,
                        #[serde(
                            default,
                            deserialize_with = "eva_common::tools::de_opt_float_as_duration"
                        )]
                        duration: Option<Duration>,
                        fill: Option<Fill>,
                    }
                    let p: Payload = unpack(payload)?;
                    let result: Vec<GenData> = tokio::task::spawn_blocking(move || {
                        p.source
                            .plan(p.duration.unwrap_or(DEFAULT_PLANNING_DURATION), p.fill)
                    })
                    .await
                    .map_err(Error::core)??;
                    Ok(Some(pack(&result)?))
                }
            }
            "source.apply" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct Payload {
                        source: Source,
                        t_start: Value,
                        t_end: Option<Value>,
                        #[serde(default)]
                        targets: Vec<OID>,
                    }
                    let p: Payload = unpack(payload)?;
                    let t_start = p.t_start.as_timestamp()?;
                    let t_end = p.t_end.map_or_else(
                        || Ok(eva_common::time::now_ns_float()),
                        |v| v.as_timestamp(),
                    )?;
                    p.source.apply(t_start, t_end, p.targets).await?;
                    Ok(None)
                }
            }
            "source.deploy" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct Sources {
                        generator_sources: Vec<Source>,
                    }
                    let p: Sources = unpack(payload)?;
                    let reg = reg!();
                    let mut sources = SOURCES.lock().await;
                    for mut source in p.generator_sources {
                        sources.remove(&source.name);
                        reg.key_set(&format!("sources/{}", source.name), to_value(&source)?)
                            .await?;
                        let _ = source.start().await;
                        sources.insert(source.name.clone(), source);
                    }
                    Ok(None)
                }
            }
            "source.undeploy" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct Sources {
                        generator_sources: Vec<IdOrSource>,
                    }
                    #[derive(Deserialize)]
                    #[serde(untagged)]
                    enum IdOrSource {
                        Source(Source),
                        Id(String),
                    }
                    impl IdOrSource {
                        fn name(&self) -> &str {
                            match self {
                                IdOrSource::Source(s) => &s.name,
                                IdOrSource::Id(s) => s,
                            }
                        }
                    }
                    let p: Sources = unpack(payload)?;
                    let reg = reg!();
                    let mut sources = SOURCES.lock().await;
                    for source in p.generator_sources {
                        sources.remove(source.name());
                        let _ = reg.key_delete(&format!("sources/{}", source.name())).await;
                    }
                    Ok(None)
                }
            }
            "source.destroy" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsIdOwned = unpack(payload)?;
                    let reg = reg!();
                    let mut sources = SOURCES.lock().await;
                    sources.remove(&p.i);
                    let _ = reg.key_delete(&format!("sources/{}", p.i)).await;
                    Ok(None)
                }
            }
            "source.list" => {
                if payload.is_empty() {
                    #[derive(Serialize, Sorting)]
                    #[sorting(id = "name")]
                    struct SourceInfo<'a> {
                        name: &'a str,
                        kind: SourceKind,
                        active: bool,
                    }
                    let sources = SOURCES.lock().await;
                    let mut result = sources
                        .values()
                        .map(|v| SourceInfo {
                            name: v.name.as_str(),
                            kind: v.kind,
                            active: v.worker.is_some(),
                        })
                        .collect::<Vec<SourceInfo>>();
                    result.sort();
                    Ok(Some(pack(&result)?))
                } else {
                    Err(RpcError::params(None))
                }
            }
            "source.get_config" => {
                if payload.is_empty() {
                    Err(RpcError::params(None))
                } else {
                    let p: ParamsIdOwned = unpack(payload)?;
                    let sources = SOURCES.lock().await;
                    let s = sources
                        .get(&p.i)
                        .ok_or_else(|| Error::not_found("no such source"))?;
                    Ok(Some(pack(&s)?))
                }
            }
            _ => svc_handle_default_rpc(method, &self.info),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    verbose: bool,
}

#[derive(Serialize, Deserialize, Copy, Clone)]
#[serde(rename_all = "snake_case")]
enum SourceKind {
    Random,
    RandomFloat,
    Counter,
    Wave,
    UdpFloat,
}

impl SourceKind {
    fn to_generator(self) -> Box<dyn GeneratorSource> {
        match self {
            SourceKind::Random => Box::new(generators::random::GenSource {}),
            SourceKind::RandomFloat => Box::new(generators::random_float::GenSource {}),
            SourceKind::Counter => Box::new(generators::counter::GenSource {}),
            SourceKind::Wave => Box::new(generators::wave::GenSource {}),
            SourceKind::UdpFloat => Box::new(generators::udp_float::GenSource {}),
        }
    }
}

fn prepare_sampling(sampling: Option<u32>) -> EResult<u32> {
    if let Some(s) = sampling {
        if s == 0 {
            Err(Error::invalid_params("sampling can not be zero"))
        } else {
            Ok(s)
        }
    } else {
        Ok(generators::DEFAULT_SAMPLING)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    name: String,
    kind: SourceKind,
    #[serde(default)]
    params: Value,
    sampling: Option<u32>,
    #[serde(default)]
    targets: Arc<Vec<Target>>,
    #[serde(skip)]
    worker: Option<JoinHandle<()>>,
}

impl Source {
    async fn start(&mut self) -> EResult<()> {
        let gen = self.kind.to_generator();
        match gen
            .start(
                &self.name,
                self.params.clone(),
                prepare_sampling(self.sampling)?,
                self.targets.clone(),
            )
            .await
        {
            Ok(fut) => {
                self.worker.replace(fut);
                Ok(())
            }
            Err(e) => {
                error!("source {} start error: {}", self.name, e);
                Err(e)
            }
        }
    }
    fn plan(&self, duration: Duration, fill: Option<Fill>) -> EResult<Vec<GenData>> {
        let gen = self.kind.to_generator();
        let data = gen.plan(
            self.params.clone(),
            prepare_sampling(self.sampling)?,
            duration,
        )?;
        if let Some(f) = fill {
            let mut t = 0.0;
            let mut result = Vec::new();
            let period = f.as_secs_f64();
            for chunk in data.chunks(usize::try_from(f.as_secs())?) {
                let mut sum = 0.0;
                for d in chunk {
                    sum += f64::try_from(d.value.clone())?;
                }
                if !chunk.is_empty() {
                    sum /= f64::from(u32::try_from(chunk.len())?);
                }
                result.push(GenData {
                    t,
                    value: Value::F64(sum),
                });
                t += period;
            }
            Ok(result)
        } else {
            Ok(data)
        }
    }
    async fn apply(&self, t_start: f64, t_end: f64, targets: Vec<OID>) -> EResult<()> {
        let gen = self.kind.to_generator();
        gen.apply(
            self.params.clone(),
            prepare_sampling(self.sampling)?,
            t_start,
            t_end,
            targets,
        )
        .await
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        if let Some(fut) = self.worker.take() {
            fut.abort();
        }
    }
}

#[svc_main]
async fn main(mut initial: Initial) -> EResult<()> {
    let timeout = initial.timeout();
    TIMEOUT
        .set(timeout)
        .map_err(|_| Error::core("Unable to set TIMEOUT"))?;
    SYSTEM_NAME
        .set(initial.system_name().to_owned())
        .map_err(|_| Error::core("Unable to set SYSTEM_NAME"))?;
    let config: Config = Config::deserialize(
        initial
            .take_config()
            .ok_or_else(|| Error::invalid_data("config not specified"))?,
    )?;
    VERBOSE.store(config.verbose, atomic::Ordering::Relaxed);
    let mut info = ServiceInfo::new(AUTHOR, VERSION, DESCRIPTION);
    info.add_method(ServiceMethod::new("source.list"));
    info.add_method(ServiceMethod::new("source.deploy").required("sources"));
    info.add_method(ServiceMethod::new("source.undeploy").required("sources"));
    info.add_method(ServiceMethod::new("source.get_config").required("i"));
    info.add_method(ServiceMethod::new("source.destroy").required("i"));
    info.add_method(
        ServiceMethod::new("source.plan")
            .required("source")
            .optional("duration"),
    );
    info.add_method(
        ServiceMethod::new("source.apply")
            .required("source")
            .required("t_start")
            .optional("t_end")
            .optional("targets"),
    );
    let rpc = initial.init_rpc(Handlers { info }).await?;
    initial.drop_privileges()?;
    let client = rpc.client().clone();
    RPC.set(rpc.clone())
        .map_err(|_| Error::core("Unable to set RPC"))?;
    svc_init_logs(&initial, client.clone())?;
    let registry = initial.init_registry(&rpc);
    let reg_sources = registry.key_get_recursive("sources").await?;
    {
        let mut sources = SOURCES.lock().await;
        for (k, v) in reg_sources {
            debug!("Source loaded: {}", k);
            let source = Source::deserialize(v)?;
            sources.insert(source.name.clone(), source);
        }
    }
    REG.set(registry)
        .map_err(|_| Error::core("unable to set registry object"))?;
    svc_start_signal_handlers();
    tokio::spawn(async move {
        if svc_wait_core(RPC.get().unwrap(), timeout, true)
            .await
            .is_ok()
        {
            for source in SOURCES.lock().await.values_mut() {
                let _ = source.start().await;
            }
        }
    });
    svc_mark_ready(&client).await?;
    info!("{} started ({})", DESCRIPTION, initial.id());
    svc_block(&rpc).await;
    svc_mark_terminating(&client).await?;
    Ok(())
}
