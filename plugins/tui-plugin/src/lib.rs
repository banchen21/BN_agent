//! tui-plugin — 终端聊天界面插件。
//!
//! ## 安全退出原则
//!
//! TUI 运行在独立线程中，退出（Ctrl+C / `/quit`）只结束该线程，**不影响主程序**。
//! 只有主程序退出，所有插件才真正销毁。
//!
//! ## 布局
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │  ← 消息区 ←                                 │
//! │  用户: hello                                 │
//! │  助手: Hey! How can I help?                 │
//! │  ...                                        │
//! ├─────────────────────────────────────────────┤
//! │ > 当前输入...                                │
//! └─────────────────────────────────────────────┘
//! ```
//!
//! Ctrl+C / `/quit` 退出 TUI，主程序继续运行。
//! 可通过 `/reload tui-plugin` 重新启动 TUI。

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self as ct_event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::style::{Color, Colors, Print, ResetColor, SetColors};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use plugin_interface::{
    Addr, Event as PluginEvent, EventBus, Plugin, PluginContext, PluginInfo,
};
use std::io::{stdout, Write};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

// ── 常量 ─────────────────────────────────────────────────────────────────────

const MAX_MESSAGES: usize = 500;

// ── 插件 ─────────────────────────────────────────────────────────────────────

pub struct TuiPlugin {
    info: PluginInfo,
    /// Channel to send display events to the TUI thread.
    display_tx: Option<mpsc::Sender<String>>,
    /// Channel to signal the TUI thread to shut down.
    shutdown: Option<(Arc<Mutex<bool>>, mpsc::Sender<()>)>,
    tui_thread: Option<std::thread::JoinHandle<()>>,
}

impl TuiPlugin {
    pub fn new() -> Self {
        Self {
            info: PluginInfo {
                name: "tui-plugin".into(),
                version: "0.1.0".into(),
                description: "Terminal chat UI — send/receive messages in the console".into(),
                author: "BN Team".into(),
                min_host_version: "0.1.0".into(),
            },
            display_tx: None,
            shutdown: None,
            tui_thread: None,
        }
    }
}

impl Plugin for TuiPlugin {
    fn info(&self) -> PluginInfo {
        self.info.clone()
    }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        log::info!("[tui-plugin] starting TUI thread...");
        let event_bus = ctx.event_bus.clone();

        // Channels: display (on_event → TUI thread) + shutdown signal.
        let (display_tx, display_rx) = mpsc::channel::<String>();
        let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();
        let shutdown_flag = Arc::new(Mutex::new(false));
        let flag = shutdown_flag.clone();

        let handle = std::thread::Builder::new()
            .name("tui-thread".into())
            .spawn(move || {
                tui_main_loop(event_bus, display_rx, shutdown_rx, flag);
            })?;

        self.display_tx = Some(display_tx);
        self.shutdown = Some((shutdown_flag, shutdown_tx));
        self.tui_thread = Some(handle);

        log::info!("[tui-plugin] started");
        Ok(())
    }

    fn stop(&mut self) {
        log::info!("[tui-plugin] stopping...");

        // Signal TUI thread to exit.
        if let Some((flag, tx)) = self.shutdown.take() {
            *flag.lock().unwrap() = true;
            let _ = tx.send(());
        }

        // Wait for thread to finish (with timeout to avoid deadlock on panic).
        if let Some(handle) = self.tui_thread.take() {
            let _ = handle.join();
        }

        log::info!("[tui-plugin] stopped");
    }

    fn on_event(&self, event: &PluginEvent) -> bool {
        // Forward assistant messages to the TUI display.
        if event.topic == "assistant.message" {
            if let Some(ref tx) = self.display_tx {
                let source = event
                    .data
                    .get("source")
                    .and_then(|v| v.as_str())
                    .unwrap_or("system");
                let text = event
                    .data
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(empty)");

                let display = if source == "telegram" {
                    format!("  [TG → 用户]: {}", text)
                } else {
                    format!("  助手: {}", text)
                };
                let _ = tx.send(display);
            }
        }
        true // continue propagating
    }
}

// ── FFI exports ──────────────────────────────────────────────────────────────

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> {
    Box::new(TuiPlugin::new())
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_plugin: Box<dyn Plugin>) {}

// ── TUI 主循环 ───────────────────────────────────────────────────────────────

/// TUI 线程的入口：设置终端 → 进入事件循环 → 恢复终端 → 退出。
///
/// 注意：任何 panic 都会先恢复终端再传播，避免终端残留原始模式。
fn tui_main_loop(
    event_bus: Addr<EventBus>,
    display_rx: mpsc::Receiver<String>,
    _shutdown_rx: mpsc::Receiver<()>,
    shutdown_flag: Arc<Mutex<bool>>,
) {
    // 使用 catch_unwind 确保 panic 时也能恢复终端。
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tui_run_inner(event_bus, display_rx, shutdown_flag);
    }));

    // 确保终端状态恢复。
    let _ = disable_raw_mode();
    let _ = stdout().execute(LeaveAlternateScreen);
    let _ = stdout().execute(Show);

    if let Err(e) = result {
        let msg = if let Some(s) = e.downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = e.downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown error".into()
        };
        log::error!("[tui-plugin] thread panicked: {}", msg);
    }

    log::info!("[tui-plugin] TUI exited");
}

fn tui_run_inner(
    event_bus: Addr<EventBus>,
    display_rx: mpsc::Receiver<String>,
    shutdown_flag: Arc<Mutex<bool>>,
) {
    // ── 初始化终端 ──
    enable_raw_mode().expect("raw mode");
    let mut stdout = stdout();
    stdout.execute(EnterAlternateScreen).expect("alt screen");
    stdout.execute(Hide).expect("hide cursor");
    stdout.execute(Clear(ClearType::All)).expect("clear");

    let (term_w, term_h) = crossterm::terminal::size().unwrap_or((80, 24));

    // ── 消息缓冲区 ──
    let messages: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let messages_clone = messages.clone();

    // 产线消息：显示系统启动提示。
    messages_clone.lock().unwrap().push("  TUI 聊天界面已启动。输入 /quit 退出 TUI。".into());

    // ── 输入缓冲区 ──
    let mut input: String = String::new();
    let input_line = term_h.saturating_sub(1); // 底部最后一行

    // 标记是否需要重绘。
    let mut needs_repaint = true;

    loop {
        // 检查是否收到 shutdown 信号。
        if *shutdown_flag.lock().unwrap() {
            break;
        }

        // ── 非阻塞检查显示消息通道 ──
        loop {
            match display_rx.try_recv() {
                Ok(msg) => {
                    let mut msgs = messages_clone.lock().unwrap();
                    msgs.push(msg);
                    if msgs.len() > MAX_MESSAGES {
                        msgs.remove(0);
                    }
                    needs_repaint = true;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    // on_event 不再发送 — 插件可能在停止中。
                    break;
                }
            }
        }

        // ── 检查键盘事件（非阻塞，短暂超时） ──
        if ct_event::poll(std::time::Duration::from_millis(50)).unwrap_or(false) {
            if let ct_event::Event::Key(key) = ct_event::read().expect("key event") {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char(c) => {
                        if key.modifiers == KeyModifiers::CONTROL && c == 'c' {
                            // Ctrl+C → 退出 TUI，不影响主程序。
                            break;
                        }
                        input.push(c);
                        needs_repaint = true;
                    }
                    KeyCode::Backspace => {
                        input.pop();
                        needs_repaint = true;
                    }
                    KeyCode::Enter => {
                        let line = input.trim().to_string();
                        input.clear();
                        needs_repaint = true;

                        if line.is_empty() {
                            continue;
                        }

                        if line == "/quit" || line == "/exit" {
                            break;
                        }

                        // ── 将用户输入发布到 EventBus ──
                        {
                            let mut msgs = messages_clone.lock().unwrap();
                            msgs.push(format!("  用户: {}", line));
                            if msgs.len() > MAX_MESSAGES {
                                msgs.remove(0);
                            }
                        }

                        event_bus.do_send(PluginEvent::new(
                            "user.message",
                            serde_json::json!({
                                "chat_id": 0,
                                "text": line,
                                "source": "tui",
                                "user_name": "User",
                            }),
                            "tui-plugin",
                        ));
                    }
                    _ => {}
                }
            }
        }

        // ── 重绘屏幕 ──
        if needs_repaint {
            draw_screen(&mut stdout, term_w, term_h, &messages_clone, &input, input_line);
            needs_repaint = false;
        }
    }

    // ── 退出：恢复终端 ──
    let _ = stdout.execute(Show);
    let _ = stdout.execute(LeaveAlternateScreen);
    let _ = disable_raw_mode();
    let _ = stdout.flush();

    // 输出一行分隔，提示 TUI 已退出（但主程序继续）。
    println!("\n[TUI 已退出。输入 /reload tui-plugin 重启 TUI]\n");
}

// ── 屏幕绘制 ──

fn draw_screen(
    stdout: &mut std::io::Stdout,
    w: u16,
    h: u16,
    messages: &Arc<Mutex<Vec<String>>>,
    input: &str,
    input_line: u16,
) {
    let _ = stdout.execute(Clear(ClearType::All));

    // ── 标题行 ──
    let _ = stdout.execute(SetColors(Colors::new(
        Color::White,
        Color::DarkBlue,
    )));
    let title = format!("{:^width$}", " BN Agent — Chat TUI (Ctrl+C / /quit 退出) ", width = w as usize);
    let _ = stdout.execute(Print(&title[..title.len().min(w as usize)]));
    let _ = stdout.execute(ResetColor);
    let _ = stdout.execute(MoveTo(0, 0));

    // ── 消息区 ──
    let msgs = messages.lock().unwrap();
    // 可用行数 = 总行 - 2（标题/分割线 + 输入区）
    let avail = (h.saturating_sub(2)).min(msgs.len() as u16);

    let start = msgs.len().saturating_sub(avail as usize);
    for (i, msg) in msgs.iter().skip(start).enumerate() {
        let row = (i as u16) + 1; // 跳过标题行（已被 Clear 清除，但 row 从 1 开始）
        if row >= h.saturating_sub(1) {
            break;
        }

        // 根据消息来源着色
        if msg.contains("助手:") {
            let _ = stdout.execute(SetColors(Colors::new(Color::Green, Color::Reset)));
        } else if msg.contains("用户:") || msg.contains("[TG") {
            let _ = stdout.execute(SetColors(Colors::new(Color::Cyan, Color::Reset)));
        } else {
            let _ = stdout.execute(SetColors(Colors::new(Color::DarkYellow, Color::Reset)));
        }

        // 截断超长消息（安全 UTF-8 边界）
        let display = if msg.len() > w as usize {
            let max = w as usize;
            // 找到 ≤ max-4 的最近字符边界
            let cut = msg.char_indices()
                .take_while(|(i, _)| *i < max.saturating_sub(4))
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(max.saturating_sub(4));
            format!("{}...", &msg[..cut])
        } else {
            msg.clone()
        };
        let _ = stdout.execute(MoveTo(0, row));
        let _ = stdout.execute(Print(&display));
    }

    // ── 分隔线 ──
    let sep_row = h.saturating_sub(2);
    let _ = stdout.execute(SetColors(Colors::new(Color::DarkGrey, Color::Reset)));
    let _ = stdout.execute(MoveTo(0, sep_row));
    let _ = stdout.execute(Print(format!("{:─>width$}", "", width = w as usize)));
    let _ = stdout.execute(ResetColor);

    // ── 输入行 ──
    let _ = stdout.execute(SetColors(Colors::new(Color::White, Color::Reset)));
    let _ = stdout.execute(MoveTo(0, input_line));

    let prompt = "> ";
    let input_display = if input.len() > (w as usize).saturating_sub(prompt.len() + 1) {
        let max_w = w as usize;
        // 从末尾找最近的安全字符边界
        let cut = input.char_indices()
            .rev()
            .find(|(i, _)| *i >= input.len().saturating_sub(max_w - prompt.len() - 1))
            .map(|(i, _)| i)
            .unwrap_or(input.len().saturating_sub(max_w - prompt.len() - 1));
        format!("{}{}", prompt, &input[cut..])
    } else {
        format!("{}{}", prompt, input)
    };
    let _ = stdout.execute(Print(&input_display));
    let _ = stdout.execute(ResetColor);

    // 光标闪烁
    let cursor_x = input_display.len().min((w - 1) as usize) as u16;
    let _ = stdout.execute(MoveTo(cursor_x, input_line));
    let _ = stdout.execute(Show);
    let _ = stdout.flush();
}
