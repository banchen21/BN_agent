"""悦悦 LiveKit Agent：全双工语音 + 用户视频订阅 + 视觉工具。

启动：
    .venv\\Scripts\\python.exe agent_main.py start

链路：
    用户麦克风 -> Silero VAD 切句 -> yueyue 8877 /api/asr
    -> /api/chat(SSE) -> /api/tts_stream(SSE) -> 房间音频
    用户摄像头 -> LiveKit 视频轨 -> AgentSession(video_sampler=FrameStore) -> 最新帧
    LLM 决定看图时（<look>/<snapshot/> 或关键词兜底）-> 视觉模型 -> 口语回复

显式 dispatch：WorkerOptions(agent_name="yueyue")，由 call-backend 的
AgentDispatchService.CreateDispatch 派发（metadata 里带 userID/identity/mediaType）。
"""
from __future__ import annotations

import asyncio
import json
import logging
import sys
import time

from livekit.agents import (
    Agent,
    AgentSession,
    JobContext,
    JobProcess,
    WorkerOptions,
    cli,
    function_tool,
    stt,
)
from livekit import rtc
from livekit.agents.voice.room_io import AudioInputOptions, RoomOptions
from livekit.plugins import silero

from bargein import BargeInController, BargeInState
from config import SETTINGS
from frames import FrameStore
from persona import load_instructions
from plugins import CallMeta, YueyueLLM, YueyueSTT, YueyueTTS
from vision import VisionClient, VisionError
from yueyue import YueyueClient

logger = logging.getLogger("call.agent")

INSTRUCTIONS = load_instructions(SETTINGS.data_dir)


class YueyueAgent(Agent):
    """带视觉工具的通话 Agent。工具由 LLM 标签触发（见 plugins._ToolTagStreamer）。"""

    def __init__(self, *, instructions: str, vision: VisionClient, frames: FrameStore) -> None:
        super().__init__(instructions=instructions)
        self._vision = vision
        self._frames = frames

    @function_tool()
    async def look_at_user(self, question: str = "") -> str:
        """看一眼对方摄像头当前画面，并回答关于画面内容的问题。
        用户问“你能看到我吗/我长什么样/我穿的什么/现在画面里有什么”时使用。"""
        snapshot = self._frames.latest_jpeg()
        if snapshot is None:
            logger.info("vision: tool look_at_user 无画面")
            return "对方摄像头没有可用画面（可能没开摄像头，或视频还没到）。请如实告诉对方你现在看不到。"
        logger.info(
            "vision: tool look_at_user frame_age=%.1fs size=%dx%d",
            snapshot.age,
            snapshot.width,
            snapshot.height,
        )
        try:
            return await self._vision.look_at_user(snapshot.jpeg, question or "你看到了什么？")
        except VisionError as exc:
            logger.warning("vision: tool look_at_user 失败: %s", exc)
            return f"视觉理解失败（{exc}）。请如实告诉对方你暂时看不清。"

    @function_tool()
    async def snapshot_view(self) -> str:
        """截取对方摄像头当前画面并描述场景。需要了解对方环境、在做什么时使用。"""
        snapshot = self._frames.latest_jpeg()
        if snapshot is None:
            logger.info("vision: tool snapshot_view 无画面")
            return "对方摄像头没有可用画面（可能没开摄像头，或视频还没到）。请如实告诉对方你现在看不到。"
        logger.info(
            "vision: tool snapshot_view frame_age=%.1fs size=%dx%d",
            snapshot.age,
            snapshot.width,
            snapshot.height,
        )
        try:
            return await self._vision.snapshot_view(snapshot.jpeg)
        except VisionError as exc:
            logger.warning("vision: tool snapshot_view 失败: %s", exc)
            return f"视觉理解失败（{exc}）。请如实告诉对方你暂时看不清。"


def prewarm(proc: JobProcess) -> None:
    # 每个 job 进程预加载一次 VAD，省掉首次说话的初始化延迟
    proc.userdata["vad"] = silero.VAD.load(
        min_silence_duration=SETTINGS.turn.vad_min_silence,
        prefix_padding_duration=SETTINGS.turn.vad_prefix_padding,
    )


def _parse_metadata(ctx: JobContext) -> dict:
    raw = ""
    try:
        raw = ctx.job.metadata or ""
    except Exception:  # noqa: BLE001
        raw = getattr(ctx.room, "metadata", "") or ""
    if not raw:
        raw = getattr(ctx.room, "metadata", "") or ""
    if not raw:
        return {}
    try:
        data = json.loads(raw)
        return data if isinstance(data, dict) else {}
    except json.JSONDecodeError:
        return {"raw": raw}


_background: set[asyncio.Task] = set()


async def entrypoint(ctx: JobContext) -> None:
    started = time.time()
    await ctx.connect()
    meta = _parse_metadata(ctx)
    logger.info(
        "Agent 入会 room=%s identity=%s metadata=%s",
        ctx.room.name,
        ctx.room.local_participant.identity,
        json.dumps(meta, ensure_ascii=False)[:300],
    )

    ctx.room.on(
        "participant_connected",
        lambda p: logger.info("participant connected: %s", p.identity),
    )
    ctx.room.on(
        "participant_disconnected",
        lambda p: logger.info("participant disconnected: %s", p.identity),
    )

    vad = ctx.proc.userdata.get("vad") or silero.VAD.load()
    client = YueyueClient(SETTINGS.yueyue_url)
    vision = VisionClient(SETTINGS.vision)
    frames = FrameStore(
        max_fps=SETTINGS.video.max_fps,
        max_width=SETTINGS.video.max_width,
        jpeg_quality=SETTINGS.video.jpeg_quality,
    )
    call_meta = CallMeta()
    bargein_state = BargeInState()

    session = AgentSession(
        stt=stt.StreamAdapter(stt=YueyueSTT(client, call_meta), vad=vad),
        vad=vad,
        llm=YueyueLLM(client, call_meta, vision=vision, frames=frames),
        tts=YueyueTTS(client, bargein=bargein_state, duck_gain=SETTINGS.bargein.duck_gain),
        # 边听边说：TTS 播放期间 VAD/ASR 常开。
        # 框架层只兜底“超长连续说话（默认 >=3s）”；短附和/正常插话由 BargeInController
        # 在 final transcript 到达后决策，避免 0.8s 的“嗯”把回复误打断。
        turn_handling={
            "turn_detection": "vad",
            "endpointing": {
                "min_delay": SETTINGS.turn.endpointing_min_delay,
                "max_delay": SETTINGS.turn.endpointing_max_delay,
            },
            "interruption": {
                "enabled": True,
                "mode": "vad",
                "min_duration": SETTINGS.turn.framework_interrupt_min_duration,
                "min_words": 0,
            },
            # 8877 的对话历史由服务端维护；预生成会重复调用 /api/chat 污染历史，
            # 且自定义 LLM 无法复用预生成结果，这里关闭。
            "preemptive_generation": {"enabled": False},
        },
        # 视频帧进入 FrameStore（按需截图），不进入 LLM 上下文
        video_sampler=frames,
    )

    controller = BargeInController(
        session,
        bargein_state,
        SETTINGS.bargein,
        framework_min_duration=SETTINGS.turn.framework_interrupt_min_duration,
    )
    controller.attach()

    def _on_error(event) -> None:
        # 下游（8877 / 视觉模型）不可用时只记录，不让 worker 退出
        logger.error("AgentSession error: %s", getattr(event, "error", event))

    session.on("error", _on_error)

    await session.start(
        agent=YueyueAgent(instructions=INSTRUCTIONS, vision=vision, frames=frames),
        room=ctx.room,
        room_options=RoomOptions(
            audio_input=AudioInputOptions(sample_rate=24000, frame_size_ms=50),
            # 订阅用户摄像头（AI 不发布视频，只出声音）
            video_input=True,
        ),
    )
    logger.info("Agent ready room=%s (%.2fs)", ctx.room.name, time.time() - started)

    async def _greet_when_ready() -> None:
        """等真人入会且双向媒体就绪再打招呼：dispatch 时 agent 常先于用户入会，太早说会扑空。"""
        if not SETTINGS.greeting:
            return
        try:
            participant = await asyncio.wait_for(ctx.wait_for_participant(), timeout=30)
            logger.info("participant 已入会，准备问候: %s", participant.identity)
        except asyncio.TimeoutError:
            logger.info("30s 内无人入会，跳过问候")
            return
        except Exception as exc:  # noqa: BLE001 - 房间提前关闭/断线
            logger.info("等待参与者失败，跳过问候: %s", exc)
            return

        # 双向媒体就绪的近似信号：本端已订阅到对方的麦克风轨道。
        mic_ready = asyncio.Event()

        def _on_track_subscribed(track, publication, p) -> None:
            if p.identity == participant.identity and track.kind == rtc.TrackKind.KIND_AUDIO:
                mic_ready.set()

        ctx.room.on("track_subscribed", _on_track_subscribed)
        try:
            await asyncio.wait_for(mic_ready.wait(), timeout=6.0)
        except asyncio.TimeoutError:
            logger.info("6s 内未订阅到用户麦克风轨道，仍按计划问候")
        finally:
            ctx.room.off("track_subscribed", _on_track_subscribed)
        await asyncio.sleep(0.8)  # 再给客户端留出订阅本端音频轨道的时间
        try:
            session.say(SETTINGS.greeting, allow_interruptions=True)
        except Exception as exc:  # noqa: BLE001 - TTS 没起来也不影响通话本身
            logger.warning("打招呼失败（TTS 可能未启动）: %s", exc)

    greet_task = asyncio.create_task(_greet_when_ready())
    _background.add(greet_task)
    greet_task.add_done_callback(_background.discard)

    async def _shutdown(reason: str) -> None:
        logger.info(
            "Agent 退出 room=%s reason=%s video=%s",
            ctx.room.name,
            reason,
            frames.stats,
        )
        try:
            await session.aclose()
        except Exception as exc:  # noqa: BLE001
            logger.warning("session.aclose 异常: %s", exc)
        await frames.aclose()
        await vision.aclose()
        await client.aclose()

    ctx.add_shutdown_callback(_shutdown)


def main() -> None:
    # 重定向到文件时保持行缓冲，测试脚本/排障能实时看到日志
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(line_buffering=True, encoding="utf-8", errors="replace")
        except Exception:  # noqa: BLE001 - 某些环境不支持 reconfigure
            pass
    logging.basicConfig(
        level=getattr(logging, SETTINGS.log_level, logging.INFO),
        format="%(asctime)s %(levelname)s %(name)s %(message)s",
    )
    if not SETTINGS.livekit_key or not SETTINGS.livekit_secret:
        raise SystemExit(
            "LiveKit key/secret 未配置：检查 config.json 的 livekit_plugin_config，"
            "或设置 LIVEKIT_KEY / LIVEKIT_SECRET 环境变量"
        )
    logger.info(
        "worker 启动 agent_name=%s livekit=%s yueyue=%s data_dir=%s vision=%s/%s port=%s",
        SETTINGS.agent_name,
        SETTINGS.livekit_url,
        SETTINGS.yueyue_url,
        SETTINGS.data_dir,
        SETTINGS.vision.provider,
        SETTINGS.vision.model,
        SETTINGS.worker_http_port,
    )
    cli.run_app(
        WorkerOptions(
            entrypoint_fnc=entrypoint,
            prewarm_fnc=prewarm,
            agent_name=SETTINGS.agent_name,
            ws_url=SETTINGS.livekit_url,
            api_key=SETTINGS.livekit_key,
            api_secret=SETTINGS.livekit_secret,
            port=SETTINGS.worker_http_port,
            num_idle_processes=1,
            # 单通视频通话时 CPU load 可能短暂超过 0.7，默认阈值会让 worker 被标记
            # unavailable 导致新 dispatch 排队；放宽到 0.95，保证 QA 连续建房稳定。
            load_threshold=0.95,
            log_level=SETTINGS.log_level,
        )
    )


if __name__ == "__main__":
    main()
