//! Транспорт до ретранслятора на Node: UDP или WebSocket поверх TLS.
//!
//! UDP — основной путь: у голоса минимальная задержка, а потерю пакета прикрывает кодек.
//! WebSocket на 443-м порту — запасной: снаружи он выглядит как обычная работа клиента
//! с Node и проходит там, где UDP закрыт или голосовой трафик режут. Содержимое кадров
//! одинаково и зашифровано клиентом заранее — транспорт видит только непрозрачные байты.

use std::{
    io::{self, Read, Write},
    net::{TcpStream, ToSocketAddrs, UdpSocket},
    sync::Arc,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use tungstenite::{Message, WebSocket, client::IntoClientRequest};

use crate::CoreError;

pub const JOIN: u8 = 0x01;
pub const DATA: u8 = 0x02;
pub const JOINED: u8 = 0x03;
pub const ROOM_BYTES: usize = 16;
pub const HEADER_BYTES: usize = 1 + ROOM_BYTES + 1;

/// Комната ретранслятора, как её выдал Node, и место в ней.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayTicket {
    pub room_id: String,
    pub token: String,
    #[serde(default)]
    pub udp_host: Option<String>,
    #[serde(default)]
    pub udp_port: u16,
    pub web_socket_url: String,
}

impl RelayTicket {
    pub fn room(&self) -> Result<[u8; ROOM_BYTES], CoreError> {
        URL_SAFE_NO_PAD
            .decode(&self.room_id)
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| CoreError::Crypto("Повреждена комната звонка".to_owned()))
    }

    pub fn token_bytes(&self) -> Result<[u8; 32], CoreError> {
        URL_SAFE_NO_PAD
            .decode(&self.token)
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| CoreError::Crypto("Поврежден пропуск звонка".to_owned()))
    }

    /// Ретранслятор приходит от собеседника: подключаемся только к TLS (или к локальному
    /// узлу в тестах), чтобы приглашение нельзя было использовать для открытого трафика.
    pub fn validate(&self) -> Result<(), CoreError> {
        self.room()?;
        self.token_bytes()?;
        let url = self.web_socket_url.as_str();
        let local = url.starts_with("ws://localhost") || url.starts_with("ws://127.0.0.1");
        if !(url.starts_with("wss://") || local) || url.len() > 300 {
            return Err(CoreError::Crypto("Небезопасный адрес ретранслятора".to_owned()));
        }
        if self.udp_host.as_ref().is_some_and(|host| host.len() > 253) {
            return Err(CoreError::Crypto("Повреждён адрес ретранслятора".to_owned()));
        }
        Ok(())
    }
}

/// Канал до ретранслятора.
pub trait Link: Send {
    fn send(&mut self, frame: &[u8]) -> io::Result<()>;
    /// Один кадр, если пришёл за `timeout`; `Ok(None)` — ничего не пришло.
    fn receive(&mut self, buffer: &mut Vec<u8>, timeout: Duration) -> io::Result<Option<usize>>;
    fn kind(&self) -> &'static str;
}

pub struct UdpLink {
    socket: UdpSocket,
}

impl UdpLink {
    pub fn connect(host: &str, port: u16) -> io::Result<Self> {
        let address = (host, port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "ретранслятор не найден"))?;
        let bind = if address.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" };
        let socket = UdpSocket::bind(bind)?;
        socket.connect(address)?;
        Ok(Self { socket })
    }
}

impl Link for UdpLink {
    fn send(&mut self, frame: &[u8]) -> io::Result<()> {
        self.socket.send(frame).map(|_| ())
    }

    fn receive(&mut self, buffer: &mut Vec<u8>, timeout: Duration) -> io::Result<Option<usize>> {
        buffer.resize(2048, 0);
        self.socket.set_read_timeout(Some(timeout.max(Duration::from_millis(1))))?;
        match self.socket.recv(buffer) {
            Ok(length) => Ok(Some(length)),
            Err(error) if is_timeout(&error) => Ok(None),
            // ICMP «порт недоступен» на connected-сокете — не повод рвать звонок.
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn kind(&self) -> &'static str {
        "udp"
    }
}

enum Stream {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Stream {
    fn tcp(&self) -> &TcpStream {
        match self {
            Self::Plain(stream) => stream,
            Self::Tls(stream) => &stream.sock,
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buffer),
            Self::Tls(stream) => stream.read(buffer),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.write(buffer),
            Self::Tls(stream) => stream.write(buffer),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(stream) => stream.flush(),
            Self::Tls(stream) => stream.flush(),
        }
    }
}

pub struct WebSocketLink {
    socket: WebSocket<Stream>,
}

impl WebSocketLink {
    pub fn connect(url: &str) -> io::Result<Self> {
        let other = |message: String| io::Error::other(message);
        let request = url.into_client_request().map_err(|error| other(error.to_string()))?;
        let uri = request.uri().clone();
        let host = uri.host().ok_or_else(|| other("в адресе нет хоста".to_owned()))?.to_owned();
        let secure = uri.scheme_str() == Some("wss");
        let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
        let address = (host.as_str(), port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| other("ретранслятор не найден".to_owned()))?;
        let tcp = TcpStream::connect_timeout(&address, Duration::from_secs(6))?;
        tcp.set_nodelay(true)?;
        tcp.set_read_timeout(Some(Duration::from_secs(8)))?;
        tcp.set_write_timeout(Some(Duration::from_secs(4)))?;
        let stream = if secure {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let config = rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .map_err(|error| other(error.to_string()))?
            .with_root_certificates(roots)
            .with_no_client_auth();
            let name = rustls::pki_types::ServerName::try_from(host.clone())
                .map_err(|error| other(error.to_string()))?;
            let connection = rustls::ClientConnection::new(Arc::new(config), name)
                .map_err(|error| other(error.to_string()))?;
            Stream::Tls(Box::new(rustls::StreamOwned::new(connection, tcp)))
        } else {
            Stream::Plain(tcp)
        };
        let (socket, _) = tungstenite::client(request, stream).map_err(|error| other(error.to_string()))?;
        Ok(Self { socket })
    }
}

impl Link for WebSocketLink {
    fn send(&mut self, frame: &[u8]) -> io::Result<()> {
        self.socket
            .send(Message::Binary(frame.to_vec().into()))
            .map_err(tungstenite_error)
    }

    fn receive(&mut self, buffer: &mut Vec<u8>, timeout: Duration) -> io::Result<Option<usize>> {
        self.socket
            .get_ref()
            .tcp()
            .set_read_timeout(Some(timeout.max(Duration::from_millis(1))))?;
        match self.socket.read() {
            Ok(Message::Binary(data)) => {
                buffer.clear();
                buffer.extend_from_slice(&data);
                Ok(Some(data.len()))
            }
            Ok(Message::Close(_)) => Err(io::Error::new(io::ErrorKind::ConnectionAborted, "ретранслятор закрыл канал")),
            Ok(_) => Ok(None),
            Err(tungstenite::Error::Io(error)) if is_timeout(&error) => Ok(None),
            Err(error) => Err(tungstenite_error(error)),
        }
    }

    fn kind(&self) -> &'static str {
        "tls"
    }
}

fn tungstenite_error(error: tungstenite::Error) -> io::Error {
    match error {
        tungstenite::Error::Io(error) => error,
        other => io::Error::other(other.to_string()),
    }
}

fn is_timeout(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut)
}

/// Кадр JOIN: открывает своё место в комнате и держит открытым NAT.
pub fn join_frame(room: &[u8; ROOM_BYTES], side: u8, token: &[u8; 32]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(HEADER_BYTES + 32);
    frame.push(JOIN);
    frame.extend_from_slice(room);
    frame.push(side);
    frame.extend_from_slice(token);
    frame
}

pub fn header(kind: u8, room: &[u8; ROOM_BYTES], side: u8) -> [u8; HEADER_BYTES] {
    let mut value = [0u8; HEADER_BYTES];
    value[0] = kind;
    value[1..1 + ROOM_BYTES].copy_from_slice(room);
    value[HEADER_BYTES - 1] = side;
    value
}
