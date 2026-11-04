use crate::security::valid_id;
use serde_json::{json, Value};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
const TIMEOUT: Duration = Duration::from_secs(2);
const MAX_FRAME: usize = 65536;
pub struct DiscordRpcSession {
    stream: UnixStream,
    pending: Vec<u8>,
}
fn socket_paths() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for key in ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"] {
        if let Some(d) = std::env::var_os(key)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
        {
            if key == "XDG_RUNTIME_DIR" {
                dirs.push(d.join("app/com.discordapp.Discord"));
                dirs.push(d.join("discord-ipc"));
            }
            dirs.push(d);
        }
    }
    dirs.push(PathBuf::from("/tmp"));
    dirs.into_iter()
        .flat_map(|d| (0..10).map(move |n| d.join(format!("discord-ipc-{n}"))))
        .collect()
}
impl DiscordRpcSession {
    pub async fn connect(id: &str) -> Result<Self, String> {
        if !valid_id(id) {
            return Err("Invalid application ID".into());
        }
        for path in socket_paths() {
            let Ok(m) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            // Never follow socket symlinks or connect to another user's server.
            if !m.file_type().is_socket() || m.uid() != unsafe { libc::geteuid() } {
                continue;
            }
            let Ok(Ok(stream)) = tokio::time::timeout(TIMEOUT, UnixStream::connect(&path)).await
            else {
                continue;
            };
            if stream.peer_cred().map_err(|e| e.to_string())?.uid() != unsafe { libc::geteuid() } {
                continue;
            }
            let mut s = Self {
                stream,
                pending: Vec::new(),
            };
            let result: Result<(), String> = tokio::time::timeout(TIMEOUT, async {
                s.write(0, &json!({"v":1,"client_id":id})).await?;
                let v = s.frame().await?;
                if v["evt"] != "READY" || v["cmd"] != "DISPATCH" {
                    return Err("Discord did not confirm READY".into());
                }
                Ok(())
            })
            .await
            .map_err(|_| "Discord handshake timed out".to_string())?;
            result?;
            return Ok(s);
        }
        Err("Discord IPC недоступен. Запустите Discord Desktop; для Flatpak проверьте доступ к IPC.".into())
    }
    async fn write(&mut self, op: u32, v: &Value) -> Result<(), String> {
        let data = serde_json::to_vec(v).map_err(|e| e.to_string())?;
        self.write_raw(op, &data).await
    }
    async fn write_raw(&mut self, op: u32, data: &[u8]) -> Result<(), String> {
        if data.len() > MAX_FRAME {
            return Err("Oversized IPC payload".into());
        }
        let mut bytes = Vec::with_capacity(data.len() + 8);
        bytes.extend_from_slice(&op.to_le_bytes());
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(data);
        tokio::time::timeout(TIMEOUT, self.stream.write_all(&bytes))
            .await
            .map_err(|_| "IPC write timed out".to_string())?
            .map_err(|e| e.to_string())
    }
    async fn frame(&mut self) -> Result<Value, String> {
        loop {
            let mut header = [0u8; 8];
            self.stream
                .read_exact(&mut header)
                .await
                .map_err(|e| e.to_string())?;
            let op = u32::from_le_bytes(header[..4].try_into().unwrap());
            let len = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
            if len > MAX_FRAME {
                return Err("Oversized IPC frame".into());
            }
            let mut body = vec![0; len];
            self.stream
                .read_exact(&mut body)
                .await
                .map_err(|e| e.to_string())?;
            if op == 3 {
                self.write_raw(4, &body).await?;
                continue;
            }
            if op != 1 {
                return Err(format!("Unexpected Discord IPC opcode {op}"));
            }
            return serde_json::from_slice(&body).map_err(|e| e.to_string());
        }
    }
    pub async fn set_activity(
        &mut self,
        _name: &str,
        start_ms: u64,
        pid: u32,
    ) -> Result<(), String> {
        let nonce = format!("{pid}-{start_ms}");
        self.write(1,&json!({"cmd":"SET_ACTIVITY","args":{"pid":pid,"activity":{"timestamps":{"start":start_ms},"instance":true}},"nonce":nonce})).await?;
        tokio::time::timeout(TIMEOUT, async {
            loop {
                let v = self.frame().await?;
                if v["nonce"] != nonce {
                    continue;
                }
                return confirm_activity(&v);
            }
        })
        .await
        .map_err(|_| "Discord activity acknowledgement timed out".to_string())?
    }
    /// Consume heartbeats and detect disconnects during long sessions without losing partial frames.
    pub async fn poll_connection(&mut self) -> Result<(), String> {
        let mut chunk = [0u8; 4096];
        loop {
            match self.stream.try_read(&mut chunk) {
                Ok(0) => return Err("Discord IPC closed".into()),
                Ok(n) => {
                    self.pending.extend_from_slice(&chunk[..n]);
                    if self.pending.len() > MAX_FRAME + 8 {
                        return Err("Oversized buffered IPC data".into());
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.to_string()),
            }
        }
        while self.pending.len() >= 8 {
            let op = u32::from_le_bytes(self.pending[..4].try_into().unwrap());
            let len = u32::from_le_bytes(self.pending[4..8].try_into().unwrap()) as usize;
            if len > MAX_FRAME {
                return Err("Oversized IPC frame".into());
            }
            if self.pending.len() < len + 8 {
                break;
            }
            let body = self.pending[8..8 + len].to_vec();
            self.pending.drain(..8 + len);
            match op {
                3 => self.write_raw(4, &body).await?,
                4 => (),
                1 => {
                    let v: Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
                    if v["evt"] == "ERROR" {
                        return Err("Discord reported an IPC error".into());
                    }
                }
                _ => return Err("Discord IPC closed or invalid opcode".into()),
            }
        }
        Ok(())
    }
    pub async fn clear_activity(&mut self, pid: u32) -> Result<(), String> {
        self.write(
            1,
            &json!({"cmd":"SET_ACTIVITY","args":{"pid":pid,"activity":null},"nonce":"clear"}),
        )
        .await
    }
}
fn confirm_activity(v: &Value) -> Result<(), String> {
    if v["evt"] == "ERROR" {
        return Err(format!(
            "Discord rejected activity: {}",
            v["data"]["message"]
        ));
    }
    if v["cmd"] != "SET_ACTIVITY" || !v["data"].is_object() {
        return Err("Invalid activity acknowledgement".into());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn error_is_not_success() {
        assert!(confirm_activity(
            &json!({"cmd":"SET_ACTIVITY","evt":"ERROR","data":{"message":"denied"}})
        )
        .is_err());
        assert!(confirm_activity(&json!({"cmd":"DISPATCH"})).is_err());
    }
    #[tokio::test]
    async fn rejects_oversized_frame() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let mut s = DiscordRpcSession {
            stream: client,
            pending: Vec::new(),
        };
        let task = tokio::spawn(async move {
            server.write_all(&[1, 0, 0, 0, 1, 0, 1, 0]).await.unwrap();
        });
        assert!(s.frame().await.unwrap_err().contains("Oversized"));
        task.await.unwrap();
    }
    #[tokio::test]
    async fn ping_pong_and_response() {
        let (c, mut s) = UnixStream::pair().unwrap();
        let mut session = DiscordRpcSession {
            stream: c,
            pending: Vec::new(),
        };
        let t = tokio::spawn(async move {
            for (op, body) in [
                (3u32, b"hello".as_slice()),
                (1, b"{\"evt\":\"READY\"}".as_slice()),
            ] {
                s.write_all(&op.to_le_bytes()).await.unwrap();
                s.write_all(&(body.len() as u32).to_le_bytes())
                    .await
                    .unwrap();
                s.write_all(body).await.unwrap();
            }
            let mut pong = [0; 13];
            s.read_exact(&mut pong).await.unwrap();
            assert_eq!(&pong[..4], &4u32.to_le_bytes());
            assert_eq!(&pong[8..], b"hello");
        });
        assert_eq!(session.frame().await.unwrap()["evt"], "READY");
        t.await.unwrap();
    }
    #[tokio::test]
    async fn timeout_is_not_success() {
        let (c, _s) = UnixStream::pair().unwrap();
        let mut session = DiscordRpcSession {
            stream: c,
            pending: Vec::new(),
        };
        assert!(session
            .set_activity("x", 1, 1)
            .await
            .unwrap_err()
            .contains("timed out"));
    }
    #[tokio::test]
    async fn detects_closed_connection() {
        let (client, server) = UnixStream::pair().unwrap();
        let mut session = DiscordRpcSession {
            stream: client,
            pending: Vec::new(),
        };
        drop(server);
        session.stream.readable().await.unwrap();
        assert!(session.poll_connection().await.is_err());
    }
    #[tokio::test]
    async fn retains_partial_heartbeat() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let mut session = DiscordRpcSession {
            stream: client,
            pending: Vec::new(),
        };
        server.write_all(&[3, 0, 0, 0]).await.unwrap();
        session.stream.readable().await.unwrap();
        session.poll_connection().await.unwrap();
        assert_eq!(session.pending.len(), 4);
        server.write_all(&[2, 0, 0, 0, 111, 107]).await.unwrap();
        session.stream.readable().await.unwrap();
        session.poll_connection().await.unwrap();
        assert!(session.pending.is_empty());
        let mut pong = [0; 10];
        server.read_exact(&mut pong).await.unwrap();
        assert_eq!(pong, [4, 0, 0, 0, 2, 0, 0, 0, 111, 107]);
    }
}
