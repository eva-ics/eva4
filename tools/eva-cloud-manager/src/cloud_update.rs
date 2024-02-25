use clap::Parser;
use colored::Colorize;
use eva_client::{EvaClient, EvaCloudClient, NodeMap, SystemInfo, VersionInfo};
use eva_common::common_payloads::{NodeData, ParamsId};
use eva_common::prelude::*;
use log::{error, info, warn};
use prettytable::row;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::time::Duration;

const WAIT_SLEEP_STEP: Duration = Duration::from_secs(2);

#[derive(Deserialize, Debug, bmart::tools::Sorting)]
#[sorting(id = "name")]
struct NodeDataN {
    name: String,
    #[serde(flatten)]
    data: NodeData,
}

#[derive(Deserialize)]
struct NodeDataS {
    #[serde(default)]
    managed: bool,
    #[serde(default)]
    online: bool,
    #[serde(flatten)]
    ver: VersionInfo,
}

#[derive(Deserialize)]
struct SPointDataS {
    name: String,
    #[serde(flatten)]
    ver: Option<VersionInfo>,
}

impl SPointDataS {
    fn id(&self, node: &str) -> String {
        format!(
            "{}/{}",
            node,
            self.name.strip_prefix("eva.spoint.").unwrap_or(&self.name)
        )
    }
}

#[derive(Deserialize, PartialEq, Eq)]
struct SPointDataFull {
    node: String,
    name: String,
    #[serde(flatten)]
    ver: VersionInfo,
}

impl SPointDataFull {
    fn id(&self) -> String {
        format!(
            "{}/{}",
            self.node,
            self.name.strip_prefix("eva.spoint.").unwrap_or(&self.name)
        )
    }
}

impl Ord for SPointDataFull {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match self.node.cmp(&other.node) {
            std::cmp::Ordering::Equal => self.name.cmp(&other.name),
            v => v,
        }
    }
}

impl PartialOrd for SPointDataFull {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Serialize)]
struct ParamsStartUpdate<'a> {
    yes: bool,
    #[serde(flatten)]
    ver: &'a VersionInfo,
}

struct UpdateResult {
    ver: Option<VersionInfo>,
    kind: UpdateResultKind,
}

impl UpdateResult {
    fn success(ver: VersionInfo) -> Self {
        Self {
            ver: Some(ver),
            kind: UpdateResultKind::Success,
        }
    }
    fn upd_fail(ver: VersionInfo) -> Self {
        Self {
            ver: Some(ver),
            kind: UpdateResultKind::UpdateFail,
        }
    }
    fn start_fail(ver: VersionInfo) -> Self {
        Self {
            ver: Some(ver),
            kind: UpdateResultKind::StartFail,
        }
    }
    fn fail(kind: UpdateResultKind) -> Self {
        Self { ver: None, kind }
    }
    fn version(&self) -> Option<&str> {
        self.ver.as_ref().map(|v| v.version.as_str())
    }
    fn build(&self) -> Option<u64> {
        self.ver.as_ref().map(|v| v.build)
    }
    fn is_success(&self) -> bool {
        self.kind == UpdateResultKind::Success
    }
}

#[derive(Eq, PartialEq, Copy, Clone)]
enum UpdateResultKind {
    Success,
    StartFail,
    UpdateFail,
    Offline,
}

impl fmt::Display for UpdateResultKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                UpdateResultKind::Success => "success".green().bold(),
                UpdateResultKind::StartFail => "failed to begin".yellow().bold(),
                UpdateResultKind::UpdateFail => "not updated".yellow().bold(),
                UpdateResultKind::Offline => "OFFLINE".red().bold(),
            }
        )
    }
}

#[derive(Parser, Clone, Debug)]
pub struct Options {
    #[clap(short = 't', long = "timeout", default_value = "10")]
    pub timeout: f64,
    #[clap(long = "check-timeout", default_value = "120")]
    pub check_timeout: f64,
    #[clap(short = 'C', long = "connection-path")]
    connection_path: Option<String>,
    #[clap(help = "Nodes to update, for s-points: <NODE_NAME>/<SPOINT>")]
    node: Vec<String>,
    #[clap(long = "all", help = "update all nodes")]
    all: bool,
    #[clap(
        short = 'i',
        long = "info-only",
        help = "display the update plan and exit"
    )]
    info_only: bool,
    #[clap(long = "YES", help = "update without any confirmations")]
    yes: bool,
}

#[derive(bmart::tools::Sorting)]
#[sorting(id = "name")]
struct UpdateInfoRow {
    name: String,
    current: VersionInfo,
}

impl From<&NodeDataN> for UpdateInfoRow {
    fn from(n: &NodeDataN) -> Self {
        let i = n.data.info().unwrap();
        Self {
            name: n.name.clone(),
            current: VersionInfo {
                version: i.version.clone(),
                build: i.build,
            },
        }
    }
}

impl From<&SPointDataFull> for UpdateInfoRow {
    fn from(s: &SPointDataFull) -> Self {
        Self {
            name: s.id(),
            current: s.ver.clone(),
        }
    }
}

#[allow(clippy::module_name_repetitions)]
#[allow(clippy::too_many_lines)]
pub async fn cloud_update(opts: Options) -> EResult<()> {
    if (opts.node.is_empty() && !opts.all) || (!opts.node.is_empty() && opts.all) {
        return Err(Error::invalid_params(
            "specify either nodes to update or --all",
        ));
    }
    let mut update_all = opts.all;
    for n in &opts.node {
        if n == "#" || n == "*" {
            update_all = true;
            break;
        }
    }
    let mut requested_nodes = HashSet::new();
    // required to make client map only
    let mut requested_spoint_nodes = HashSet::new();
    for n in opts.node {
        if let Some(pos) = n.find('/') {
            requested_spoint_nodes.insert(n[..pos].to_owned());
        }
        requested_nodes.insert(n);
    }
    let connection_path = opts
        .connection_path
        .unwrap_or_else(|| crate::DEFAULT_CONNECTION_PATH.to_string());
    let timeout = Duration::from_secs_f64(opts.timeout);
    let check_timeout = Duration::from_secs_f64(opts.check_timeout);
    info!("connecting to {}", connection_path);
    let bus_client = EvaClient::connect(
        &connection_path,
        ecm::BUS_CLIENT_NAME,
        eva_client::Config::new().timeout(timeout),
    )
    .await?;
    info!("gathering facts");
    let info = bus_client.get_system_info().await?;
    if !info.active {
        return Err(Error::failed("system is not active"));
    }
    let system_name = info.system_name;
    info!("local node: {}", system_name);
    let mut node_map = NodeMap::new();
    let nodes_all: Vec<NodeDataN> = match bus_client.call("eva.core", "node.list", None).await {
        Ok(v) => v,
        Err(e) => {
            error!("unable to fetch node list");
            return Err(e);
        }
    };
    let nodes = nodes_all
        .iter()
        .filter(|node| {
            node.data.svc().is_some()
                && node.name != system_name
                && node.data.online()
                && (update_all
                    || requested_nodes.contains(&node.name)
                    || requested_spoint_nodes.contains(&node.name))
        })
        .collect::<Vec<&NodeDataN>>();
    let mut update_candidates = Vec::new();
    for node in nodes {
        if let Some(svc) = node.data.svc() {
            let ns = match bus_client
                .call::<NodeDataS>(svc, "node.get", Some(to_value(ParamsId { i: &node.name })?))
                .await
            {
                Ok(v) => Some(v),
                Err(e) => {
                    warn!("Unable to get node info: {}", e);
                    None
                }
            };
            if let Some(ns) = ns {
                if ns.managed {
                    node_map.insert(node.name.clone(), svc.to_owned());
                    if update_all || requested_nodes.contains(&node.name) {
                        if let Some(i) = node.data.info() {
                            if info.ver.major_matches(&i.version).unwrap_or_default()
                                && i.build < info.ver.build
                            {
                                info!("adding node {} to the update plan", node.name);
                                update_candidates.push(node);
                            }
                        }
                    }
                } else {
                    warn!("node {} is not managed, skipped", node.name);
                }
            }
        }
    }
    let mut spoint_update_candidates: Vec<SPointDataFull> = Vec::new();
    let client = EvaCloudClient::new(&system_name, bus_client, node_map);
    for node in &nodes_all {
        if update_all || requested_spoint_nodes.contains(&node.name) {
            let spoint_node_data: Vec<SPointDataFull> = match client
                .call::<Vec<SPointDataS>>(&node.name, "eva.core", "spoint.list", None)
                .await
            {
                Ok(v) => v
                    .into_iter()
                    .filter(|sp| {
                        (update_all
                            || requested_nodes.contains(&sp.id(&node.name))
                            || requested_nodes.contains(&format!("{}/*", node.name))
                            || requested_nodes.contains(&format!("{}/#", node.name)))
                            && sp.ver.as_ref().map_or(false, |i| {
                                info.ver.major_matches(&i.version).unwrap_or_default()
                                    && i.build < info.ver.build
                            })
                    })
                    .map(|sp| SPointDataFull {
                        node: node.name.clone(),
                        name: sp.name,
                        ver: sp.ver.unwrap(),
                    })
                    .collect(),
                Err(e) => {
                    warn!(
                        "Unable to get s-point data for the node {}: {}",
                        node.name, e
                    );
                    Vec::new()
                }
            };
            for d in spoint_node_data {
                info!("adding s-point {} to the update plan", d.id());
                spoint_update_candidates.push(d);
            }
        }
    }
    if update_candidates.is_empty() && spoint_update_candidates.is_empty() {
        info!("no update candidates found");
        return Ok(());
    }
    update_candidates.sort();
    spoint_update_candidates.sort();
    if !opts.yes || opts.info_only {
        let mut update_info: Vec<UpdateInfoRow> = Vec::new();
        for c in &update_candidates {
            update_info.push((*c).into());
        }
        for c in &spoint_update_candidates {
            update_info.push(c.into());
        }
        update_info.sort();
        let mut tbl = ecm::tools::ctable(&[
            "node/spoint",
            "cur.ver",
            "cur.build",
            "new.ver",
            "new.build",
        ]);
        for c in update_info {
            tbl.add_row(row![
                c.name,
                c.current.version.cyan(),
                c.current.build.to_string().cyan(),
                info.ver.version.yellow(),
                info.ver.build.to_string().yellow()
            ]);
        }
        tbl.printstd();
        println!();
        if opts.info_only {
            return Ok(());
        }
        ecm::tools::confirm("apply the update plan").await?;
    }
    warn!("applying the update plan");
    let mut wait_candidates = Vec::new();
    let mut result = BTreeMap::new();
    let payload = to_value(ParamsStartUpdate {
        yes: true,
        ver: &info.ver,
    })?;
    for c in update_candidates {
        info!("sending update command to the node {}", c.name);
        match client
            .call::<()>(&c.name, "eva.core", "update", Some(payload.clone()))
            .await
        {
            Ok(()) => {
                info!("command accepted");
                wait_candidates.push(c);
            }
            Err(e) => {
                warn!("command failed: {}", e);
                result.insert(
                    c.name.clone(),
                    UpdateResult::start_fail(c.data.info().unwrap().clone().into()),
                );
            }
        }
    }
    // update nodes
    info!("updating the nodes");
    let op = eva_common::op::Op::new(check_timeout);
    ttycarousel::tokio1::spawn0("Waiting").await;
    while !op.is_timed_out() && !wait_candidates.is_empty() {
        tokio::time::sleep(WAIT_SLEEP_STEP).await;
        let mut updated = HashSet::new();
        for c in &wait_candidates {
            if let Ok(v) = client.get_system_info(&c.name).await {
                if v.ver == info.ver {
                    updated.insert(c.name.clone());
                    result.insert(c.name.clone(), UpdateResult::success(v.ver));
                }
            }
        }
        wait_candidates.retain(|n| !updated.contains(&n.name));
    }
    ttycarousel::tokio1::stop_clear().await;
    // process failed nodes
    for n in wait_candidates {
        match client
            .call::<NodeDataS>(
                ".local",
                n.data.svc().unwrap(),
                "node.get",
                Some(to_value(ParamsId { i: &n.name })?),
            )
            .await
        {
            Ok(v) => {
                if v.online {
                    result.insert(n.name.clone(), UpdateResult::upd_fail(v.ver));
                } else {
                    result.insert(
                        n.name.clone(),
                        UpdateResult::fail(UpdateResultKind::Offline),
                    );
                }
            }
            Err(_) => {
                result.insert(
                    n.name.clone(),
                    UpdateResult::fail(UpdateResultKind::Offline),
                );
            }
        }
    }
    // update s-points
    let mut wait_candidates = Vec::new();
    spoint_update_candidates.retain(|s| {
        if result.get(&s.node).map_or(true, UpdateResult::is_success) {
            true
        } else {
            result.insert(s.id(), UpdateResult::upd_fail(s.ver.clone()));
            false
        }
    });
    if !spoint_update_candidates.is_empty() {
        for c in spoint_update_candidates {
            info!("sending update command to the s-point {}", c.id());
            match client
                .call::<()>(&c.node, &c.name, "update", Some(payload.clone()))
                .await
            {
                Ok(()) => {
                    info!("command accepted");
                    wait_candidates.push(c);
                }
                Err(e) => {
                    warn!("command failed: {}", e);
                    result.insert(c.id(), UpdateResult::start_fail(c.ver));
                }
            }
        }
    }
    // update s-points
    info!("updating the s-points");
    let op = eva_common::op::Op::new(check_timeout);
    ttycarousel::tokio1::spawn0("Waiting").await;
    while !op.is_timed_out() && !wait_candidates.is_empty() {
        tokio::time::sleep(WAIT_SLEEP_STEP).await;
        let mut updated = HashSet::new();
        for c in &wait_candidates {
            if let Ok(v) = client
                .call::<SystemInfo>(&c.node, &c.name, "test", None)
                .await
            {
                if v.ver == info.ver {
                    updated.insert(c.id());
                    result.insert(c.id(), UpdateResult::success(v.ver));
                }
            }
        }
        wait_candidates.retain(|n| !updated.contains(&n.id()));
    }
    ttycarousel::tokio1::stop_clear().await;
    // process failed s-points
    for n in wait_candidates {
        match client
            .call::<SystemInfo>(&n.node, &n.name, "test", None)
            .await
        {
            Ok(v) => {
                if v.active {
                    result.insert(n.id(), UpdateResult::upd_fail(v.ver));
                } else {
                    result.insert(n.id(), UpdateResult::fail(UpdateResultKind::Offline));
                }
            }
            Err(_) => {
                result.insert(n.id(), UpdateResult::fail(UpdateResultKind::Offline));
            }
        }
    }
    // print the results
    let mut names: Vec<&str> = result.keys().map(String::as_str).collect();
    names.sort_unstable();
    let mut tbl = ecm::tools::ctable(&["node/spoint", "result", "cur.ver", "cur.build"]);
    let mut success = true;
    for i in names {
        let res = result.get(i).unwrap();
        let mut version = res.version().unwrap_or_default().to_owned();
        let mut build = res.build().map(|v| v.to_string()).unwrap_or_default();
        if res.is_success() {
            version = version.cyan().to_string();
            build = build.cyan().to_string();
        } else {
            success = false;
        }
        tbl.add_row(row![i, res.kind.to_string(), version, build]);
    }
    tbl.printstd();
    println!();
    if success {
        Ok(())
    } else {
        Err(Error::failed("Some nodes/spoints failed to update"))
    }
}
