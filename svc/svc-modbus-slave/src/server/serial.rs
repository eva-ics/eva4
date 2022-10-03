use crate::server::{handle_frame, Server};
use async_trait::async_trait;
use eva_common::err_logger;
use eva_common::{EResult, Error};
use log::{info, trace};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_serial::{DataBits, Parity, SerialPortBuilderExt, SerialStream, StopBits};

err_logger!();

#[allow(clippy::module_name_repetitions)]
pub struct SerialServer {
    stream: SerialStream,
    unit: u8,
    timeout: Duration,
    path: String,
    proto: rmodbus::ModbusProto,
}

#[async_trait]
impl Server for SerialServer {
    async fn launch(&mut self) -> EResult<()> {
        match self.proto {
            rmodbus::ModbusProto::Rtu => loop {
                let mut buf = [0u8; 3];
                self.stream.read_exact(&mut buf).await?;
                trace!("{} rtu frame", self.path);
                self.handle_rtu_frame(buf).await.log_ef();
            },
            rmodbus::ModbusProto::Ascii => loop {
                let mut buf = [0u8; 7];
                self.stream.read_exact(&mut buf).await?;
                trace!("{} ascii frame", self.path);
                self.handle_ascii_frame(buf).await.log_ef();
            },
            rmodbus::ModbusProto::TcpUdp => panic!(),
        }
    }
}

impl SerialServer {
    pub async fn create(
        path: &str,
        unit: u8,
        timeout: Duration,
        proto: rmodbus::ModbusProto,
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
        info!(
            "Serial listener at {} ({}), unit {}",
            path,
            format!("{:?}", proto).to_lowercase(),
            unit
        );
        Ok(Self {
            stream,
            unit,
            timeout,
            path: path.to_owned(),
            proto,
        })
    }
    async fn handle_rtu_frame(&mut self, initial_buf: [u8; 3]) -> EResult<()> {
        let len = rmodbus::guess_request_frame_len(&initial_buf, rmodbus::ModbusProto::Rtu)
            .map_err(Error::io)?;
        let mut req = initial_buf.to_vec();
        if len > 3 {
            let mut rest = vec![0u8; (len - 3) as usize];
            tokio::time::timeout(self.timeout, self.stream.read_exact(&mut rest)).await??;
            req.extend(rest);
        }
        if req.len() > 256 {
            return Err(Error::io("invalid frame len"));
        }
        req.resize(256, 0);
        let mut response = Vec::new();
        let mut frame = rmodbus::server::ModbusFrame::new(
            self.unit,
            req.as_slice().try_into()?,
            rmodbus::ModbusProto::Rtu,
            &mut response,
        );
        handle_frame(&mut frame).await?;
        if frame.response_required {
            frame.finalize_response().map_err(Error::io)?;
            tokio::time::timeout(self.timeout, self.stream.write_all(&response)).await??;
        }
        Ok(())
    }

    async fn handle_ascii_frame(&mut self, initial_buf: [u8; 7]) -> EResult<()> {
        let len = rmodbus::guess_request_frame_len(&initial_buf, rmodbus::ModbusProto::Ascii)
            .map_err(Error::io)?;
        let mut req = initial_buf.to_vec();
        if len > 7 {
            let mut rest = vec![0u8; (len - 7) as usize];
            tokio::time::timeout(self.timeout, self.stream.read_exact(&mut rest)).await??;
            req.extend(rest);
        }
        let mut buf: rmodbus::ModbusFrameBuf = [0; 256];
        rmodbus::parse_ascii_frame(&req, req.len(), &mut buf, 0).map_err(Error::io)?;
        let mut response = Vec::new();
        let mut frame = rmodbus::server::ModbusFrame::new(
            self.unit,
            &buf,
            rmodbus::ModbusProto::Ascii,
            &mut response,
        );
        handle_frame(&mut frame).await?;
        if frame.response_required {
            frame.finalize_response().map_err(Error::io)?;
            let mut ascii_buf = Vec::new();
            rmodbus::generate_ascii_frame(&response, &mut ascii_buf).map_err(Error::io)?;
            tokio::time::timeout(self.timeout, self.stream.write_all(&ascii_buf)).await??;
        }
        Ok(())
    }
}
