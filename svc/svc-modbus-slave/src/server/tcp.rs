use crate::server::{Server, handle_frame, parse_host_port};
use async_trait::async_trait;
use eva_common::err_logger;
use eva_common::{EResult, Error};
use log::{info, trace};
use rmodbus::ModbusFrameBuf;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

err_logger!();

#[allow(clippy::module_name_repetitions)]
pub struct TcpServer {
    listener: TcpListener,
    unit: u8,
    timeout: Duration,
    keep_alive_timeout: Duration,
    host: String,
    port: u16,
}

#[async_trait]
impl Server for TcpServer {
    async fn launch(&mut self) -> EResult<()> {
        loop {
            let (socket, addr) = self.listener.accept().await?;
            trace!(
                "TCP {}:{}: connected client: {}",
                self.host, self.port, addr
            );
            let unit = self.unit;
            let timeout = self.timeout;
            let keep_alive_timeout = self.keep_alive_timeout;
            tokio::spawn(async move {
                handle_tcp(socket, unit, timeout, keep_alive_timeout)
                    .await
                    .log_ef();
            });
        }
    }
}

async fn handle_tcp(
    mut stream: TcpStream,
    unit: u8,
    timeout: Duration,
    keep_alive_timeout: Duration,
) -> EResult<()> {
    loop {
        let mut buf: ModbusFrameBuf = [0; 256];
        let Ok(len) = tokio::time::timeout(keep_alive_timeout, stream.read(&mut buf)).await else {
            break;
        };
        if len? == 0 {
            break;
        }
        let mut response = Vec::new();
        let mut frame = rmodbus::server::ModbusFrame::new(
            unit,
            &buf,
            rmodbus::ModbusProto::TcpUdp,
            &mut response,
        );
        handle_frame(&mut frame).await?;
        if frame.response_required {
            frame.finalize_response().map_err(Error::io)?;
            tokio::time::timeout(timeout, stream.write(&response)).await??;
        }
    }
    Ok(())
}

impl TcpServer {
    pub async fn create(
        path: &str,
        unit: u8,
        timeout: Duration,
        keep_alive_timeout: Duration,
    ) -> EResult<Self> {
        let (host, port) = parse_host_port(path)?;
        let listener = TcpListener::bind(format!("{}:{}", host, port)).await?;
        info!("TCP listener at {}:{}, unit {}", host, port, unit);
        Ok(Self {
            listener,
            unit,
            timeout,
            keep_alive_timeout,
            host: host.to_owned(),
            port,
        })
    }
}
