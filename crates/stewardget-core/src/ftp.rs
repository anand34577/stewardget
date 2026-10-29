//! A small FTP client: enough to probe a file and stream a byte range of it.
//!
//! It uses passive mode and `REST`, so the segmented, resumable engine works
//! on `ftp://` links exactly as it does on HTTP: several connections each
//! fetch their own range. Credentials come from the link
//! (`ftp://user:password@host/file`); without them it logs in anonymously.
//! The connection is not encrypted (plain FTP), so passwords travel in the
//! clear; use it on trusted networks only.

use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;

use bytes::Bytes;
use futures_util::Stream;
use futures_util::stream;
use percent_encoding::percent_decode_str;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpSocket, TcpStream};
use url::Url;

use crate::Error;
use crate::probe::Probe;

/// Chunks of a transfer, in order.
pub(crate) type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, Error>> + Send>>;

struct Target {
    host: String,
    port: u16,
    user: String,
    pass: String,
    path: String,
}

fn target(url: &Url) -> Result<Target, Error> {
    let host =
        url.host_str().filter(|h| !h.is_empty()).ok_or_else(|| Error::InvalidUrl("ftp link without a host".into()))?;
    let decode = |s: &str| percent_decode_str(s).decode_utf8_lossy().into_owned();
    let user = if url.username().is_empty() { "anonymous".to_owned() } else { decode(url.username()) };
    let pass = match url.password() {
        Some(p) => decode(p),
        None => "stewardget@localhost".to_owned(),
    };
    Ok(Target {
        host: host.trim_matches(['[', ']']).to_owned(),
        port: url.port().unwrap_or(21),
        user,
        pass,
        path: decode(url.path()),
    })
}

/// The control connection.
struct Control {
    io: BufReader<TcpStream>,
    peer: IpAddr,
    local: Option<IpAddr>,
}

fn io_err(e: std::io::Error) -> Error {
    Error::Network(e.to_string())
}

/// A server's answer: 4xx may pass by itself, 5xx won't.
fn refused(code: u16, text: &str) -> Error {
    let msg = format!("the FTP server answered {code} {}", text.trim());
    if (400..500).contains(&code) { Error::Network(msg) } else { Error::Unsupported(msg) }
}

async fn open_socket(host: &str, port: u16, local: Option<IpAddr>) -> Result<TcpStream, Error> {
    let mut addrs = tokio::net::lookup_host((host, port)).await.map_err(io_err)?.collect::<Vec<SocketAddr>>();
    // A socket bound to an IPv4 adapter can only reach IPv4 servers.
    if let Some(local) = local {
        addrs.retain(|a| a.is_ipv4() == local.is_ipv4());
    }
    let addr = addrs.first().copied().ok_or_else(|| Error::Network(format!("could not find {host}")))?;
    match local {
        Some(ip) => {
            let socket = if ip.is_ipv4() { TcpSocket::new_v4() } else { TcpSocket::new_v6() }.map_err(io_err)?;
            socket.bind(SocketAddr::new(ip, 0)).map_err(io_err)?;
            socket.connect(addr).await.map_err(io_err)
        }
        None => TcpStream::connect(addr).await.map_err(io_err),
    }
}

impl Control {
    async fn connect(t: &Target, local: Option<IpAddr>) -> Result<Self, Error> {
        let stream = open_socket(&t.host, t.port, local).await?;
        let peer = stream.peer_addr().map_err(io_err)?.ip();
        let _ = stream.set_nodelay(true);
        let mut c = Self { io: BufReader::new(stream), peer, local };
        let (code, text) = c.reply().await?;
        if code != 220 {
            return Err(refused(code, &text));
        }
        let (code, text) = c.cmd(&format!("USER {}", t.user)).await?;
        match code {
            230 => {}
            331 | 332 => {
                let (code, text) = c.cmd(&format!("PASS {}", t.pass)).await?;
                if code != 230 {
                    return Err(if code == 530 {
                        Error::Unsupported("the FTP server refused the login".into())
                    } else {
                        refused(code, &text)
                    });
                }
            }
            _ => return Err(refused(code, &text)),
        }
        let (code, text) = c.cmd("TYPE I").await?;
        if code != 200 {
            return Err(refused(code, &text));
        }
        Ok(c)
    }

    /// One reply; a multi-line reply (`123-…` until `123 …`) counts as one.
    async fn reply(&mut self) -> Result<(u16, String), Error> {
        let mut first = String::new();
        let mut code = 0u16;
        loop {
            let mut line = String::new();
            let n = self.io.read_line(&mut line).await.map_err(io_err)?;
            if n == 0 {
                return Err(Error::Network("the FTP server closed the connection".into()));
            }
            let line = line.trim_end();
            if code == 0 {
                code = line
                    .get(..3)
                    .and_then(|c| c.parse().ok())
                    .ok_or_else(|| Error::Network(format!("odd FTP reply: {line}")))?;
                first = line.get(4..).unwrap_or("").to_owned();
                if line.as_bytes().get(3) != Some(&b'-') {
                    return Ok((code, first));
                }
            } else if line.len() >= 4 && line.starts_with(&code.to_string()) && line.as_bytes()[3] == b' ' {
                return Ok((code, first));
            }
        }
    }

    async fn cmd(&mut self, line: &str) -> Result<(u16, String), Error> {
        self.io.get_mut().write_all(format!("{line}\r\n").as_bytes()).await.map_err(io_err)?;
        self.reply().await
    }

    /// Opens a passive data connection.
    async fn passive(&mut self) -> Result<TcpStream, Error> {
        let (code, text) = self.cmd("PASV").await?;
        if code != 227 {
            return Err(refused(code, &text));
        }
        let port = parse_pasv_port(&text).ok_or_else(|| Error::Network(format!("odd PASV reply: {text}")))?;
        // The address in the reply is often a private one behind NAT: the
        // server we already reached is the right one.
        let stream = match self.local {
            Some(ip) => {
                let socket = TcpSocket::new_v4().map_err(io_err)?;
                socket.bind(SocketAddr::new(ip, 0)).map_err(io_err)?;
                socket.connect(SocketAddr::new(self.peer, port)).await.map_err(io_err)?
            }
            None => TcpStream::connect(SocketAddr::new(self.peer, port)).await.map_err(io_err)?,
        };
        Ok(stream)
    }

    async fn quit(mut self) {
        let _ = self.io.get_mut().write_all(b"QUIT\r\n").await;
    }
}

/// The port from `227 Entering Passive Mode (h1,h2,h3,h4,p1,p2)`.
fn parse_pasv_port(text: &str) -> Option<u16> {
    let inner = text.split_once('(')?.1.split_once(')')?.0;
    let parts: Vec<u16> = inner.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    (parts.len() == 6 && parts[4] < 256 && parts[5] < 256).then(|| parts[4] * 256 + parts[5])
}

/// Size, resume support and name of a file, like the HTTP probe.
pub(crate) async fn probe(url: &Url) -> Result<Probe, Error> {
    let t = target(url)?;
    let mut c = Control::connect(&t, None).await?;
    let (code, text) = c.cmd(&format!("SIZE {}", t.path)).await?;
    let total = match code {
        213 => text.trim().parse::<u64>().ok(),
        550 => return Err(Error::Unsupported("the FTP server has no such file".into())),
        // Servers that don't know SIZE still serve the file, without a known length.
        500 | 502 => None,
        _ => return Err(refused(code, &text)),
    };
    let last_modified = match c.cmd(&format!("MDTM {}", t.path)).await {
        Ok((213, stamp)) => Some(format!("mdtm {}", stamp.trim())),
        _ => None,
    };
    // `REST 0` is what resuming needs; a server that refuses it can't resume.
    let ranges = matches!(c.cmd("REST 0").await, Ok((350, _))) && total.is_some();
    c.quit().await;
    let name = t.path.rsplit('/').find(|s| !s.is_empty()).map(str::to_owned);
    Ok(Probe {
        final_url: url.clone(),
        total,
        ranges,
        etag: None,
        last_modified,
        file_name: name,
        content_type: None,
        stream_ext: None,
    })
}

/// Streams the file from `offset` to its end (the caller stops reading at the
/// end of its range and drops the stream, which closes the transfer).
pub(crate) async fn open(url: &Url, offset: u64, local: Option<IpAddr>) -> Result<ByteStream, Error> {
    let t = target(url)?;
    let mut c = Control::connect(&t, local).await?;
    let data = c.passive().await?;
    if offset > 0 {
        let (code, text) = c.cmd(&format!("REST {offset}")).await?;
        if code != 350 {
            return Err(if code / 100 == 5 { Error::ResumeUnsupported } else { refused(code, &text) });
        }
    }
    let (code, text) = c.cmd(&format!("RETR {}", t.path)).await?;
    if !matches!(code, 125 | 150) {
        return Err(match code {
            550 => Error::Unsupported("the FTP server has no such file".into()),
            _ => refused(code, &text),
        });
    }
    // The control connection stays open until the transfer is done.
    Ok(Box::pin(stream::unfold((data, Some(c)), |(mut data, control)| async move {
        let mut buf = vec![0u8; 64 * 1024];
        match data.read(&mut buf).await {
            Ok(0) => {
                if let Some(c) = control {
                    c.quit().await;
                }
                None
            }
            Ok(n) => {
                buf.truncate(n);
                Some((Ok(Bytes::from(buf)), (data, control)))
            }
            Err(e) => Some((Err(io_err(e)), (data, None))),
        }
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use tokio::net::TcpListener;

    #[test]
    fn passive_replies() {
        assert_eq!(parse_pasv_port("Entering Passive Mode (127,0,0,1,195,80)."), Some(195 * 256 + 80));
        assert_eq!(parse_pasv_port("Entering Passive Mode"), None);
        assert_eq!(parse_pasv_port("(1,2,3,4,300,1)"), None);
    }

    #[test]
    fn credentials_and_paths_come_from_the_link() {
        let t = target(&Url::parse("ftp://me:p%40ss@host.test:2121/a%20b/file.bin").unwrap()).unwrap();
        assert_eq!((t.user.as_str(), t.pass.as_str(), t.port, t.path.as_str()), ("me", "p@ss", 2121, "/a b/file.bin"));
        let anon = target(&Url::parse("ftp://host.test/x").unwrap()).unwrap();
        assert_eq!(anon.user, "anonymous");
    }

    /// A tiny FTP server holding one file.
    async fn serve(data: Vec<u8>, with_size: bool) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else { return };
                let data = data.clone();
                tokio::spawn(async move {
                    let mut io = BufReader::new(sock);
                    let mut pasv: Option<TcpListener> = None;
                    let mut rest = 0usize;
                    io.get_mut().write_all(b"220 hello\r\n").await.unwrap();
                    loop {
                        let mut line = String::new();
                        if io.read_line(&mut line).await.unwrap_or(0) == 0 {
                            return;
                        }
                        let (cmd, arg) = line.trim().split_once(' ').unwrap_or((line.trim(), ""));
                        let out: String = match cmd {
                            "USER" => "331 password please\r\n".into(),
                            "PASS" => "230 in\r\n".into(),
                            "TYPE" => "200 binary\r\n".into(),
                            "SIZE" if with_size => format!("213 {}\r\n", data.len()),
                            "SIZE" => "502 no\r\n".into(),
                            "MDTM" => "213 20250102030405\r\n".into(),
                            "REST" => {
                                rest = arg.parse().unwrap_or(0);
                                "350 ok\r\n".into()
                            }
                            "PASV" => {
                                let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
                                let p = l.local_addr().unwrap().port();
                                pasv = Some(l);
                                format!("227 Entering Passive Mode (127,0,0,1,{},{})\r\n", p / 256, p % 256)
                            }
                            "RETR" => {
                                io.get_mut().write_all(b"150 here it comes\r\n").await.unwrap();
                                let (mut d, _) = pasv.take().unwrap().accept().await.unwrap();
                                let _ = d.write_all(&data[rest.min(data.len())..]).await;
                                drop(d);
                                "226 done\r\n".into()
                            }
                            "QUIT" => return,
                            _ => "500 what\r\n".into(),
                        };
                        io.get_mut().write_all(out.as_bytes()).await.unwrap();
                    }
                });
            }
        });
        port
    }

    #[tokio::test]
    async fn probes_and_streams_from_an_offset() {
        let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let port = serve(data.clone(), true).await;
        let url = Url::parse(&format!("ftp://127.0.0.1:{port}/pub/file%20one.bin")).unwrap();
        let p = probe(&url).await.unwrap();
        assert_eq!(p.total, Some(200_000));
        assert!(p.ranges);
        assert_eq!(p.file_name.as_deref(), Some("file one.bin"));
        assert_eq!(p.last_modified.as_deref(), Some("mdtm 20250102030405"));

        let mut s = open(&url, 150_000, None).await.unwrap();
        let mut got = Vec::new();
        while let Some(chunk) = s.next().await {
            got.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(got, data[150_000..]);
    }

    #[tokio::test]
    async fn a_server_without_size_cannot_resume() {
        let port = serve(vec![1; 10], false).await;
        let p = probe(&Url::parse(&format!("ftp://127.0.0.1:{port}/x")).unwrap()).await.unwrap();
        assert_eq!(p.total, None);
        assert!(!p.ranges);
    }
}
