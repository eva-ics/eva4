use crate::server::{handle_frame, parse_host_port, Server};
use async_trait::async_trait;
use eva_common::err_logger;
use eva_common::{EResult, Error};
use log::{info, trace};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::UdpSocket;

err_logger!();

#[allow(clippy::module_name_repetitions)]
pub struct UdpServer {
    socket: UdpSocket,
    unit: u8,
    timeout: Duration,
    host: String,
    port: u16,
}

impl UdpServer {
    pub async fn create(path: &str, unit: u8, timeout: Duration) -> EResult<Self> {
        let (host, port) = parse_host_port(path)?;
        let socket = UdpSocket::bind(format!("{}:{}", host, port)).await?;
        info!("UDP listener at {}:{}, unit {}", host, port, unit);
        Ok(Self {
            socket,
            unit,
            timeout,
            host: host.to_owned(),
            port,
        })
    }
    async fn handle(&self, buf: rmodbus::ModbusFrameBuf, addr: SocketAddr) -> EResult<()> {
        let mut response = Vec::new();
        let mut frame = rmodbus::server::ModbusFrame::new(
            self.unit,
            &buf,
            rmodbus::ModbusProto::TcpUdp,
            &mut response,
        );
        handle_frame(&mut frame).await?;
        if frame.response_required {
            frame.finalize_response().map_err(Error::io)?;
            tokio::time::timeout(self.timeout, self.socket.send_to(&response, addr)).await??;
        }
        Ok(())
    }
}

#[async_trait]
impl Server for UdpServer {
    async fn launch(&mut self) -> EResult<()> {
        loop {
            let mut buf = [0; 256];
            let (_, addr) = self.socket.recv_from(&mut buf).await?;
            trace!("UDP {}:{} frame from {}", self.host, self.port, addr);
            self.handle(buf, addr).await.log_ef();
        }
    }
}
