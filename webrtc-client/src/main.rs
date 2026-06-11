//! WebRTC 客户端 — 内嵌 HTTP 服务器
//!
//! 托管 index.html 页面，手机/浏览器打开后直接连 BN Agent 信令服务器。
//! 页面里的 JS 自动处理 WebRTC 连接、音频捕获和播放。
//!
//! 用法：
//!   webrtc-client.exe
//!
//! 环境变量：
//!   HTTP_PORT       - HTTP 服务器端口（默认 8080）
//!   SIGNALING_URL   - 注入到页面的信令服务器地址（默认 ws://127.0.0.1:9876）
//!   ROOM_ID         - 注入到页面的房间号（默认 "default"）

use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

/// 编译时嵌入 index.html
const INDEX_HTML: &str = include_str!("../index.html");

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let http_port: u16 = std::env::var("HTTP_PORT")
        .ok().and_then(|s| s.parse().ok()).unwrap_or(8080);
    let signaling_url = std::env::var("SIGNALING_URL")
        .unwrap_or_else(|_| "ws://127.0.0.1:9876".into());
    let room_id = std::env::var("ROOM_ID")
        .unwrap_or_else(|_| "default".into());

    // 注入配置到页面
    let page = INDEX_HTML
        .replace("ws://127.0.0.1:9876", &signaling_url)
        .replace(">default<", &format!(">{}<", room_id));

    let addr = format!("0.0.0.0:{}", http_port);
    let listener = TcpListener::bind(&addr).await?;

    let local_ip = get_local_ip().unwrap_or_else(|| "127.0.0.1".to_string());
    println!("=== WebRTC 客户端 (HTTP) ===");
    println!("桌面访问: http://127.0.0.1:{}/", http_port);
    println!("手机访问: http://{}:{}/", local_ip, http_port);
    println!("信令服务器: {}", signaling_url);
    println!("房间号: {}", room_id);

    loop {
        let (mut socket, _) = listener.accept().await?;
        let page = page.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            let _ = socket.readable().await;
            let _ = socket.try_read(&mut buf);

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                page.len(),
                page
            );
            let _ = socket.write_all(response.as_bytes()).await;
        });
    }
}

fn get_local_ip() -> Option<String> {
    use std::net::UdpSocket;
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let addr = socket.local_addr().ok()?;
    Some(addr.ip().to_string())
}
