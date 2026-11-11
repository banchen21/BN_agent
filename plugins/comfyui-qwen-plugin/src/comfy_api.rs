//! ComfyUI HTTP API 客户端。
//!
//! 只依赖 `/upload/image`、`/prompt`、`/queue`、`/history/{id}`、`/view` 五个标准接口，
//! 不要求 ComfyUI 与宿主共享文件系统（输出通过 `/view` 下载）。

use reqwest::{multipart, Client};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Clone, Debug, Default)]
pub struct QueueSnapshot {
    pub running: usize,
    pub pending: usize,
}

impl QueueSnapshot {
    pub fn total(&self) -> usize {
        self.running + self.pending
    }
}

#[derive(Clone, Debug)]
pub struct OutputRef {
    pub filename: String,
    pub subfolder: String,
    pub file_type: String,
}

#[derive(Clone, Debug)]
pub struct FetchedImage {
    pub prompt_id: String,
    pub bytes: Vec<u8>,
    #[allow(dead_code)]
    pub output: OutputRef,
    /// 提交时检测到前方排队任务而追加的等待秒数。
    pub queue_extra_secs: u64,
}

#[derive(Debug)]
pub struct ComfyClient {
    base_url: String,
    http: Client,
    client_id: String,
    timeout: Duration,
}

impl ComfyClient {
    pub fn new(server_url: &str, timeout_secs: u64) -> Result<Self, String> {
        let trimmed = server_url.trim().trim_end_matches('/');
        let url = reqwest::Url::parse(trimmed).map_err(|e| {
            format!(
                "ComfyUI 地址无效（{}）：{}；需以 http:// 或 https:// 开头",
                trimmed, e
            )
        })?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(format!(
                "ComfyUI 地址必须以 http:// 或 https:// 开头（收到 {}）",
                trimmed
            ));
        }
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| format!("创建 HTTP 客户端失败：{}", e))?;
        Ok(Self {
            base_url: trimmed.to_string(),
            http,
            client_id: Uuid::new_v4().to_string(),
            timeout: Duration::from_secs(timeout_secs.max(1)),
        })
    }

    /// 上传一张参考图到 ComfyUI input 目录，返回 LoadImage 可用的文件名。
    pub async fn upload_image(&self, filename: &str, bytes: Vec<u8>) -> Result<String, String> {
        let part = multipart::Part::bytes(bytes)
            .file_name(filename.to_string())
            .mime_str("application/octet-stream")
            .map_err(|e| format!("构造上传请求失败：{}", e))?;
        let form = multipart::Form::new().part("image", part);

        let response = self
            .http
            .post(format!("{}/upload/image", self.base_url))
            .multipart(form)
            .send()
            .await
            .map_err(|e| format!("上传参考图失败（无法连接 ComfyUI）：{}", e))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!(
                "上传参考图失败（HTTP {}）：{}",
                status.as_u16(),
                truncate(&text, 300)
            ));
        }
        let name = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|value| {
                value
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| filename.to_string());
        Ok(name)
    }

    /// 提交工作流，返回 prompt_id。
    pub async fn queue_prompt(&self, workflow: &Value) -> Result<String, String> {
        let body = json!({
            "prompt": workflow,
            "client_id": self.client_id,
        });
        let response = self
            .http
            .post(format!("{}/prompt", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("提交生成任务失败（无法连接 ComfyUI）：{}", e))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!(
                "提交生成任务失败（HTTP {}）：{}",
                status.as_u16(),
                extract_comfy_error(&text)
            ));
        }
        let value: Value = serde_json::from_str(&text)
            .map_err(|e| format!("解析 ComfyUI 提交响应失败：{}", e))?;
        value
            .get("prompt_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("ComfyUI 未返回 prompt_id：{}", truncate(&text, 300)))
    }

    /// 查询队列状态（失败时返回空快照，不影响主流程）。
    pub async fn queue_snapshot(&self) -> QueueSnapshot {
        let Ok(response) = self
            .http
            .get(format!("{}/queue", self.base_url))
            .send()
            .await
        else {
            return QueueSnapshot::default();
        };
        if !response.status().is_success() {
            return QueueSnapshot::default();
        }
        let Ok(value) = response.json::<Value>().await else {
            return QueueSnapshot::default();
        };
        let count = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0)
        };
        QueueSnapshot {
            running: count("queue_running"),
            pending: count("queue_pending"),
        }
    }

    /// 等待任务完成并下载第一张输出图。
    pub async fn queue_and_wait_image(
        &self,
        workflow: &Value,
        output_node: Option<&str>,
    ) -> Result<FetchedImage, String> {
        let prompt_id = self.queue_prompt(workflow).await?;

        // 提交后估算前方排队量，按每个任务 60s 追加等待上限（最多 10 分钟）。
        tokio::time::sleep(Duration::from_millis(250)).await;
        let snapshot = self.queue_snapshot().await;
        let tasks_ahead = snapshot.total().saturating_sub(1);
        let queue_extra_secs = (tasks_ahead as u64 * 60).min(600);

        let output = self
            .wait_output(
                &prompt_id,
                output_node,
                self.timeout + Duration::from_secs(queue_extra_secs),
            )
            .await?;
        let bytes = self.download(&output).await?;
        Ok(FetchedImage {
            prompt_id,
            bytes,
            output,
            queue_extra_secs,
        })
    }

    async fn wait_output(
        &self,
        prompt_id: &str,
        output_node: Option<&str>,
        total_timeout: Duration,
    ) -> Result<OutputRef, String> {
        let deadline = Instant::now() + total_timeout;
        loop {
            if Instant::now() >= deadline {
                return Err(format!(
                    "等待生成结果超时（{} 秒）",
                    total_timeout.as_secs()
                ));
            }

            let response = self
                .http
                .get(format!("{}/history/{}", self.base_url, prompt_id))
                .send()
                .await
                .map_err(|e| format!("查询生成状态失败：{}", e))?;
            if response.status().is_success() {
                let value: Value = response
                    .json()
                    .await
                    .map_err(|e| format!("解析生成状态失败：{}", e))?;
                if let Some(entry) = value.get(prompt_id) {
                    let status = entry.get("status");
                    let status_str = status
                        .and_then(|s| s.get("status_str"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if status_str == "error" {
                        return Err(format!(
                            "ComfyUI 执行失败：{}",
                            history_error_message(status)
                        ));
                    }
                    if let Some(output) = select_output(entry.get("outputs"), output_node) {
                        return Ok(output);
                    }
                    let completed = status
                        .and_then(|s| s.get("completed"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if completed || status_str == "success" {
                        return Err("任务已完成但未找到输出图片".to_string());
                    }
                }
            } else if response.status().as_u16() == 404 {
                return Err(format!("ComfyUI 找不到任务 {}", prompt_id));
            }

            tokio::time::sleep(Duration::from_millis(700)).await;
        }
    }

    async fn download(&self, output: &OutputRef) -> Result<Vec<u8>, String> {
        let response = self
            .http
            .get(format!("{}/view", self.base_url))
            .query(&[
                ("filename", output.filename.as_str()),
                ("subfolder", output.subfolder.as_str()),
                ("type", output.file_type.as_str()),
            ])
            .send()
            .await
            .map_err(|e| format!("下载生成图片失败：{}", e))?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!(
                "下载生成图片失败（HTTP {}）：{}",
                status.as_u16(),
                truncate(&text, 300)
            ));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("读取生成图片失败：{}", e))?;
        if bytes.is_empty() {
            return Err("ComfyUI 返回了空图片".to_string());
        }
        Ok(bytes.to_vec())
    }
}

fn select_output(outputs: Option<&Value>, preferred_node: Option<&str>) -> Option<OutputRef> {
    let outputs = outputs?.as_object()?;

    let pick = |node_output: &Value| -> Option<OutputRef> {
        let images = node_output.get("images")?.as_array()?;
        let image = images.first()?;
        let filename = image.get("filename")?.as_str()?;
        if filename.is_empty() {
            return None;
        }
        Some(OutputRef {
            filename: filename.to_string(),
            subfolder: image
                .get("subfolder")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            file_type: image
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("output")
                .to_string(),
        })
    };

    if let Some(node_id) = preferred_node {
        if let Some(output) = outputs.get(node_id).and_then(&pick) {
            return Some(output);
        }
    }
    outputs.values().find_map(pick)
}

fn history_error_message(status: Option<&Value>) -> String {
    if let Some(messages) = status
        .and_then(|s| s.get("messages"))
        .and_then(Value::as_array)
    {
        for message in messages {
            if let Some(payload) = message.as_array().and_then(|items| items.get(1)) {
                if let Some(exception) = payload
                    .get("exception_message")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                {
                    return exception.to_string();
                }
                if let Some(node_id) = payload.get("node_id").and_then(Value::as_str) {
                    return format!("节点 {} 执行异常", node_id);
                }
            }
        }
    }
    "未知错误（请查看 ComfyUI 日志）".to_string()
}

/// 从 ComfyUI 的 400 响应中提取可读错误。
pub fn extract_comfy_error(body: &str) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        let mut parts: Vec<String> = Vec::new();
        if let Some(message) = value
            .pointer("/error/message")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            parts.push(message.to_string());
        } else if let Some(message) = value
            .get("error")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            parts.push(message.to_string());
        }
        if let Some(details) = value
            .pointer("/error/details")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            parts.push(details.to_string());
        }
        if let Some(node_errors) = value.get("node_errors").and_then(Value::as_object) {
            for (node_id, node_error) in node_errors {
                if let Some(errors) = node_error.get("errors").and_then(Value::as_array) {
                    for error in errors {
                        if let Some(message) = error.get("message").and_then(Value::as_str) {
                            let class = node_error
                                .get("class_type")
                                .and_then(Value::as_str)
                                .unwrap_or("节点");
                            parts.push(format!("节点 {}（{}）：{}", node_id, class, message));
                        }
                    }
                }
            }
        }
        if !parts.is_empty() {
            return parts.join("；");
        }
    }
    truncate(body, 300)
}

fn truncate(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        trimmed.to_string()
    } else {
        let mut out: String = trimmed.chars().take(max).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    #[test]
    fn client_rejects_invalid_url() {
        let err = ComfyClient::new("127.0.0.1:8188", 30).expect_err("no scheme");
        assert!(err.contains("http://"), "{err}");
        let err = ComfyClient::new("ftp://x", 30).expect_err("bad scheme");
        assert!(err.contains("http://"), "{err}");
        ComfyClient::new("http://127.0.0.1:8188/", 30).expect("valid");
    }

    #[test]
    fn extract_error_prefers_message_and_node_errors() {
        let body = r#"{
          "error": {"type": "prompt_outputs_failed_validation", "message": "Prompt outputs failed validation", "details": ""},
          "node_errors": {"6": {"class_type": "KSampler", "errors": [{"message": "Value not in list"}]}}
        }"#;
        let msg = extract_comfy_error(body);
        assert!(msg.contains("Prompt outputs failed validation"), "{msg}");
        assert!(msg.contains("KSampler"), "{msg}");
        assert!(msg.contains("Value not in list"), "{msg}");
    }

    #[test]
    fn extract_error_handles_plain_text() {
        assert_eq!(extract_comfy_error("upstream boom"), "upstream boom");
    }

    #[test]
    fn select_output_prefers_bound_node() {
        let outputs = json!({
            "7": {"images": [{"filename": "wrong.png", "subfolder": "", "type": "output"}]},
            "8": {"images": [{"filename": "right.png", "subfolder": "sub", "type": "output"}]}
        });
        let picked = select_output(Some(&outputs), Some("8")).unwrap();
        assert_eq!(picked.filename, "right.png");
        assert_eq!(picked.subfolder, "sub");

        let picked = select_output(Some(&outputs), Some("missing")).unwrap();
        assert_eq!(picked.filename, "wrong.png");
        assert!(select_output(None, None).is_none());
    }

    #[test]
    fn history_error_extracts_exception() {
        let status = json!({
            "status_str": "error",
            "messages": [["execution_error", {"node_id": "6", "exception_message": "CUDA out of memory"}]]
        });
        assert_eq!(history_error_message(Some(&status)), "CUDA out of memory");
    }

    // ── 本地 mock ComfyUI ────────────────────────────────────────

    fn read_request(stream: &mut TcpStream) -> String {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => buffer.extend_from_slice(&chunk[..n]),
            }
            if let Some(pos) = find_subsequence(&buffer, b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buffer[..pos]).to_string();
                let content_length = head
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                let body_start = pos + 4;
                while buffer.len() < body_start + content_length {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                    }
                }
                return head;
            }
        }
        String::new()
    }

    fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    fn spawn_server(
        expected_requests: usize,
        respond: fn(&str) -> (u16, &'static str, Vec<u8>),
    ) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handle = thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..expected_requests {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let head = read_request(&mut stream);
                let first_line = head.lines().next().unwrap_or("").to_string();
                seen.push(first_line.clone());
                let (status, content_type, body) = respond(&first_line);
                let reason = if status == 200 { "OK" } else { "Bad Request" };
                let header = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    status,
                    reason,
                    content_type,
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&body);
                let _ = stream.flush();
            }
            seen
        });
        (format!("http://{}", addr), handle)
    }

    fn happy_responder(first_line: &str) -> (u16, &'static str, Vec<u8>) {
        if first_line.starts_with("POST /upload/image") {
            (
                200,
                "application/json",
                br#"{"name":"renamed.png","subfolder":"","type":"input"}"#.to_vec(),
            )
        } else if first_line.starts_with("POST /prompt") {
            (200, "application/json", br#"{"prompt_id":"p1"}"#.to_vec())
        } else if first_line.starts_with("GET /queue") {
            (
                200,
                "application/json",
                br#"{"queue_running":[],"queue_pending":[]}"#.to_vec(),
            )
        } else if first_line.starts_with("GET /history/p1") {
            (
                200,
                "application/json",
                br#"{"p1":{"status":{"status_str":"success","completed":true},"outputs":{"8":{"images":[{"filename":"out.png","subfolder":"","type":"output"}]}}}}"#
                    .to_vec(),
            )
        } else if first_line.starts_with("GET /view") {
            (200, "image/png", b"\x89PNG\r\n\x1a\nmock".to_vec())
        } else {
            (404, "application/json", b"{}".to_vec())
        }
    }

    #[test]
    fn upload_queue_wait_download_roundtrip() {
        let (base, handle) = spawn_server(5, happy_responder);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            let client = ComfyClient::new(&base, 10).unwrap();
            let uploaded = client
                .upload_image("a.png", b"\x89PNG\r\n\x1a\npayload".to_vec())
                .await
                .unwrap();
            assert_eq!(uploaded, "renamed.png");

            let fetched = client
                .queue_and_wait_image(&json!({"1": {"class_type": "X", "inputs": {}}}), Some("8"))
                .await
                .unwrap();
            assert_eq!(fetched.prompt_id, "p1");
            assert_eq!(fetched.output.filename, "out.png");
            assert_eq!(fetched.bytes, b"\x89PNG\r\n\x1a\nmock");
        });
        let seen = handle.join().unwrap();
        assert_eq!(seen.len(), 5);
        assert!(seen[0].starts_with("POST /upload/image"), "{:?}", seen);
        assert!(seen[1].starts_with("POST /prompt"), "{:?}", seen);
        assert!(seen[2].starts_with("GET /queue"), "{:?}", seen);
        assert!(seen[3].starts_with("GET /history/p1"), "{:?}", seen);
        assert!(seen[4].starts_with("GET /view"), "{:?}", seen);
    }

    #[test]
    fn queue_error_body_is_surfaced() {
        let (base, handle) = spawn_server(1, |first_line| {
            assert!(first_line.starts_with("POST /prompt"));
            (
                400,
                "application/json",
                br#"{"error":{"message":"Prompt outputs failed validation"},"node_errors":{"6":{"class_type":"KSampler","errors":[{"message":"Value not in list"}]}}}"#
                    .to_vec(),
            )
        });
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let err = runtime
            .block_on(async move {
                let client = ComfyClient::new(&base, 10).unwrap();
                client
                    .queue_prompt(&json!({"1": {"class_type": "X", "inputs": {}}}))
                    .await
            })
            .expect_err("should fail");
        handle.join().unwrap();
        assert!(err.contains("failed validation"), "{err}");
        assert!(err.contains("Value not in list"), "{err}");
    }
}
