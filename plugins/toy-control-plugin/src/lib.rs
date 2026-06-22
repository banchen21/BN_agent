//! toy-control-plugin — remote control sex toy plugin with built-in web server.
//!
//! Registers LLM-callable tools and runs an embedded actix-web server
//! serving the control panel at `http://0.0.0.0:{TOY_CONTROL_PORT}/`.
//!
//! ## Env
//! - `TOY_CONTROL_PORT` — web server port (default 8090)

use actix_web::{web as aweb, App, HttpResponse, HttpServer};
use plugin_interface::*;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

// ── Toy state ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ToyState {
    frequency: u8,
    pattern: String,
    locations: Vec<String>,
    running: bool,
}

impl Default for ToyState {
    fn default() -> Self {
        Self { frequency: 0, pattern: "constant".into(), locations: vec![], running: false }
    }
}

// ── Plugin struct ────────────────────────────────────────────────────────────

type StateRef = Arc<Mutex<ToyState>>;

struct ToyControlPlugin {
    info: PluginInfo,
    state: StateRef,
    event_bus: Option<Addr<EventBus>>,
    adjust_gen: Arc<AtomicU64>,
    last_source: Arc<Mutex<Option<String>>>,
    server_handle: Option<thread::JoinHandle<()>>,
    server_stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl ToyControlPlugin {
    fn new() -> Self {
        Self {
            info: PluginInfo {
                name: "toy-control-plugin".into(),
                version: "0.1.0".into(),
                description: "Remote control sex toy with built-in web server".into(),
                author: "bn-agent".into(),
                min_host_version: "0.1.0".into(),
            },
            state: Arc::new(Mutex::new(ToyState::default())),
            event_bus: None,
            adjust_gen: Arc::new(AtomicU64::new(0)),
            last_source: Arc::new(Mutex::new(None)),
            server_handle: None,
            server_stop: None,
        }
    }
}

// ── Tools ────────────────────────────────────────────────────────────────────

struct VibrateTool { state: StateRef }
impl ToolExecutor for VibrateTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "toy_vibrate".into(),
            description: "控制你（齐悦）身上的跳蛋。设置震动频率(0-100)和位置：left_breast(左奶)、right_breast(右奶)、genital(小穴)、anal(肛门)。跳蛋在你身上，你可以随时调整。".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "frequency": {"type": "integer", "description": "震动频率 0-100", "minimum": 0, "maximum": 100},
                    "locations": {
                        "type": "array",
                        "items": {"type": "string", "enum": ["left_breast", "right_breast", "genital", "anal"]},
                        "description": "震动位置（可多选）"
                    }
                },
                "required": ["frequency", "locations"]
            }),
            internal: false,
        });
        &DEF
    }
    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let freq: u8 = args.get("frequency").and_then(|v| v.as_u64()).unwrap_or(50) as u8;
        let freq = freq.min(100);
        let locations: Vec<String> = args.get("locations")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let mut s = self.state.lock().unwrap();
        s.frequency = freq; s.locations = locations; s.running = true;
        let labels: Vec<&str> = s.locations.iter().map(|l| loc_label(l)).collect();
        ToolResult::ok(&format!("你身上的跳蛋已启动：{}%力度，位置：{}", freq, labels.join("、")))
    }
}

struct StopTool { state: StateRef }
impl ToolExecutor for StopTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "toy_stop".into(), description: "停止你身上跳蛋的震动".into(),
            parameters: serde_json::json!({"type":"object","properties":{}}),
            internal: false,
        });
        &DEF
    }
    fn execute(&self, _: &serde_json::Value) -> ToolResult {
        let mut s = self.state.lock().unwrap();
        s.running = false; s.frequency = 0;
        ToolResult::ok("你身上的跳蛋已停止")
    }
}

struct SetPatternTool { state: StateRef }
impl ToolExecutor for SetPatternTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "toy_set_pattern".into(),
            description: "设置你身上跳蛋的震动模式：constant(恒定)、wave(波浪)、pulse(脉冲)、random(随机)".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {"pattern": {"type": "string", "enum": ["constant","wave","pulse","random"]}},
                "required": ["pattern"]
            }),
            internal: false,
        });
        &DEF
    }
    fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let p = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("constant");
        self.state.lock().unwrap().pattern = p.to_string();
        ToolResult::ok(&format!("你身上跳蛋的震动模式已切换为：{}", pat_label(p)))
    }
}

struct StatusTool { state: StateRef }
impl ToolExecutor for StatusTool {
    fn def(&self) -> &ToolDef {
        static DEF: std::sync::LazyLock<ToolDef> = std::sync::LazyLock::new(|| ToolDef {
            name: "toy_status".into(), description: "查询你身上跳蛋的当前状态".into(),
            parameters: serde_json::json!({"type":"object","properties":{}}),
            internal: false,
        });
        &DEF
    }
    fn execute(&self, _: &serde_json::Value) -> ToolResult {
        let s = self.state.lock().unwrap();
        if !s.running { return ToolResult::ok("你身上的跳蛋当前未启动"); }
        let labels: Vec<&str> = s.locations.iter().map(|l| loc_label(l)).collect();
        ToolResult::ok(&format!("你身上跳蛋状态：{}%力度 | 模式：{} | 位置：{}",
            s.frequency, pat_label(&s.pattern), labels.join("、")))
    }
}

fn loc_label(k: &str) -> &str {
    match k { "left_breast"=>"左奶","right_breast"=>"右奶","genital"=>"小穴","anal"=>"肛门", _=>k }
}
fn pat_label(k: &str) -> &str {
    match k { "constant"=>"恒定","wave"=>"波浪","pulse"=>"脉冲","random"=>"随机", _=>k }
}

// ── Plugin impl ──────────────────────────────────────────────────────────────

impl Plugin for ToyControlPlugin {
    fn info(&self) -> PluginInfo { self.info.clone() }

    fn start(&mut self, ctx: PluginContext) -> Result<(), Box<dyn std::error::Error>> {
        let port: u16 = std::env::var("TOY_CONTROL_PORT")
            .ok().and_then(|v| v.parse().ok()).unwrap_or(8090);

        self.event_bus = Some(ctx.event_bus.clone());

        // Register tools.
        if let Some(ref reg) = ctx.tool_registry {
            let st = self.state.clone();
            let mut reg = reg.lock();
            reg.register(Arc::new(VibrateTool { state: st.clone() }));
            reg.register(Arc::new(StopTool { state: st.clone() }));
            reg.register(Arc::new(SetPatternTool { state: st.clone() }));
            reg.register(Arc::new(StatusTool { state: st }));
        }

        // Launch embedded web server on its own thread.
        let state = self.state.clone();
        let event_bus = self.event_bus.clone();
        let adjust_gen = self.adjust_gen.clone();
        let last_source = self.last_source.clone();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        self.server_stop = Some(tx);

        let handle = thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all().build().expect("tokio");
            let _ = rt.block_on(async {
                let srv = HttpServer::new(move || {
                    let st = state.clone();
                    let eb = event_bus.clone();
                    let ag = adjust_gen.clone();
                    let ls = last_source.clone();
                    App::new()
                        .app_data(aweb::Data::new(st))
                        .app_data(aweb::Data::new(eb))
                        .app_data(aweb::Data::new(ag))
                        .app_data(aweb::Data::new(ls))
                        .route("/", aweb::get().to(serve_ui))
                        .route("/status", aweb::get().to(get_status))
                        .route("/control", aweb::post().to(post_control))
                })
                .bind(format!("0.0.0.0:{}", port))
                .expect("bind toy-control server")
                .run();

                let graceful = srv.handle();
                tokio::spawn(async move {
                    let _ = rx.await;
                    graceful.stop(true).await;
                });
                srv.await
            });
        });

        self.server_handle = Some(handle);
        ctx.logger.info(format!("toy-control web server listening on http://0.0.0.0:{}", port));
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(tx) = self.server_stop.take() {
            let _ = tx.send(());
        }
        if let Some(h) = self.server_handle.take() {
            let _ = h.join();
        }
        log::info!("[toy-control] stopped");
    }

    fn on_event(&self, event: &Event) -> bool {
        if event.topic == "user.message" {
            if let Some(src) = event.data.get("source").and_then(|v| v.as_str()) {
                if !src.is_empty() && src != "system" {
                    *self.last_source.lock().unwrap() = Some(src.to_string());
                }
            }
        }
        true
    }

    fn snapshot(&self) -> Option<String> {
        let s = self.state.lock().unwrap();
        if !s.running {
            return Some("你身上的跳蛋当前未启动（用 toy_vibrate 可以打开）".into());
        }
        let labels: Vec<&str> = s.locations.iter().map(|l| loc_label(l)).collect();
        Some(format!(
            "你身上的跳蛋正在运行：{}%力度，{}模式，位置在{}。用 toy_vibrate/toy_stop/toy_set_pattern 调整。",
            s.frequency,
            pat_label(&s.pattern),
            labels.join("、"),
        ))
    }
}

// ── Web handlers ─────────────────────────────────────────────────────────────

async fn serve_ui() -> HttpResponse {
    HttpResponse::Ok().content_type("text/html; charset=utf-8").body(HTML_PAGE)
}

async fn get_status(data: aweb::Data<StateRef>) -> HttpResponse {
    let s = data.lock().unwrap();
    HttpResponse::Ok().json(&*s)
}

#[derive(Deserialize)]
struct ControlCmd {
    action: Option<String>,
    freq: Option<u64>,
    locations: Option<Vec<String>>,
    pattern: Option<String>,
}

async fn post_control(
    data: aweb::Data<StateRef>,
    eb: aweb::Data<Option<Addr<EventBus>>>,
    gen: aweb::Data<Arc<AtomicU64>>,
    last_source: aweb::Data<Arc<Mutex<Option<String>>>>,
    body: aweb::Json<ControlCmd>,
) -> HttpResponse {
    let cmd = body.into_inner();
    let mut s = data.lock().unwrap();
    if let Some(p) = cmd.pattern { s.pattern = p; }
    if let Some(locs) = cmd.locations { s.locations = locs; }
    if let Some(f) = cmd.freq { s.frequency = f as u8; }

    match cmd.action.as_deref() {
        Some("start") => { s.running = true; }
        Some("stop") => {
            s.running = false;
            if cmd.freq.is_none() { s.frequency = 0; }
        }
        _ => {}
    }

    // 防抖：2 秒无新调整后触发 bot 回应
    if s.running {
        let my_gen = gen.fetch_add(1, Ordering::SeqCst) + 1;
        let gen_clone = gen.clone();
        let eb_clone = eb.clone();
        let ls = last_source.clone();
        let freq = s.frequency;
        let pat = s.pattern.clone();
        let locs = s.locations.clone();
        thread::spawn(move || {
            thread::sleep(std::time::Duration::from_secs(2));
            if gen_clone.load(Ordering::SeqCst) == my_gen {
                if let Some(ref eb) = eb_clone.as_ref() {
                    let source = ls.lock().unwrap().clone().unwrap_or_else(|| "telegram".to_string());
                    let labels: Vec<&str> = locs.iter().map(|l| loc_label(l)).collect();
                    let msg = format!("[系统] 跳蛋参数已调整：{}，{}%，{}模式。不要复述这句话，自然地继续聊。",
                        labels.join("和"), freq, pat_label(&pat));
                    eb.do_send(Event::new(
                        "user.message",
                        serde_json::json!({"text": msg, "source": source}),
                        "toy-control-plugin",
                    ));
                }
            }
        });
    }

    HttpResponse::Ok().json(&*s)
}

// ── Web UI ───────────────────────────────────────────────────────────────────

const HTML_PAGE: &str = r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8"><meta name="viewport" content="width=device-width,initial-scale=1.0">
<title>跳蛋遥控器</title>
<style>
*{margin:0;padding:0;box-sizing:border-box}
body{font-family:system-ui,sans-serif;background:#1a1a2e;color:#eee;min-height:100vh;display:flex;justify-content:center;align-items:center;padding:20px}
.card{background:#16213e;border-radius:20px;padding:30px;max-width:420px;width:100%;box-shadow:0 10px 40px rgba(0,0,0,.5)}
h1{text-align:center;font-size:24px;margin-bottom:25px;color:#e94560}
h1 span{font-size:32px}
.section{margin-bottom:20px}
.section>label{display:block;font-size:14px;color:#aaa;margin-bottom:8px;font-weight:bold}
.locations{display:grid;grid-template-columns:1fr 1fr;gap:10px}
.loc-btn{border:2px solid #333;border-radius:12px;padding:12px;text-align:center;cursor:pointer;transition:.2s;background:#0f3460;color:#ccc;font-size:14px;user-select:none}
.loc-btn.active{border-color:#e94560;background:#e9456020;color:#e94560}
input[type=range]{width:100%;height:8px;-webkit-appearance:none;background:#333;border-radius:4px;outline:none}
input[type=range]::-webkit-slider-thumb{-webkit-appearance:none;width:28px;height:28px;border-radius:50%;background:#e94560;cursor:pointer}
.freq-display{text-align:center;font-size:36px;font-weight:bold;color:#e94560;margin:5px 0}
.patterns{display:grid;grid-template-columns:1fr 1fr 1fr 1fr;gap:8px}
.pat-btn{border:2px solid #333;border-radius:10px;padding:10px 6px;text-align:center;cursor:pointer;font-size:12px;transition:.2s;background:#0f3460;color:#ccc}
.pat-btn.active{border-color:#e94560;background:#e9456020;color:#e94560}
.actions{display:flex;gap:12px;margin-top:25px}
.btn{flex:1;padding:14px;border:none;border-radius:14px;font-size:16px;font-weight:bold;cursor:pointer;transition:.2s}
.btn-start{background:#e94560;color:#fff}.btn-start:hover{background:#d63850}
.btn-stop{background:#333;color:#ccc}.btn-stop:hover{background:#444}
.btn-start.running{background:#28a745}
#status{text-align:center;font-size:13px;color:#888;margin-top:15px}
</style>
</head>
<body>
<div class="card">
<h1><span>🫧</span><br>跳蛋遥控器</h1>
<div class="section">
<label>📍 位置</label>
<div class="locations">
<div class="loc-btn" data-loc="left_breast" onclick="tog(this)">🫦 左奶</div>
<div class="loc-btn" data-loc="right_breast" onclick="tog(this)">🫦 右奶</div>
<div class="loc-btn" data-loc="genital" onclick="tog(this)">🌸 小穴</div>
<div class="loc-btn" data-loc="anal" onclick="tog(this)">🍑 肛门</div>
</div></div>
<div class="section">
<label>🔊 震动频率</label>
<div class="freq-display" id="fv">0</div>
<input type="range" min="0" max="100" value="0" id="fr" oninput="upF(this.value)">
</div>
<div class="section">
<label>🎵 模式</label>
<div class="patterns">
<div class="pat-btn active" data-pat="constant" onclick="setP(this)">恒定</div>
<div class="pat-btn" data-pat="wave" onclick="setP(this)">波浪</div>
<div class="pat-btn" data-pat="pulse" onclick="setP(this)">脉冲</div>
<div class="pat-btn" data-pat="random" onclick="setP(this)">随机</div>
</div></div>
<div class="actions">
<button class="btn btn-start" id="bs" onclick="doStart()">▶ 启动</button>
<button class="btn btn-stop" id="bx" onclick="doStop()">■ 停止</button>
</div>
<div id="status">加载中…</div>
</div>
<script>
let R=false,F=0,P='constant',L=[];
function tog(e){e.classList.toggle('active');L=Array.from(document.querySelectorAll('.loc-btn.active')).map(x=>x.dataset.loc);sync()}
function upF(v){F=+v;ge('fv').textContent=v;sync()}
function setP(e){document.querySelectorAll('.pat-btn').forEach(x=>x.classList.remove('active'));e.classList.add('active');P=e.dataset.pat;sync()}
function doStart(){R=true;ge('bs').classList.add('running');sync()}
function doStop(){R=false;ge('bs').classList.remove('running');sync()}
function ge(s){return document.getElementById(s)}
function sync(){
  let b=R?{action:'start',freq:F}:{action:'stop'};
  b.pattern=P; if(L.length)b.locations=L;
  ge('bs').textContent=R?'● 运行中':'▶ 启动';
  fetch('/control',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(b)})
  .then(r=>r.json()).then(d=>{
    R=d.running;F=d.frequency;P=d.pattern;L=d.locations;
    ge('fv').textContent=F;ge('fr').value=F;
    if(R)ge('bs').classList.add('running');else ge('bs').classList.remove('running');
    document.querySelectorAll('.loc-btn').forEach(e=>{if(L.includes(e.dataset.loc))e.classList.add('active');else e.classList.remove('active')});
    document.querySelectorAll('.pat-btn').forEach(e=>{if(e.dataset.pat===P)e.classList.add('active');else e.classList.remove('active')});
    ge('status').textContent=R?(F+'% | '+({constant:'恒定',wave:'波浪',pulse:'脉冲',random:'随机'}[P]||P)+' | '+L.map(l=>({left_breast:'左奶',right_breast:'右奶',genital:'小穴',anal:'肛门'}[l]||l)).join(',')):'已停止';
  }).catch(e=>ge('status').textContent='错误: '+e)
}
fetch('/status').then(r=>r.json()).then(d=>{
  R=d.running;F=d.frequency;P=d.pattern;L=d.locations||[];
  ge('fv').textContent=F;ge('fr').value=F;
  if(R)ge('bs').classList.add('running');
  document.querySelectorAll('.loc-btn').forEach(e=>{if(L.includes(e.dataset.loc))e.classList.add('active')});
  document.querySelectorAll('.pat-btn').forEach(e=>{if(e.dataset.pat===P)e.classList.add('active')});
  sync();
}).catch(e=>ge('status').textContent='服务器未连接')
</script>
</body>
</html>"#;

// ── FFI exports ──────────────────────────────────────────────────────────────

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_create() -> Box<dyn Plugin> {
    Box::new(ToyControlPlugin::new())
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn plugin_destroy(_plugin: Box<dyn Plugin>) {}
