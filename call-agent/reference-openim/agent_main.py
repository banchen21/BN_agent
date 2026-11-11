"""悦悦 LiveKit Agent：显式 dispatch（agent_name="yueyue"）入会。

启动：
    .venv\\Scripts\\python.exe agent_main.py start

链路：房间音频 -> Silero VAD 切句 -> /api/asr -> /api/chat(SSE) -> /api/tts_stream(SSE)
      -> 房间音频；用户开口时取消当前回复（打断）。
"""
from __future__ import annotations

import asyncio
import logging

from livekit.agents import (
    Agent,
    AgentSession,
    JobContext,
    JobProcess,
    WorkerOptions,
    cli,
    stt,
)
from livekit.agents.voice.room_io import AudioInputOptions, RoomOptions
from livekit.plugins import silero

from bargein import BargeInController, BargeInState
from config import SETTINGS
from plugins import CallMeta, YueyueLLM, YueyueSTT, YueyueTTS
from yueyue import YueyueClient

logger = logging.getLogger("ai-call.agent")

INSTRUCTIONS = (
    "你是齐悦，正在和对方打电话。用口语、短句说话，像真人接电话一样自然；"
    "只说你嘴里说出来的话，不要旁白和动作描写。"
)


def prewarm(proc: JobProcess) -> None:
    # 每个 job 进程预加载一次 VAD，省掉首次说话的初始化延迟
    proc.userdata["vad"] = silero.VAD.load(
        min_silence_duration=0.45,
        prefix_padding_duration=0.4,
    )


async def entrypoint(ctx: JobContext) -> None:
    await ctx.connect()
    logger.info(
        "Agent 入会 room=%s mediaType=%s",
        ctx.room.name,
        getattr(ctx.job, "metadata", "") or getattr(ctx.room, "metadata", "") or "-",
    )

    vad = ctx.proc.userdata.get("vad") or silero.VAD.load()
    client = YueyueClient(SETTINGS.yueyue_api)
    meta = CallMeta()
    recognizer = YueyueSTT(client, meta)
    bargein_state = BargeInState()

    session = AgentSession(
        stt=stt.StreamAdapter(stt=recognizer, vad=vad),
        vad=vad,
        llm=YueyueLLM(client, meta),
        tts=YueyueTTS(client, bargein_state),
        # 边听边说：TTS 播放期间 VAD/ASR 常开。
        # min_duration 放宽到 0.8s，让短附和（嗯/对/好）不触发 VAD 打断；
        # 真正的插话由 BargeInController 在转写层补打断并记录 bargein 日志。
        turn_handling={
            "turn_detection": "vad",
            "endpointing": {"min_delay": 0.3, "max_delay": 3.0},
            "interruption": {
                # 框架层只处理“超长连续说话”（>=2.5s）的兜底打断；
                # 短附和（嗯/对）和正常插话（等一下…）都由 BargeInController
                # 在转写分类后调用 session.interrupt()，避免 0.8s 的「嗯」误打断。
                "enabled": True,
                "mode": "vad",
                "min_duration": 2.5,
                "min_words": 0,
            },
        },
    )
    BargeInController(session, bargein_state).attach()

    def _on_error(event) -> None:
        # 下游（8877/9880）不可用时只记录，不让 worker 退出
        logger.error("AgentSession error: %s", getattr(event, "error", event))

    session.on("error", _on_error)

    await session.start(
        agent=Agent(instructions=INSTRUCTIONS),
        room=ctx.room,
        # 视频通话时只订阅用户的摄像头，AI 不发布视频（输出只有音频）
        room_options=RoomOptions(
            audio_input=AudioInputOptions(sample_rate=24000, frame_size_ms=50),
            video_input=True,
        ),
    )

    if SETTINGS.greeting:
        try:
            session.say(SETTINGS.greeting, allow_interruptions=True)
        except Exception as exc:  # TTS 没起来也不影响通话本身
            logger.warning("打招呼失败（TTS 可能未启动）: %s", exc)

    async def _shutdown(reason: str) -> None:
        logger.info("Agent 退出 room=%s reason=%s", ctx.room.name, reason)
        await session.aclose()
        await client.aclose()

    ctx.add_shutdown_callback(_shutdown)


def main() -> None:
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s %(message)s",
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
            log_level="INFO",
        )
    )


if __name__ == "__main__":
    main()
