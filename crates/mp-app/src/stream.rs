//! 给笔记内的视频播放器提供按范围读取的回环地址。媒体边读边解密，不产生明文临时文件。

use mp_core::vault::BlobHandle;
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 没有任何请求这么久之后，服务自己停下来。
const IDLE_SHUTDOWN: Duration = Duration::from_secs(30 * 60);
const ACCEPT_POLL: Duration = Duration::from_millis(20);
/// 每次读多少解密后的数据送给播放器。
const SEND_CHUNK: usize = 256 * 1024;

pub struct StreamServer {
    port: u16,
    token: String,
    stop: Arc<AtomicBool>,
}

impl StreamServer {
    /// 在本机回环地址上随机挑一个端口开始监听。
    pub fn start(blob: BlobHandle) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let token = mp_core::util::random_id().map_err(std::io::Error::other)?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread_token = token.clone();
        std::thread::spawn(move || serve(listener, blob, thread_token, thread_stop));
        Ok(Self { port, token, stop })
    }

    /// 某个媒体文件的播放地址。文件名留在网址末尾，播放器才知道该按什么格式解。
    pub fn url(&self, blob_id: &str, file_name: &str) -> String {
        let safe_name: String = file_name
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() || ch == '.' { ch } else { '_' })
            .collect();
        format!("http://127.0.0.1:{}/{}/{blob_id}/{safe_name}", self.port, self.token)
    }

    pub fn is_running(&self) -> bool {
        !self.stop.load(Ordering::Relaxed)
    }
}

impl Drop for StreamServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn serve(listener: TcpListener, blob: BlobHandle, token: String, stop: Arc<AtomicBool>) {
    let mut last_request = Instant::now();
    let mut requests: Vec<std::thread::JoinHandle<()>> = Vec::new();
    while !stop.load(Ordering::Relaxed) && last_request.elapsed() < IDLE_SHUTDOWN {
        // Seeking can start a new Range request before the decoder releases its previous connection.
        requests.retain(|request| !request.is_finished());
        if requests.len() >= 4 {
            std::thread::sleep(ACCEPT_POLL);
            continue;
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                last_request = Instant::now();
                let blob = blob.clone();
                let token = token.clone();
                let stop = Arc::clone(&stop);
                requests.push(std::thread::spawn(move || {
                    if stream.set_nonblocking(false).is_err() {
                        return;
                    }
                    let _ = stream.set_nodelay(true);
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                    handle(&mut stream, &blob, &token, &stop);
                    let _ = stream.shutdown(Shutdown::Write);
                }));
            }
            Err(error) if retry_accept(error.kind()) => {
                std::thread::sleep(ACCEPT_POLL);
            }
            Err(_) => break,
        }
    }
    stop.store(true, Ordering::Relaxed);
}

fn retry_accept(kind: ErrorKind) -> bool {
    // 播放器可以在服务器接收之前取消旧连接（Windows 会报告 ConnectionReset）。
    matches!(kind, ErrorKind::WouldBlock | ErrorKind::Interrupted | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted)
}

/// 读请求头，直到空行。请求头异常长就直接断开。
fn read_request_head(stream: &mut TcpStream, stop: &AtomicBool) -> Option<String> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        let count = stream.read(&mut buffer).ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            return Some(String::from_utf8_lossy(&bytes).to_string());
        }
        if bytes.len() > 16 * 1024 {
            return None;
        }
    }
}

/// 解析 `Range: bytes=a-b`，返回 (起点, 终点)，两端都包含。
fn parse_range(head: &str, total: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(line) = head.lines().find(|line| line.to_ascii_lowercase().starts_with("range:")) else {
        return Ok(None);
    };
    if total == 0 {
        return Err(());
    }
    let value = line.split_once(':').ok_or(())?.1.trim();
    let (start, end) = value.strip_prefix("bytes=").ok_or(())?.split_once('-').ok_or(())?;
    if start.trim().is_empty() {
        let suffix: u64 = end.trim().parse().map_err(|_| ())?;
        return if suffix == 0 { Err(()) } else { Ok(Some((total.saturating_sub(suffix), total - 1))) };
    }
    let start: u64 = start.trim().parse().map_err(|_| ())?;
    let end = match end.trim() {
        "" => total - 1,
        value => value.parse::<u64>().map_err(|_| ())?.min(total - 1),
    };
    if start <= end { Ok(Some((start, end))) } else { Err(()) }
}

fn handle(stream: &mut TcpStream, blob: &BlobHandle, token: &str, stop: &AtomicBool) {
    let Some(head) = read_request_head(stream, stop) else {
        return;
    };
    // 请求行长这样：GET /<令牌>/<媒体标识>/<文件名> HTTP/1.1
    let mut parts = head.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();
    let path = path.split(['?', '#']).next().unwrap_or("");
    let mut segments = path.trim_start_matches('/').split('/');
    let (got_token, blob_id, file_name) = (segments.next().unwrap_or(""), segments.next().unwrap_or(""), segments.next().unwrap_or(""));
    if !matches!(method, "GET" | "HEAD") || got_token != token {
        let _ = stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    }
    let Ok(mut reader) = blob.open(blob_id) else {
        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    };

    let total = reader.len();
    let (status, start, length) = match parse_range(&head, total) {
        Ok(Some((start, end))) => ("206 Partial Content", start, end - start + 1),
        Ok(None) => ("200 OK", 0, total),
        Err(()) => {
            let _ = write!(stream, "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return;
        }
    };
    let kind = mp_core::media::classify(file_name);
    let mime = kind.map(|kind| kind.mime(file_name)).unwrap_or("application/octet-stream");
    if reader.seek(SeekFrom::Start(start)).is_err() {
        return;
    }
    let mut head = format!(
        "HTTP/1.1 {status}\r\nAccept-Ranges: bytes\r\nContent-Type: {mime}\r\nContent-Length: {length}\r\nCache-Control: no-store\r\nConnection: close\r\n"
    );
    if status.starts_with("206") {
        head.push_str(&format!("Content-Range: bytes {start}-{}/{}\r\n", start + length - 1, total));
    }
    head.push_str("\r\n");
    if stream.write_all(head.as_bytes()).is_err() {
        return;
    }
    if method == "HEAD" {
        return;
    }

    // 边解密边送。播放器中途关掉（写失败）或服务要停，就到这里为止。
    let mut remaining = length;
    let mut buffer = vec![0u8; SEND_CHUNK];
    while remaining > 0 && !stop.load(Ordering::Relaxed) {
        let want = (buffer.len() as u64).min(remaining) as usize;
        let Ok(count) = reader.read(&mut buffer[..want]) else {
            break;
        };
        if count == 0 || stream.write_all(&buffer[..count]).is_err() {
            break;
        }
        remaining -= count as u64;
    }
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_retries_pending_and_client_errors_only() {
        for kind in [ErrorKind::WouldBlock, ErrorKind::Interrupted, ErrorKind::ConnectionReset, ErrorKind::ConnectionAborted] {
            assert!(retry_accept(kind), "{kind:?}");
        }
        for kind in [ErrorKind::PermissionDenied, ErrorKind::NotConnected, ErrorKind::Other] {
            assert!(!retry_accept(kind), "{kind:?}");
        }
        #[cfg(windows)]
        assert!(retry_accept(std::io::Error::from_raw_os_error(10054).kind()));
    }

    #[test]
    fn decoder_can_seek_while_its_previous_connection_is_still_open() {
        let path = std::env::temp_dir().join(format!("moss-stream-{}", mp_core::util::random_id().unwrap()));
        let (vault, _) = mp_core::vault::Vault::create(&path, "stream-test").unwrap();
        let (id, _) = vault.import_blob(&b"frame-zero-frame-one"[..]).unwrap();
        let server = StreamServer::start(vault.blob_handle()).unwrap();
        let address = ("127.0.0.1", server.port);
        let mut first = TcpStream::connect(address).unwrap();
        first.write_all(b"GET /pending HTTP/1.1\r\nX-Pending: ").unwrap();
        let (finish, done) = std::sync::mpsc::channel::<()>();
        let stalled = std::thread::spawn(move || {
            // Keep this incomplete header active, including on slow CI machines.
            while matches!(done.recv_timeout(Duration::from_millis(100)), Err(std::sync::mpsc::RecvTimeoutError::Timeout)) {
                if first.write_all(b".").is_err() { break; }
            }
        });
        let mut seek = TcpStream::connect(address).unwrap();
        seek.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
        write!(seek, "GET /{}/{id}/clip.mp4 HTTP/1.1\r\nRange: bytes=11-\r\n\r\n", server.token).unwrap();
        let mut response = std::io::BufReader::new(seek);
        use std::io::BufRead;
        let mut header = String::new();
        while !header.ends_with("\r\n\r\n") {
            assert_ne!(response.read_line(&mut header).unwrap(), 0);
        }
        assert!(header.starts_with("HTTP/1.1 206"));
        let mut body = [0; 9];
        response.read_exact(&mut body).unwrap();
        assert_eq!(&body, b"frame-one");
        drop(finish);
        stalled.join().unwrap();
        drop(server);
        drop(vault);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn ranges_handle_suffixes_empty_files_and_unsatisfied_offsets() {
        assert_eq!(parse_range("GET / HTTP/1.1\r\n", 10), Ok(None));
        assert_eq!(parse_range("Range: bytes=2-99", 10), Ok(Some((2, 9))));
        assert_eq!(parse_range("Range: bytes=-3", 10), Ok(Some((7, 9))));
        assert_eq!(parse_range("Range: bytes=-30", 10), Ok(Some((0, 9))));
        assert_eq!(parse_range("Range: bytes=0-", 0), Err(()));
        assert_eq!(parse_range("Range: bytes=10-", 10), Err(()));
        assert_eq!(parse_range("Range: bytes=-0", 10), Err(()));
    }
}
