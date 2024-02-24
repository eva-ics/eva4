use crate::safe_http::{self, IsUrl};
use clap::Parser;
use colored::Colorize;
use eva_client::{EvaClient, EvaCloudClient, NodeMap, SystemInfo};
use eva_common::common_payloads::{NodeData, ParamsId};
use eva_common::prelude::*;
use log::{debug, error, info, warn};
use openssl::sha::Sha256;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::fs;
use tokio::io::AsyncReadExt;

pub const DEPLOY_VERSION: u16 = 4;

const SVC_WAIT_INITIAL: Duration = Duration::from_secs(2);
const SVC_WAIT_SLEEP_STEP: Duration = Duration::from_secs(5);

#[derive(Parser, Clone, Debug)]
pub struct Options {
    #[clap()]
    deployment_file: String,
    #[clap(short = 't', long = "timeout", default_value = "10")]
    pub timeout: f64,
    #[clap(short = 'C', long = "connection-path")]
    connection_path: Option<String>,
    #[clap(
        short = 'c',
        long = "config-var",
        help = "deployment config var name=value, can be specified multiple times"
    )]
    vars: Vec<String>,
    #[clap(long = "config", help = "load configuration variables from YAML file")]
    config: Option<String>,
    #[clap(long = "test", help = "test the configuration and exit")]
    test: bool,
}

async fn execute_extra(
    client: &EvaCloudClient,
    node: &str,
    extra: Vec<ExtraMethod>,
    timeout: Duration,
) -> EResult<()> {
    for cmd in extra {
        match cmd {
            ExtraMethod::Bus(b) => {
                let mut sp = b.method.splitn(2, "::");
                let m = sp.next().unwrap();
                let (target, method) = if let Some(x) = sp.next() {
                    (m, x)
                } else {
                    ("eva.core", m)
                };
                info!("{}/{}::{}", node, target, method);
                let params = if let ValueOptionOwned::Value(v) = b.params {
                    Some(v)
                } else {
                    None
                };
                let r: EResult<Value> = client.call(node, target, method, params).await;
                if !b._pass {
                    r?;
                }
            }
            ExtraMethod::Func(f) => match f.function {
                LocalFunction::Sleep => {
                    let t: Duration = if let Some(v) = f.args.get(0) {
                        let ft: f64 = v.try_into()?;
                        Duration::from_secs_f64(ft)
                    } else {
                        Duration::from_secs(0)
                    };
                    info!("sleep({:?})", t);
                    tokio::time::sleep(t).await;
                }
                LocalFunction::System => {
                    if let Some(args) = f.args.get(0) {
                        let a = args.to_string();
                        info!("system({})", a);
                        let result = bmart::process::command(
                            "sh",
                            ["-c", &a],
                            timeout,
                            bmart::process::Options::default(),
                        )
                        .await;
                        if !f._pass {
                            let res = result?;
                            if res.ok() {
                                for l in res.out {
                                    info!("{}", l);
                                }
                            } else {
                                for l in res.err {
                                    error!("{}", l);
                                }
                                return Err(Error::failed(format!(
                                    "command failed with exit code: {}",
                                    res.code.unwrap_or_default()
                                )));
                            }
                        }
                    }
                }
            },
        }
    }
    Ok(())
}

#[allow(clippy::module_name_repetitions)]
#[allow(clippy::too_many_lines)]
pub async fn deploy_undeploy(opts: Options, deploy: bool) -> EResult<()> {
    let connection_path = opts
        .connection_path
        .unwrap_or_else(|| crate::DEFAULT_CONNECTION_PATH.to_string());
    let timeout = Duration::from_secs_f64(opts.timeout);
    let mut http_client = safe_http::Client::new(timeout * 10);
    http_client.allow_redirects();
    let mut tpl_context = tera::Context::new();
    if let Some(ref config_file) = opts.config {
        info!("reading variable configuration file {}", config_file);
        let mut val: Value =
            serde_yaml::from_slice(&fs::read(config_file).await?).map_err(Error::invalid_data)?;
        info!("extending variable configuration file");
        let config_fpath = Path::new(config_file);
        let config_base_path = config_fpath.parent().unwrap().to_path_buf();
        val = val.extend(timeout, &config_base_path).await?;
        let vars: BTreeMap<String, Value> = BTreeMap::deserialize(val)?;
        for (k, v) in vars {
            debug!("{} = {}", k, v);
            tpl_context.insert(k, &v);
        }
    }
    if !opts.vars.is_empty() {
        info!("parsing configuration variables");
        for var in opts.vars {
            let mut sp = var.splitn(2, '=');
            let name = sp.next().unwrap();
            let value = sp.next().ok_or_else(|| {
                Error::invalid_params("invalid variable argument, must be name=value")
            })?;
            let val: Value = value.parse().unwrap();
            debug!("{} = {}", name, val);
            tpl_context.insert(name, &val);
        }
    }
    info!("parsing environment variables");
    for (var_name, value) in std::env::vars() {
        if let Some(name) = var_name.strip_prefix("ECD_") {
            let val: Value = value.parse().unwrap();
            debug!("{} = {}", name, val);
            tpl_context.insert(name, &val);
        }
    }
    let expanded_path = expanduser::expanduser(&opts.deployment_file)?;
    let fpath = Path::new(&expanded_path);
    let s = if opts.deployment_file == "-" {
        ecm::tools::read_stdin().await
    } else {
        info!("reading the deployment file {}", opts.deployment_file);
        if fpath.is_url() {
            http_client.fetch_animated(&fpath.to_string_lossy()).await?
        } else {
            fs::read(fpath).await?
        }
    };
    let tpl = std::str::from_utf8(&s)?;
    let base_path = fpath.parent().unwrap().to_path_buf();
    info!("parsing template tags");
    let t = tera::Tera::one_off(tpl, &tpl_context, true).map_err(Error::invalid_data)?;
    info!("parsing YAML structure");
    let mut data: Value = serde_yaml::from_str(&t).map_err(Error::invalid_data)?;
    info!("extending");
    data = data.extend(timeout, &base_path).await?;
    info!("parsing the payload");
    let mut payload: DeploymentPayload = DeploymentPayload::deserialize(data)?;
    if payload.version != DEPLOY_VERSION {
        return Err(Error::unsupported(format!(
            "unsupported deployment file version: {}",
            payload.version
        )));
    }
    info!("analyzing upload");
    for node in &mut payload.content {
        for file in &mut node.upload {
            file.extract = file.extract.normalize(if let Some(ref url) = file.url {
                Some(Path::new(url))
            } else {
                file.src.as_deref()
            })?;
            if file.svc.is_empty() {
                file.svc = node.params.filemgr_svc.clone();
            }
            if let Some(ref mut src) = file.src {
                if !src.is_absolute() && !src.is_url() {
                    let mut path = base_path.clone();
                    path.push(&src);
                    *src = path;
                }
                if !src.is_url() && (!src.exists() || src.is_dir()) {
                    return Err(Error::invalid_params(format!(
                        "{} does not exist or is not a file",
                        src.to_string_lossy()
                    )));
                }
            } else if file.text.is_none() && file.url.is_none() {
                return Err(Error::invalid_params(
                    "either src file or text/url content must be defined",
                ));
            }
            if file.target.to_string_lossy().ends_with('/') && file.extract.is_no() {
                file.target.push(if let Some(ref url) = file.url {
                    Path::new(url)
                        .file_name()
                        .ok_or_else(|| Error::invalid_params("file name missing in URL!"))?
                } else {
                    file.src
                        .as_ref()
                        .ok_or_else(|| {
                            Error::invalid_params("source as a content but no target file name set")
                        })?
                        .file_name()
                        .ok_or_else(|| Error::invalid_params("file name missing in source!"))?
                });
            }
        }
    }
    if opts.test {
        println!("{}", "-----".dimmed());
        println!(
            "{}",
            serde_yaml::to_string(&payload).map_err(Error::invalid_data)?
        );
        println!("{}", "-----".dimmed());
    }
    info!("connecting to {}", connection_path);
    let bus_client = EvaClient::connect(
        &connection_path,
        ecm::BUS_CLIENT_NAME,
        eva_client::Config::new().timeout(timeout),
    )
    .await?;
    let mut info = bus_client.get_system_info().await?;
    if !info.active {
        let wait_until = Instant::now() + timeout;
        ttycarousel::tokio1::spawn0("waiting for system to become active").await;
        loop {
            tokio::time::sleep(eva_common::SLEEP_STEP).await;
            info = bus_client.get_system_info().await?;
            if info.active {
                break;
            }
            if wait_until <= Instant::now() {
                ttycarousel::tokio1::stop_clear().await;
                return Err(Error::failed("system is not active"));
            }
        }
        ttycarousel::tokio1::stop_clear().await;
    }
    let system_name = info.system_name;
    info!("local node: {}", system_name);
    let mut node_map = NodeMap::new();
    for node in &payload.content {
        if node.node != ".local" {
            let data: NodeData = match bus_client
                .call(
                    "eva.core",
                    "node.get",
                    Some(to_value(ParamsId { i: &node.node })?),
                )
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    error!("unable to fetch info for the node {}", node.node);
                    return Err(e);
                }
            };
            if !data.online() {
                return Err(Error::failed(format!("node {} is offline", node.node)));
            }
            if let Some(svc) = data.svc() {
                info!("node {} replication svc: {}", node.node, svc);
                node_map.insert(node.node.clone(), svc.to_owned());
            }
        }
    }
    let client = EvaCloudClient::new(&system_name, bus_client, node_map);
    macro_rules! test_mp {
        ($method: expr, $x: expr) => {{
            let mut params: BTreeMap<&str, Vec<()>> = BTreeMap::new();
            params.insert($x, Vec::new());
            Some(($method, to_value(params)?))
        }};
    }
    for node in &payload.content {
        let mut svcs_to_test: BTreeMap<&str, Option<(&str, Value)>> = BTreeMap::new();
        info!("testing node {}", node.node);
        let _r: SystemInfo = client.call(&node.node, "eva.core", "test", None).await?;
        for f in &node.upload {
            svcs_to_test.insert(&f.svc, None);
        }
        if node.acls.is_some() {
            svcs_to_test.insert(&node.params.acl_svc, test_mp!("acl.deploy", "acls"));
        }
        if node.keys.is_some() {
            svcs_to_test.insert(&node.params.key_svc, test_mp!("key.deploy", "keys"));
        }
        if node.users.is_some() {
            svcs_to_test.insert(&node.params.user_svc, test_mp!("user.deploy", "users"));
        }
        if node.generator_sources.is_some() {
            svcs_to_test.insert(
                &node.params.generator_svc,
                test_mp!("source.deploy", "generator_sources"),
            );
        }
        for (svc, p) in svcs_to_test {
            if node.svcs.iter().any(|s| s.id == svc) {
                info!("skipping service test {}/{}", node.node, svc);
                continue;
            }
            info!("testing service {}/{}", node.node, svc);
            if let Some((method, params)) = p {
                client.call(&node.node, svc, method, Some(params)).await?;
            } else {
                client.call(&node.node, svc, "test", None).await?;
            }
        }
    }
    if opts.test {
        info!("tests completed successfully");
        return Ok(());
    }
    warn!(
        "starting {}",
        if deploy { "deployment" } else { "undeployment" }
    );
    macro_rules! dp {
        ($x: expr, $d: expr) => {{
            let mut payload: BTreeMap<&str, Value> = BTreeMap::new();
            payload.insert($x, $d);
            Some(to_value(payload)?)
        }};
    }
    if deploy {
        for node in payload.content {
            info!("deploying node {}", node.node);
            if !node.extra.deploy.before.is_empty() {
                info!("executing before deploy tasks");
                execute_extra(&client, &node.node, node.extra.deploy.before, timeout).await?;
            }
            for f in node.upload {
                let mut buf = Vec::new();
                let (content, mut permissions, download) = if let Some(src) = f.src {
                    info!(
                        "uploading file {} => {}:{}",
                        src.to_string_lossy(),
                        node.node,
                        f.target.to_string_lossy()
                    );
                    if src.is_url() {
                        let url = src.to_string_lossy();
                        buf = http_client.fetch_animated(&url).await?;
                        (buf.as_slice(), None, false)
                    } else {
                        let mut file = fs::File::open(src).await?;
                        let permissions = file.metadata().await?.permissions().mode();
                        file.read_to_end(&mut buf).await?;
                        (buf.as_slice(), Some(permissions), false)
                    }
                } else if let Some(ref text) = f.text {
                    info!(
                        "uploading file => {}:{}",
                        node.node,
                        f.target.to_string_lossy()
                    );
                    (text.as_bytes(), None, false)
                } else {
                    let url = f.url.as_ref().unwrap();
                    info!(
                        "requesting fetch {} => {}:{}",
                        url,
                        node.node,
                        f.target.to_string_lossy()
                    );
                    (url.as_bytes(), None, true)
                };
                let hasher = if f.url.is_some() {
                    None
                } else {
                    let mut h = Sha256::new();
                    h.update(content);
                    Some(h)
                };
                if let Some(s_permissions) = f.permissions {
                    permissions.replace(s_permissions);
                }
                let params = ParamsFilePut {
                    path: &f.target.to_string_lossy(),
                    content,
                    permissions,
                    sha256: hasher.map(Sha256::finish),
                    extract: f.extract,
                    download,
                };
                client
                    .call(&node.node, &f.svc, "file.put", Some(to_value(params)?))
                    .await?;
            }
            macro_rules! deploy_resource {
                ($src: expr, $name: expr, $field: expr, $svc: expr, $fn: expr) => {
                    if let ValueOptionOwned::Value(res) = $src {
                        info!("deploying {}", $name);
                        client
                            .call(&node.node, $svc, $fn, dp!($field, to_value(res)?))
                            .await?;
                    }
                };
                ($src: expr, $name: expr, $svc: expr, $fn: expr) => {
                    deploy_resource!($src, $name, $name, $svc, $fn);
                };
            }
            deploy_resource!(node.items, "items", "eva.core", "item.deploy");
            if !node.svcs.is_empty() {
                let mut svcs_wait: HashSet<&str> = HashSet::new();
                info!("deploying services");
                for svc in &node.svcs {
                    svcs_wait.insert(&svc.id);
                }
                client
                    .call(
                        &node.node,
                        "eva.core",
                        "svc.deploy",
                        dp!("svcs", to_value(&node.svcs)?),
                    )
                    .await?;
                info!("waiting services start");
                let op = eva_common::op::Op::new(timeout);
                tokio::time::sleep(SVC_WAIT_INITIAL).await;
                loop {
                    if op.is_timed_out() {
                        for svc in &svcs_wait {
                            error!("service is not started: {}/{}", node.node, svc);
                        }
                        return Err(Error::timeout());
                    }
                    let mut svcs_online: HashSet<&str> = HashSet::new();
                    for svc in &svcs_wait {
                        let st: SvcStatus = client
                            .call(
                                &node.node,
                                "eva.core",
                                "svc.get",
                                Some(to_value(ParamsId { i: svc })?),
                            )
                            .await?;
                        if st.status == "online" && st.pid.is_some() {
                            svcs_online.insert(svc);
                        }
                    }
                    svcs_wait.retain(|v| !svcs_online.contains(v));
                    if svcs_wait.is_empty() {
                        break;
                    }
                    tokio::time::sleep(SVC_WAIT_SLEEP_STEP).await;
                }
            }
            deploy_resource!(node.acls, "acls", &node.params.acl_svc, "acl.deploy");
            deploy_resource!(node.keys, "keys", &node.params.key_svc, "key.deploy");
            deploy_resource!(node.users, "users", &node.params.user_svc, "user.deploy");
            deploy_resource!(
                node.generator_sources,
                "generator_sources",
                &node.params.generator_svc,
                "source.deploy"
            );
            if !node.extra.deploy.after.is_empty() {
                info!("executing after deploy tasks");
                execute_extra(&client, &node.node, node.extra.deploy.after, timeout).await?;
            }
        }
    } else {
        for node in payload.content.into_iter().rev() {
            info!("undeploying node {}", node.node);
            if !node.extra.undeploy.before.is_empty() {
                info!("executing before undeploy tasks");
                execute_extra(&client, &node.node, node.extra.undeploy.before, timeout).await?;
            }
            macro_rules! undeploy_resource {
                ($src: expr, $name: expr, $field: expr, $svc: expr, $fn: expr) => {
                    if let ValueOptionOwned::Value(res) = $src {
                        info!("undeploying {}", $name);
                        client
                            .call(&node.node, $svc, $fn, dp!($field, to_value(res)?))
                            .await?;
                    }
                };
                ($src: expr, $name: expr, $svc: expr, $fn: expr) => {
                    undeploy_resource!($src, $name, $name, $svc, $fn);
                };
            }
            undeploy_resource!(node.items, "items", "eva.core", "item.undeploy");
            undeploy_resource!(
                node.generator_sources,
                "generator_sources",
                &node.params.generator_svc,
                "source.undeploy"
            );
            undeploy_resource!(node.users, "users", &node.params.user_svc, "user.undeploy");
            undeploy_resource!(node.keys, "keys", &node.params.key_svc, "key.undeploy");
            undeploy_resource!(node.acls, "acls", &node.params.acl_svc, "acl.undeploy");
            if !node.svcs.is_empty() {
                info!("undeploying services");
                client
                    .call(
                        &node.node,
                        "eva.core",
                        "svc.undeploy",
                        dp!("svcs", to_value(&node.svcs)?),
                    )
                    .await?;
            }
            for f in node.upload {
                info!("deleting file {}:{}", node.node, f.target.to_string_lossy());
                let params = ParamsFileUnlink {
                    path: &f.target.to_string_lossy(),
                };
                if let Err(e) = client
                    .call::<()>(&node.node, &f.svc, "file.unlink", Some(to_value(params)?))
                    .await
                {
                    warn!("{}", e);
                }
            }
            if !node.extra.undeploy.after.is_empty() {
                info!("executing after undeploy tasks");
                execute_extra(&client, &node.node, node.extra.undeploy.after, timeout).await?;
            }
        }
    }
    Ok(())
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
struct DeploymentPayload {
    version: u16,
    content: Vec<DeploymentContent>,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
struct DeploymentContent {
    node: String,
    #[serde(default)]
    params: NodeParams,
    #[serde(default)]
    upload: Vec<NodeUpload>,
    #[serde(default)]
    svcs: Vec<DeploymentService>,
    #[serde(default, skip_serializing_if = "ValueOptionOwned::is_none")]
    acls: ValueOptionOwned,
    #[serde(default, skip_serializing_if = "ValueOptionOwned::is_none")]
    keys: ValueOptionOwned,
    #[serde(default, skip_serializing_if = "ValueOptionOwned::is_none")]
    users: ValueOptionOwned,
    #[serde(default, skip_serializing_if = "ValueOptionOwned::is_none")]
    items: ValueOptionOwned,
    #[serde(default, skip_serializing_if = "ValueOptionOwned::is_none")]
    generator_sources: ValueOptionOwned,
    #[serde(default)]
    extra: DeploymentExtra,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
struct DeploymentService {
    id: String,
    params: Value,
}

#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(deny_unknown_fields)]
struct DeploymentExtra {
    #[serde(default)]
    deploy: ExtraBeforeAfter,
    #[serde(default)]
    undeploy: ExtraBeforeAfter,
}

#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(deny_unknown_fields)]
struct ExtraBeforeAfter {
    #[serde(default)]
    before: Vec<ExtraMethod>,
    #[serde(default)]
    after: Vec<ExtraMethod>,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(untagged)]
enum ExtraMethod {
    Bus(BusMethod),
    Func(Function),
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
struct BusMethod {
    method: String,
    #[serde(default, skip_serializing_if = "ValueOptionOwned::is_none")]
    params: ValueOptionOwned,
    #[serde(default)]
    _pass: bool,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
struct Function {
    function: LocalFunction,
    #[serde(default)]
    args: Vec<Value>,
    #[serde(default)]
    _pass: bool,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "snake_case")]
enum LocalFunction {
    Sleep,
    System,
}

fn default_acl_svc() -> String {
    "eva.aaa.acl".to_owned()
}

fn default_auth_svc() -> String {
    "eva.aaa.localauth".to_owned()
}

fn default_filemgr_svc() -> String {
    "eva.filemgr.main".to_owned()
}

fn default_generator_svc() -> String {
    "eva.generator.default".to_owned()
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
struct NodeParams {
    #[serde(default = "default_acl_svc")]
    acl_svc: String,
    #[serde(default = "default_auth_svc")]
    key_svc: String,
    #[serde(default = "default_auth_svc")]
    user_svc: String,
    #[serde(default = "default_filemgr_svc")]
    filemgr_svc: String,
    #[serde(default = "default_generator_svc")]
    generator_svc: String,
}

impl Default for NodeParams {
    fn default() -> Self {
        Self {
            acl_svc: default_acl_svc(),
            key_svc: default_auth_svc(),
            user_svc: default_auth_svc(),
            filemgr_svc: default_filemgr_svc(),
            generator_svc: default_generator_svc(),
        }
    }
}

/// copied form filemgr, merge in the future
#[derive(Deserialize, Serialize, Eq, PartialEq, Copy, Clone, Debug)]
#[serde(rename_all = "lowercase")]
enum Extract {
    No,
    Txz,
    Tgz,
    Tbz2,
    Zip,
    Tar,
}

#[derive(Deserialize, Serialize, Debug, Copy, Clone)]
#[serde(untagged)]
enum ExtractBoolOrStr {
    Bool(bool),
    Extr(Extract),
}

impl Default for ExtractBoolOrStr {
    fn default() -> Self {
        Self::Bool(false)
    }
}

impl ExtractBoolOrStr {
    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn is_no(&self) -> bool {
        match self {
            ExtractBoolOrStr::Bool(v) => !v,
            ExtractBoolOrStr::Extr(x) => *x == Extract::No,
        }
    }
    fn normalize(self, src: Option<&Path>) -> EResult<Self> {
        match self {
            ExtractBoolOrStr::Bool(v) => {
                if v {
                    if let Some(s) = src {
                        if let Some(fname) = s.file_name().map(std::ffi::OsStr::to_string_lossy) {
                            if fname.ends_with(".tgz") || fname.ends_with(".tar.gz") {
                                Ok(ExtractBoolOrStr::Extr(Extract::Tgz))
                            } else if fname.ends_with(".tbz")
                                || fname.ends_with(".tar.bz")
                                || fname.ends_with(".tbz2")
                                || fname.ends_with(".tar.bz2")
                            {
                                Ok(ExtractBoolOrStr::Extr(Extract::Tbz2))
                            } else if fname.ends_with(".txz") || fname.ends_with(".tar.xz") {
                                Ok(ExtractBoolOrStr::Extr(Extract::Txz))
                            } else if fname.ends_with(".zip") {
                                Ok(ExtractBoolOrStr::Extr(Extract::Zip))
                            } else if fname.ends_with(".tar") {
                                Ok(ExtractBoolOrStr::Extr(Extract::Tar))
                            } else {
                                Err(Error::failed(
                                    "Extraction requested but the source extension is unsupported",
                                ))
                            }
                        } else {
                            Err(Error::failed(
                                "Extraction requested but the source extension is unknown",
                            ))
                        }
                    } else {
                        Err(Error::failed(
                            "Extraction requested but the source is not specified",
                        ))
                    }
                } else {
                    Ok(ExtractBoolOrStr::Extr(Extract::No))
                }
            }
            ExtractBoolOrStr::Extr(_) => Ok(self),
        }
    }
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
struct NodeUpload {
    #[serde(default)]
    src: Option<PathBuf>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    url: Option<String>,
    target: PathBuf,
    #[serde(default)]
    svc: String,
    #[serde(default)]
    permissions: Option<u32>,
    #[serde(default)]
    extract: ExtractBoolOrStr,
}

#[derive(Deserialize)]
struct SvcStatus {
    status: String,
    pid: Option<u32>,
}

#[derive(Serialize)]
struct ParamsFilePut<'a> {
    path: &'a str,
    content: &'a [u8],
    #[serde(skip_serializing_if = "Option::is_none")]
    permissions: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha256: Option<[u8; 32]>,
    #[serde(skip_serializing_if = "ExtractBoolOrStr::is_no")]
    extract: ExtractBoolOrStr,
    download: bool,
}

#[derive(Serialize)]
struct ParamsFileUnlink<'a> {
    path: &'a str,
}
