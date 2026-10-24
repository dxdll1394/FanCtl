//! Synchronous unix-socket client used by the UI and the CLI.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::protocol::{Request, Response};

#[derive(Debug)]
pub struct Client {
    socket: PathBuf,
    timeout: Duration,
}

#[derive(Debug)]
pub enum ClientError {
    NotRunning(PathBuf),
    Io(std::io::Error),
    Protocol(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::NotRunning(p) => write!(
                f,
                "后台服务未运行（找不到 {}）。先执行 `sudo fanctl daemon install` 或 `sudo fanctl daemon run`",
                p.display()
            ),
            ClientError::Io(e) => write!(f, "与后台服务通信失败：{e}"),
            ClientError::Protocol(m) => write!(f, "后台服务返回了无法解析的数据：{m}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<std::io::Error> for ClientError {
    fn from(e: std::io::Error) -> Self {
        ClientError::Io(e)
    }
}

impl Client {
    pub fn new(socket: impl AsRef<Path>, timeout: Duration) -> Self {
        Self { socket: socket.as_ref().to_path_buf(), timeout }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    pub fn call(&self, req: &Request) -> Result<Response, ClientError> {
        if !self.socket.exists() {
            return Err(ClientError::NotRunning(self.socket.clone()));
        }
        let mut stream = UnixStream::connect(&self.socket)?;
        stream.set_read_timeout(Some(self.timeout))?;
        stream.set_write_timeout(Some(self.timeout))?;
        let mut payload =
            serde_json::to_vec(req).map_err(|e| ClientError::Protocol(e.to_string()))?;
        payload.push(b'\n');
        stream.write_all(&payload)?;
        stream.flush()?;
        let mut line = String::new();
        let mut reader = BufReader::new(stream);
        reader.read_line(&mut line)?;
        if line.trim().is_empty() {
            return Err(ClientError::Protocol("空响应".into()));
        }
        serde_json::from_str(&line).map_err(|e| ClientError::Protocol(e.to_string()))
    }

    /// Convenience wrapper returning the snapshot or a readable error.
    pub fn snapshot(&self) -> Result<crate::protocol::Snapshot, ClientError> {
        let resp = self.call(&Request::Snapshot)?;
        resp.snapshot.ok_or_else(|| {
            ClientError::Protocol(resp.error.unwrap_or_else(|| "响应里没有快照".into()))
        })
    }
}
