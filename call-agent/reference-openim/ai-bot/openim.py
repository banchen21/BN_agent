"""OpenIM REST 封装：管理员 token、用户 token、注册、好友、发消息。"""
from __future__ import annotations

import base64
import json
import logging
import threading
import time
import uuid
from typing import Any

import requests

log = logging.getLogger("ai-bot.openim")

# OpenIM 常见的「已经做过」错误码（注册重复 / 好友已存在）
BENIGN_ERR_CODES = {1002, 1005, 1006, 1007, 1102, 1201, 1202, 1301}


class OpenIMError(RuntimeError):
    def __init__(self, path: str, err_code: Any, err_msg: str, err_dlt: str = "") -> None:
        super().__init__(f"{path} errCode={err_code} errMsg={err_msg} errDlt={err_dlt}")
        self.path = path
        self.err_code = err_code
        self.err_msg = err_msg
        self.err_dlt = err_dlt


def _jwt_exp(token: str) -> int:
    """读取 JWT 的 exp（OpenIM 的 token 是标准 JWT）。"""
    try:
        payload = token.split(".")[1]
        payload += "=" * (-len(payload) % 4)
        data = json.loads(base64.urlsafe_b64decode(payload))
        return int(data.get("exp") or 0)
    except Exception:  # noqa: BLE001 - token 不是 JWT 时按不缓存处理
        return 0


class OpenIMClient:
    def __init__(self, api_base: str, secret: str, admin_user: str) -> None:
        self.api_base = api_base.rstrip("/")
        self.secret = secret
        self.admin_user = admin_user
        self._http = requests.Session()
        # 宿主机上的系统代理会劫持 127.0.0.1 请求，必须绕开
        self._http.trust_env = False
        self._lock = threading.Lock()
        self._admin_token = ""
        self._admin_exp = 0.0
        self._user_tokens: dict[str, tuple[str, float]] = {}

    # ---------------------------------------------------------------- 基础请求
    def _post(
        self,
        path: str,
        body: dict | None = None,
        *,
        token: str | None = None,
        timeout: float = 30.0,
        tolerate: bool = False,
    ) -> Any:
        headers = {
            "Content-Type": "application/json",
            "operationID": str(uuid.uuid4()),
        }
        if token:
            headers["token"] = token
        url = f"{self.api_base}{path}"
        try:
            resp = self._http.post(url, json=body or {}, headers=headers, timeout=timeout)
            resp.raise_for_status()
            data = resp.json()
        except requests.RequestException as exc:
            raise OpenIMError(path, "network", str(exc)) from exc
        except ValueError as exc:
            raise OpenIMError(path, "bad-json", str(exc)) from exc

        err_code = data.get("errCode", 0)
        if err_code not in (0, None):
            if tolerate:
                log.info("OpenIM %s 返回非零但忽略：%s", path, data)
            else:
                raise OpenIMError(
                    path, err_code, str(data.get("errMsg", "")), str(data.get("errDlt", ""))
                )
        return data.get("data")

    # ---------------------------------------------------------------- token
    def admin_token(self) -> str:
        with self._lock:
            now = time.time()
            if self._admin_token and now < self._admin_exp - 120:
                return self._admin_token
            data = self._post(
                "/auth/get_admin_token",
                {"secret": self.secret, "userID": self.admin_user},
            )
            token = (data or {}).get("token", "")
            if not token:
                raise OpenIMError("/auth/get_admin_token", "empty", "admin token 为空")
            exp = _jwt_exp(token)
            ttl = float((data or {}).get("expireTimeSeconds") or 0)
            self._admin_token = token
            self._admin_exp = exp or (now + (ttl or 3600))
            return token

    def user_token(self, user_id: str) -> str:
        with self._lock:
            now = time.time()
            cached = self._user_tokens.get(user_id)
            if cached and now < cached[1] - 120:
                return cached[0]
        # 不在锁内发请求，避免管理员 token 刷新时互相等待
        data = self._post(
            "/auth/get_user_token",
            {"secret": self.secret, "userID": user_id, "platformID": 5},
        )
        token = (data or {}).get("token", "")
        if not token:
            raise OpenIMError("/auth/get_user_token", "empty", f"{user_id} token 为空")
        exp = _jwt_exp(token) or (time.time() + 3600)
        with self._lock:
            self._user_tokens[user_id] = (token, exp)
        return token

    # ---------------------------------------------------------------- 账号 / 好友
    def ensure_user(self, user_id: str, nickname: str, password: str) -> None:
        """注册用户；已存在时静默通过。"""
        body = {
            "users": [
                {
                    "userID": user_id,
                    "nickname": nickname,
                    "faceURL": "",
                    "ex": "",
                    "createTime": 0,
                    "appMangerLevel": 0,
                    "globalRecvMsgOpt": 0,
                    "password": password,
                }
            ]
        }
        try:
            self._post("/user/user_register", body, token=self.admin_token(), timeout=20)
            log.info("已注册 OpenIM 用户 %s", user_id)
        except OpenIMError as exc:
            text = f"{exc.err_msg} {exc.err_dlt}".lower()
            if exc.err_code in BENIGN_ERR_CODES or "exist" in text or "already" in text:
                log.info("OpenIM 用户 %s 已存在，跳过注册", user_id)
                return
            raise

    def update_user_info(
        self, user_id: str, nickname: str, face_url: str = "", ex: str = ""
    ) -> None:
        """修正昵称等信息（注册接口不会覆盖已存在用户）。"""
        self._post(
            "/user/update_user_info",
            {"userInfo": {"userID": user_id, "nickname": nickname, "faceURL": face_url, "ex": ex}},
            token=self.admin_token(),
            timeout=20,
            tolerate=True,
        )

    def get_friend_ids(self, user_id: str, page_size: int = 500) -> set[str]:
        """取某个用户已存在的好友 userID 集合。"""
        ids: set[str] = set()
        page = 1
        while True:
            data = self._post(
                "/friend/get_friend_list",
                {
                    "userID": user_id,
                    "pagination": {"pageNumber": page, "showNumber": page_size},
                },
                token=self.admin_token(),
                timeout=20,
            ) or {}
            items = data.get("friendsInfo") or []
            for item in items:
                friend_id = (item.get("friendUser") or {}).get("userID")
                if friend_id:
                    ids.add(str(friend_id))
            if len(items) < page_size or page >= 20:
                break
            page += 1
        return ids

    def import_friend(self, owner_id: str, friend_ids: list[str]) -> None:
        self._post(
            "/friend/import_friend",
            {"ownerUserID": owner_id, "friendUserIDs": friend_ids},
            token=self.admin_token(),
            timeout=20,
            tolerate=True,
        )

    # ---------------------------------------------------------------- 发消息
    def send_msg(
        self,
        send_id: str,
        recv_id: str,
        content: dict,
        content_type: int,
        *,
        session_type: int = 1,
    ) -> Any:
        """以 send_id 的身份给 recv_id 发单聊消息（管理员 token 允许代发）。"""
        body = {
            "sendID": send_id,
            "recvID": recv_id,
            "groupID": "",
            "senderPlatformID": 5,
            "content": content,
            "contentType": int(content_type),
            "sessionType": int(session_type),
            "msgFrom": 100,
            "isOnlineOnly": False,
            "notOfflinePush": False,
        }
        return self._post("/msg/send_msg", body, token=self.admin_token(), timeout=30)

    def send_text(self, send_id: str, recv_id: str, text: str) -> Any:
        return self.send_msg(send_id, recv_id, {"content": text}, 101)

    # ---------------------------------------------------------------- 上传（备用通道）
    def initiate_form_data(self, user_id: str, name: str, size: int, content_type: str) -> dict:
        return self._post(
            "/object/initiate_form_data",
            {"name": name, "size": size, "contentType": content_type, "group": "msg"},
            token=self.user_token(user_id),
            timeout=30,
        )

    def complete_form_data(self, form_id: str) -> dict:
        return self._post("/object/complete_form_data", {"id": form_id}, token=self.admin_token(), timeout=60)


def picture_elem(url: str, size: int, width: int, height: int, ext: str = "png") -> dict:
    """OpenIM PictureElem：source/big/snapshot 三份都必填，缺一个校验就 500。"""
    def part() -> dict:
        return {
            "uuid": url,
            "type": ext,
            "size": int(size),
            "width": int(width),
            "height": int(height),
            "url": url,
        }

    return {
        "sourcePicture": part(),
        "bigPicture": part(),
        "snapshotPicture": part(),
    }


def sound_elem(url: str, size: int, duration_seconds: int) -> dict:
    duration = max(1, int(round(duration_seconds)))
    return {
        "uuid": url,
        "soundPath": "",
        "sourceUrl": url,
        "dataSize": int(size),
        "duration": duration,
    }
