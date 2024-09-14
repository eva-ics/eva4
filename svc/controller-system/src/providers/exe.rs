use crate::metric::Metric;
use crate::tools::format_name;
use eva_common::err_logger;
use eva_common::prelude::*;
use eva_common::transform;
use log::{error, info};
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::time::Duration;

err_logger!();

static CONFIG: OnceCell<Config> = OnceCell::new();

pub fn set_config(mut config: Config) -> EResult<()> {
    for task in &mut config.tasks {
        task.name = format_name(&task.name).to_string();
        for m in &mut task.map {
            m.name = format_name(&m.name).to_string();
        }
    }
    CONFIG
        .set(config)
        .map_err(|_| Error::core("Unable to set EXE CONFIG"))
}

fn default_jp() -> String {
    "$.".to_owned()
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    tasks: Vec<Task>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Task {
    #[serde(default)]
    enabled: bool,
    #[serde(deserialize_with = "eva_common::tools::de_float_as_duration")]
    interval: Duration,
    name: String,
    command: String,
    #[serde(
        default,
        deserialize_with = "eva_common::tools::de_opt_float_as_duration"
    )]
    timeout: Option<Duration>,
    #[serde(default)]
    map: Vec<MappingEntry>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct MappingEntry {
    #[serde(default = "default_jp")]
    path: String,
    name: String,
    #[serde(default)]
    transform: Vec<transform::Task>,
}

#[cfg(target_os = "windows")]
const SHELL: &str = "cmd.exe";

#[cfg(not(target_os = "windows"))]
const SHELL: &str = "sh";

async fn report_failed(task: &Task) {
    for m in &task.map {
        Metric::new0(&task.name, &m.name).failed().report(-1).await;
    }
}

async fn process_result(task: &Task, value: Value) {
    for m in &task.map {
        macro_rules! report_failed {
            () => {
                Metric::new0(&task.name, &m.name).failed().report(-1).await;
            };
        }
        match value.jp_lookup(&m.path) {
            Ok(Some(mut v)) => {
                let mut value_transformed = None;
                if !m.transform.is_empty() {
                    let f = match f64::try_from(v) {
                        Ok(n) => n,
                        Err(e) => {
                            error!(
                                "{} unable to parse value for transform ({}/{}): {}",
                                task.name, m.name, value, e
                            );
                            report_failed!();
                            continue;
                        }
                    };
                    // virtual OID, just for transform
                    let oid: OID = format!("sensor:__tmp__/{}/{}", task.name, m.name)
                        .parse()
                        .unwrap();
                    match transform::transform(&m.transform, &oid, f) {
                        Ok(n) => {
                            value_transformed.replace(Value::F64(n));
                            v = value_transformed.as_ref().unwrap();
                        }
                        Err(e) => {
                            error!(
                                "{} unable to transform the value ({}/{}): {}",
                                task.name, m.name, value, e
                            );
                            report_failed!();
                            continue;
                        }
                    }
                }
                Metric::new0(&task.name, &m.name).report(v).await;
            }
            Ok(None) => {
                error!(
                    "{}/{} value process error: path not found: {}",
                    task.name, m.name, m.path
                );
                report_failed!();
            }
            Err(e) => {
                error!("{}/{} value process error: {}", task.name, m.name, e);
                report_failed!();
            }
        }
    }
}

async fn task_worker(task: &Task) {
    info!("exe report worker spawned task {}", task.name);
    let mut int = tokio::time::interval(task.interval);
    int.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let timeout = task.timeout.unwrap_or(eva_common::DEFAULT_TIMEOUT);
    #[cfg(target_os = "windows")]
    let args = vec!["/c", &task.command];
    #[cfg(not(target_os = "windows"))]
    let args = vec!["-c", &task.command];
    loop {
        int.tick().await;
        let result =
            bmart::process::command(SHELL, &args, timeout, bmart::process::Options::default())
                .await;
        match result {
            Ok(v) => {
                if v.ok() {
                    process_result(task, v.out.join("").parse().unwrap()).await;
                } else {
                    error!(
                        "task {} exited with code: {}",
                        task.name,
                        v.code.unwrap_or(-255)
                    );
                    report_failed(task).await;
                }
            }
            Err(e) => {
                error!("task {} failed: {}", task.name, e);
                report_failed(task).await;
            }
        }
    }
}

/// # Panics
///
/// will panic if config is not set
#[allow(clippy::unused_async)]
pub async fn report_worker() {
    let config = CONFIG.get().unwrap();
    for task in &config.tasks {
        if task.enabled {
            tokio::spawn(task_worker(task));
        }
    }
}
