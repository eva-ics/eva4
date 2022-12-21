use crate::types::{ProtocolKind, Register, RegisterKind};
use async_trait::async_trait;
use busrt::rpc::Rpc;
use busrt::QoS;
use eva_common::{EResult, Error};
use eva_sdk::service::poc;
use log::{error, trace};
use simple_pool::ResourcePool;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio_serial::{DataBits, Parity, SerialPortBuilderExt, SerialStream, StopBits};

const DEFAULT_FRAME_DELAY: Duration = Duration::from_millis(100);

struct ModbusSerialPort {
    stream: SerialStream,
    last_frame: Option<Instant>,
}

struct Bus {
    svc: String,
    timeout: Duration,
}

impl Bus {
    pub fn new(path: &str, timeout: Duration) -> Self {
        Self {
            svc: path.to_owned(),
            timeout,
        }
    }
}

#[async_trait]
impl ModbusClient for Bus {
    async fn process_request(&self, payload: Vec<u8>) -> EResult<Vec<u8>> {
        let rpc = crate::RPC.get().unwrap();
        let res = tokio::time::timeout(
            self.timeout,
            rpc.call(&self.svc, "MB", payload.into(), QoS::Realtime),
        )
        .await??;
        Ok(res.payload().to_vec())
    }
}

struct Serial {
    modbus_serial_port: tokio::sync::Mutex<ModbusSerialPort>,
    frame_delay: Duration,
    proto: rmodbus::ModbusProto,
}

impl Serial {
    pub fn create(
        path: &str,
        proto: rmodbus::ModbusProto,
        timeout: Duration,
        frame_delay: Option<Duration>,
    ) -> EResult<Self> {
        let mut sp = path.split(':');
        let port_path = sp.next().unwrap();
        let baud_rate: u32 = sp
            .next()
            .ok_or_else(|| Error::invalid_params("baud rate missing"))?
            .parse()?;
        let data_bits: DataBits = match sp
            .next()
            .ok_or_else(|| Error::invalid_params("data bits missing"))?
        {
            "5" => DataBits::Five,
            "6" => DataBits::Six,
            "7" => DataBits::Seven,
            "8" => DataBits::Eight,
            v => {
                return Err(Error::invalid_params(format!(
                    "unsupported data bits value: {}",
                    v
                )))
            }
        };
        let parity: Parity = match sp
            .next()
            .ok_or_else(|| Error::invalid_params("parity missing"))?
        {
            "N" => Parity::None,
            "E" => Parity::Even,
            "O" => Parity::Odd,
            v => {
                return Err(Error::invalid_params(format!(
                    "unsupported parity value: {}",
                    v
                )))
            }
        };
        let stop_bits: StopBits = match sp
            .next()
            .ok_or_else(|| Error::invalid_params("stop bits missing"))?
        {
            "1" => StopBits::One,
            "2" => StopBits::Two,
            v => {
                return Err(Error::invalid_params(format!(
                    "unsupported stop bits value: {}",
                    v
                )))
            }
        };
        let stream = tokio_serial::new(port_path, baud_rate)
            .data_bits(data_bits)
            .parity(parity)
            .stop_bits(stop_bits)
            .timeout(timeout)
            .open_native_async()
            .map_err(Error::io)?;
        Ok(Self {
            modbus_serial_port: tokio::sync::Mutex::new(ModbusSerialPort {
                stream,
                last_frame: None,
            }),
            frame_delay: frame_delay.unwrap_or(DEFAULT_FRAME_DELAY),
            proto,
        })
    }
}

#[async_trait]
impl ModbusClient for Serial {
    async fn process_request(&self, payload: Vec<u8>) -> EResult<Vec<u8>> {
        let mut client = self.modbus_serial_port.lock().await;
        macro_rules! unwrap_or_poc {
            ($res: expr) => {
                match $res {
                    Ok(v) => v,
                    Err(e) => {
                        client.last_frame.replace(Instant::now());
                        error!("serial port error: {}", e);
                        poc();
                        return Err(e.into());
                    }
                }
            };
        }
        macro_rules! unwrap_or_err {
            ($res: expr) => {
                match $res {
                    Ok(v) => v,
                    Err(e) => {
                        client.last_frame.replace(Instant::now());
                        return Err(e);
                    }
                }
            };
        }
        if let Some(last_frame) = client.last_frame {
            let el = last_frame.elapsed();
            if el < self.frame_delay {
                tokio::time::sleep(self.frame_delay - el).await;
            }
        }
        if self.proto == rmodbus::ModbusProto::Rtu {
            unwrap_or_poc!(client.stream.write_all(&payload).await);
            let mut buf = [0u8; 3];
            unwrap_or_err!(client.stream.read_exact(&mut buf).await.map_err(Into::into));
            let mut response = buf.to_vec();
            let len = unwrap_or_err!(rmodbus::guess_response_frame_len(
                &buf,
                rmodbus::ModbusProto::Rtu
            )
            .map_err(Error::io));
            if len > 3 {
                let mut rest = vec![0u8; (len - 3) as usize];
                unwrap_or_err!(client
                    .stream
                    .read_exact(&mut rest)
                    .await
                    .map_err(Into::into));
                response.extend(rest);
            }
            client.last_frame.replace(Instant::now());
            Ok(response)
        } else {
            let mut ascii_frame = Vec::new();
            rmodbus::generate_ascii_frame(&payload, &mut ascii_frame).map_err(Error::io)?;
            unwrap_or_poc!(client.stream.write_all(&ascii_frame).await);
            let mut buf = [0u8; 7];
            unwrap_or_err!(client.stream.read_exact(&mut buf).await.map_err(Into::into));
            let mut response = buf.to_vec();
            let len = unwrap_or_err!(rmodbus::guess_response_frame_len(
                &buf,
                rmodbus::ModbusProto::Ascii
            )
            .map_err(Error::io));
            if len > 7 {
                let mut rest = vec![0u8; (len - 7) as usize];
                unwrap_or_err!(client
                    .stream
                    .read_exact(&mut rest)
                    .await
                    .map_err(Into::into));
                response.extend(rest);
            }
            client.last_frame.replace(Instant::now());
            let mut buf: rmodbus::ModbusFrameBuf = [0; 256];
            let res_len = rmodbus::parse_ascii_frame(&response, response.len(), &mut buf, 0)
                .map_err(Error::io)?;
            Ok(buf[..res_len as usize].to_vec())
        }
    }
}

struct Tcp {
    target: String,
    pool: ResourcePool<TcpStream>,
}

impl Tcp {
    pub async fn create(
        host: &str,
        port: u16,
        connect_timeout: Duration,
        pool_size: usize,
    ) -> EResult<Self> {
        let pool = ResourcePool::new();
        let target = format!("{}:{}", host, port);
        for _ in 0..pool_size {
            let client =
                tokio::time::timeout(connect_timeout, TcpStream::connect(&target)).await??;
            client.set_nodelay(true)?;
            pool.append(client);
        }
        Ok(Self { target, pool })
    }
}

struct Udp {
    pool: ResourcePool<UdpSocket>,
    target: SocketAddr,
}

impl Udp {
    pub async fn create(
        host: &str,
        port: u16,
        bind_timeout: Duration,
        pool_size: usize,
    ) -> EResult<Self> {
        let target: SocketAddr =
            SocketAddr::new(host.parse().map_err(Error::invalid_params)?, port);
        let pool = ResourcePool::new();
        for _ in 0..pool_size {
            let client = tokio::time::timeout(bind_timeout, UdpSocket::bind("0.0.0.0:0")).await??;
            pool.append(client);
        }
        Ok(Self { pool, target })
    }
}

#[async_trait]
trait ModbusClient {
    async fn process_request(&self, payload: Vec<u8>) -> EResult<Vec<u8>>;
}

#[async_trait]
impl ModbusClient for Tcp {
    async fn process_request(&self, payload: Vec<u8>) -> EResult<Vec<u8>> {
        let mut client = self.pool.get().await;
        if let Err(e) = client.write_all(&payload).await {
            trace!("tcp stream error: {}, trying to reconnect", e);
            let mut new_client = TcpStream::connect(&self.target).await?;
            new_client.write_all(&payload).await?;
            client.replace_resource(new_client);
        }
        let mut buf = [0u8; 6];
        client.read_exact(&mut buf).await?;
        let mut response = buf.to_vec();
        let len = rmodbus::guess_response_frame_len(&buf, rmodbus::ModbusProto::TcpUdp)
            .map_err(Error::failed)?;
        if len > 6 {
            let mut rest = vec![0u8; (len - 6) as usize];
            client.read_exact(&mut rest).await?;
            response.extend(rest);
        }
        Ok(response)
    }
}

#[async_trait]
impl ModbusClient for Udp {
    async fn process_request(&self, payload: Vec<u8>) -> EResult<Vec<u8>> {
        let client = self.pool.get().await;
        let len = client.send_to(&payload, self.target).await?;
        if len != payload.len() {
            return Err(Error::io("communication error, payload not sent in full"));
        }
        let mut buf: rmodbus::ModbusFrameBuf = [0; 256];
        let (n, addr) = client.recv_from(&mut buf).await?;
        if addr != self.target {
            return Err(Error::io("communication error, invalid packet source"));
        }
        Ok(buf[..n].to_vec())
    }
}

struct TrId(u16);

impl Default for TrId {
    fn default() -> Self {
        Self(1)
    }
}

impl TrId {
    fn generate(&mut self) -> u16 {
        let curr = self.0;
        let tr_id = if curr == std::u16::MAX { 1 } else { curr + 1 };
        self.0 = curr;
        tr_id
    }
}

macro_rules! req {
    ($self: expr, $unit_id: expr) => {
        if $self.proto == rmodbus::ModbusProto::TcpUdp {
            rmodbus::client::ModbusRequest::new_tcp_udp(
                $unit_id,
                $self.tr_id.lock().unwrap().generate(),
            )
        } else {
            rmodbus::client::ModbusRequest::new($unit_id, $self.proto)
        }
    };
}

pub struct Client {
    client: Box<dyn ModbusClient + Send + Sync>,
    tr_id: std::sync::Mutex<TrId>,
    proto: rmodbus::ModbusProto,
}

fn parse_host_port(path: &str) -> EResult<(&str, u16)> {
    let mut sp = path.splitn(2, ':');
    let host = sp.next().unwrap();
    let port = if let Some(v) = sp.next() {
        v.parse()?
    } else {
        502
    };
    Ok((host, port))
}

impl Client {
    pub async fn create(
        path: &str,
        timeout: Duration,
        pool_size: usize,
        kind: ProtocolKind,
        frame_delay: Option<Duration>,
    ) -> EResult<Self> {
        let (client, proto) = match kind {
            ProtocolKind::Tcp => {
                let (host, port) = parse_host_port(path)?;
                let client: Box<dyn ModbusClient + Send + Sync> =
                    Box::new(Tcp::create(host, port, timeout, pool_size).await?);
                (client, rmodbus::ModbusProto::TcpUdp)
            }
            ProtocolKind::Udp => {
                let (host, port) = parse_host_port(path)?;
                let client: Box<dyn ModbusClient + Send + Sync> =
                    Box::new(Udp::create(host, port, timeout, pool_size).await?);
                (client, rmodbus::ModbusProto::TcpUdp)
            }
            ProtocolKind::Rtu | ProtocolKind::Ascii => {
                let proto = kind.into();
                let client: Box<dyn ModbusClient + Send + Sync> =
                    Box::new(Serial::create(path, proto, timeout, frame_delay)?);
                (client, proto)
            }
            ProtocolKind::Native => {
                let proto = kind.into();
                let client: Box<dyn ModbusClient + Send + Sync> = Box::new(Bus::new(path, timeout));
                (client, proto)
            }
        };
        Ok(Self {
            client,
            tr_id: <_>::default(),
            proto,
        })
    }
    pub async fn safe_get_u16(
        &self,
        unit_id: u8,
        reg: Register,
        count: u16,
        timeout: Duration,
    ) -> EResult<Vec<u16>> {
        tokio::time::timeout(timeout, self.get_u16(unit_id, reg, count))
            .await?
            .map_err(Into::into)
    }
    pub async fn get_u16(&self, unit_id: u8, reg: Register, count: u16) -> EResult<Vec<u16>> {
        let mut mreq = req!(self, unit_id);
        let mut request = Vec::new();
        match reg.kind() {
            RegisterKind::Input => {
                mreq.generate_get_inputs(reg.number(), count, &mut request)
                    .map_err(Error::failed)?;
            }
            RegisterKind::Holding => {
                mreq.generate_get_holdings(reg.number(), count, &mut request)
                    .map_err(Error::failed)?;
            }
            _ => panic!("u16 req for coils"),
        };
        let response = self.client.process_request(request).await?;
        let mut result = Vec::new();
        mreq.parse_u16(&response, &mut result)
            .map_err(Error::failed)?;
        Ok(result)
    }
    pub async fn set_bool(&self, unit_id: u8, reg: Register, value: bool) -> EResult<()> {
        let mut mreq = req!(self, unit_id);
        let mut request = Vec::new();
        match reg.kind() {
            RegisterKind::Coil => {
                mreq.generate_set_coil(reg.number(), value, &mut request)
                    .map_err(Error::failed)?;
            }
            _ => panic!("bool set req for read/only or u16 regs"),
        };
        let response = self.client.process_request(request).await?;
        mreq.parse_ok(&response).map_err(Error::failed)?;
        Ok(())
    }
    pub async fn set_u16_bulk(&self, unit_id: u8, reg: Register, value: &[u16]) -> EResult<()> {
        let mut mreq = req!(self, unit_id);
        let mut request = Vec::new();
        match reg.kind() {
            RegisterKind::Holding => {
                mreq.generate_set_holdings_bulk(reg.number(), value, &mut request)
                    .map_err(Error::failed)?;
            }
            _ => panic!("u16 set req for read/only or u16 regs"),
        };
        let response = self.client.process_request(request).await?;
        mreq.parse_ok(&response).map_err(Error::failed)?;
        Ok(())
    }
    pub async fn safe_get_bool(
        &self,
        unit_id: u8,
        reg: Register,
        count: u16,
        timeout: Duration,
    ) -> EResult<Vec<bool>> {
        tokio::time::timeout(timeout, self.get_bool(unit_id, reg, count))
            .await?
            .map_err(Into::into)
    }
    pub async fn get_bool(&self, unit_id: u8, reg: Register, count: u16) -> EResult<Vec<bool>> {
        let mut mreq = req!(self, unit_id);
        let mut request = Vec::new();
        match reg.kind() {
            RegisterKind::Coil => {
                mreq.generate_get_coils(reg.number(), count, &mut request)
                    .map_err(Error::failed)?;
            }
            RegisterKind::Discrete => {
                mreq.generate_get_discretes(reg.number(), count, &mut request)
                    .map_err(Error::failed)?;
            }
            _ => panic!("bool req for u16 regs"),
        };
        let response = self.client.process_request(request).await?;
        let mut result = Vec::new();
        mreq.parse_bool(&response, &mut result)
            .map_err(Error::failed)?;
        Ok(result)
    }
}
