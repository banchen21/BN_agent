"""LiveKit 服务端封装：建房、签发客户端 token、显式 dispatch Agent、删房。"""
from __future__ import annotations

import json
import logging
import re
import time
from datetime import timedelta

from livekit import api

from config import Settings

logger = logging.getLogger("ai-call.livekit")

_PREFIX = "ai-call"


def _safe_identity(value: str) -> str:
    """OpenIM userID 一般是字母数字下划线，这里再兜一层，避免非法房间字符。"""
    cleaned = re.sub(r"[^0-9A-Za-z_.-]", "_", value).strip("_")
    return cleaned or "user"


class LiveKitService:
    def __init__(self, settings: Settings) -> None:
        self._s = settings
        self._client: api.LiveKitAPI | None = None

    async def start(self) -> None:
        self._client = api.LiveKitAPI(
            self._s.livekit_url, self._s.livekit_key, self._s.livekit_secret
        )
        logger.info("LiveKit API client -> %s", self._s.livekit_url)

    async def aclose(self) -> None:
        if self._client is not None:
            await self._client.aclose()
            self._client = None

    @property
    def client(self) -> api.LiveKitAPI:
        if self._client is None:
            raise RuntimeError("LiveKit client 尚未初始化")
        return self._client

    # ---------------------------------------------------------------- 房间
    def new_room_name(self, user_id: str) -> str:
        return f"{self._s.room_prefix}-{_safe_identity(user_id)}-{int(time.time() * 1000)}"

    async def create_room(self, room: str, media_type: str) -> None:
        await self.client.room.create_room(
            api.CreateRoomRequest(
                name=room,
                empty_timeout=self._s.room_empty_timeout,
                departure_timeout=self._s.room_departure_timeout,
                max_participants=8,
                metadata=json.dumps({"mediaType": media_type}, ensure_ascii=False),
            )
        )

    async def delete_room(self, room: str) -> None:
        await self.client.room.delete_room(api.DeleteRoomRequest(room=room))

    # ---------------------------------------------------------------- token
    def build_user_token(self, room: str, identity: str, name: str | None = None) -> str:
        grants = api.VideoGrants(
            room_join=True,
            room=room,
            can_publish=True,
            can_subscribe=True,
            can_publish_data=True,
        )
        return (
            api.AccessToken(self._s.livekit_key, self._s.livekit_secret)
            .with_identity(identity)
            .with_name(name or identity)
            .with_ttl(timedelta(seconds=self._s.token_ttl_seconds))
            .with_grants(grants)
            .to_jwt()
        )

    # ---------------------------------------------------------------- dispatch
    async def dispatch_agent(self, room: str, media_type: str) -> str:
        """显式派发名为 agent_name 的 worker；返回 dispatch id（可能为空）。"""
        info = await self.client.agent_dispatch.create_dispatch(
            api.CreateAgentDispatchRequest(
                agent_name=self._s.agent_name,
                room=room,
                metadata=json.dumps({"mediaType": media_type}, ensure_ascii=False),
            )
        )
        return getattr(info, "id", "") or ""
