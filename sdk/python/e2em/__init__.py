"""Small explicit local client. No downloads, model imports or remote transport."""
from __future__ import annotations
import asyncio
import copy
import hashlib
import hmac
import json
import os
from pathlib import Path
import secrets
import re
import socket
import stat
import struct
from dataclasses import dataclass, field
from typing import Any

from .schema import SCHEMA, valid
from .types import Request as RequestValue, Policy as PolicyValue, Capabilities, PolicyRef
from .message import request as message_request, PRESETS

MAX_FRAME = 131072
class E2EMError(Exception):
    def __init__(self, code: str):
        super().__init__(code)
        self.code = code
        self.action = "review"

def _proof(secret: str, role: str, provider: str, principal: str, client: str, server: str) -> str:
    return hmac.new(secret.encode(), b"".join(v.encode() + b"\0" for v in (role, provider, principal, client, server)), hashlib.sha256).hexdigest()

def _snapshot(request: dict) -> str:
    return hashlib.sha256(json.dumps(request, sort_keys=True, ensure_ascii=False).encode()).hexdigest()

def utf16_span(text: str, start: int, end: int) -> tuple[int, int]:
    raw = text.encode()
    if not 0 <= start <= end <= len(raw):
        raise E2EMError("INVALID_REQUEST")
    try:
        return (len(raw[:start].decode().encode("utf-16-le")) // 2,
                len(raw[:end].decode().encode("utf-16-le")) // 2)
    except UnicodeError as exc:
        raise E2EMError("INVALID_REQUEST") from exc

@dataclass(frozen=True)
class Assessment:
    _value: dict[str, Any]
    snapshot: str
    _request: dict[str, Any] = field(default_factory=dict, repr=False)
    @property
    def value(self) -> dict[str, Any]: return copy.deepcopy(self._value)
    @property
    def status(self) -> str: return self._value["status"]
    @property
    def action(self) -> str: return self._value["action"]
    @property
    def scores(self) -> dict[str, float]:
        return {finding["category"] or finding["rule_id"]: finding["score"]
                for finding in self._value["findings"] if finding["score"] is not None}
    @property
    def request(self) -> dict[str, Any]: return copy.deepcopy(self._request)
    @property
    def unevaluated(self) -> list[str]:
        rules = {rule["id"]: rule.get("category") or rule["id"]
                 for rule in self._request.get("policy", {}).get("rules", [])}
        return [rules.get(rule, rule) for rule in self._value["coverage"]["unevaluated_rules"]]
    def applies_to(self, request: dict) -> bool: return self.snapshot == _snapshot(request)


def _assessment(value: Any, request: dict) -> Assessment:
    try:
        if not valid(value, SCHEMA["$defs"]["Assessment"]): raise ValueError()
        required = {"api_version", "request_id", "message_id", "message_revision", "status", "action", "versions", "coverage", "findings", "reason_codes", "duration_ms"}
        if not isinstance(value, dict) or not required <= value.keys() or value.keys() - required - {"error_code"}:
            raise ValueError()
        if value["api_version"] != "0.1" or value["request_id"] != request["request_id"] or value["message_id"] != request["message"]["id"] or value["message_revision"] != request["message"]["revision"]:
            raise ValueError()
        selected = request.get("policy") or request.get("policy_ref")
        if value["status"] in ("assessed", "indeterminate") and (value["versions"]["policy_id"] != selected["id"] or value["versions"]["policy_version"] != selected["version"]): raise ValueError()
        if value["status"] not in ("assessed", "indeterminate", "error", "cancelled") or value["action"] not in ("allow", "warn", "review", "block"):
            raise ValueError()
        if value["status"] != "assessed" and value["action"] != "review": raise ValueError()
        if value["status"] == "error" and not isinstance(value.get("error_code"), str): raise ValueError()
        coverage = value["coverage"]
        if value["status"] == "assessed" and (coverage["target_complete"] is not True or coverage["context_complete"] is not True or coverage["unevaluated_rules"]): raise ValueError()
        if not isinstance(value["versions"], dict) or not isinstance(value["reason_codes"], list): raise ValueError()
        texts = {request["message"]["id"]: request["message"]["text"]}
        texts.update({turn["id"]: turn["text"] for turn in request.get("context", [])})
        for finding in value["findings"]:
            if finding["method"] not in ("deterministic", "model", "custom_policy"): raise ValueError()
            score = finding["score"]
            if score is not None and (type(score) not in (float, int) or not 0 <= score <= 1): raise ValueError()
            for span in finding["spans"]:
                if type(span["start"]) is not int or type(span["end"]) is not int: raise ValueError()
                utf16_span(texts[span["message_id"]], span["start"], span["end"])
    except (KeyError, TypeError, ValueError, E2EMError) as exc:
        raise E2EMError("INTERNAL_ERROR") from exc
    return Assessment(copy.deepcopy(value), _snapshot(request), copy.deepcopy(request))


def _connection(app="my-app", config_path=None):
    """Load installer-managed local app settings without exposing secrets to callers."""
    try:
        if not isinstance(app, str) or not re.fullmatch(r"[a-zA-Z0-9_-]{1,64}", app):
            raise ValueError()
        path = Path(config_path) if config_path is not None else (
            Path(os.environ["LOCALAPPDATA"]) / "E2EM" / f"app-{app}.json"
            if os.name == "nt" else Path.home() / ".config/e2em/apps" / f"{app}.json"
        )
        descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
        with os.fdopen(descriptor, "r", encoding="utf-8") as stream:
            metadata = os.fstat(stream.fileno())
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > 16384:
                raise ValueError()
            if os.name == "posix" and (metadata.st_uid != os.getuid() or metadata.st_mode & 0o077):
                raise ValueError()
            config = json.load(stream)
        if not isinstance(config, dict):
            raise ValueError()
        fields = ("socket_path", "principal", "secret", "provider")
        if any(not isinstance(config.get(key), str) or not config[key] for key in fields):
            raise ValueError()
        if config["principal"] != app:
            raise ValueError()
        return {key: config[key] for key in fields}
    except (OSError, ValueError, KeyError, TypeError) as exc:
        raise E2EMError("MODEL_UNAVAILABLE") from exc

class Client:
    def __init__(self):
        self._reader = None
        self._writer = None
        self._read_task = None
        self._pending = {}
        self._write_lock = asyncio.Lock()
        self.provider = None
        self.capability_manifest = None
        self.model = None
        self._counter = 0
        self._closed = False

    @classmethod
    async def connect(cls, app: str = "my-app", *, config_path=None, model=None) -> Client:
        return await cls.open(**_connection(app, config_path), model=model)

    @classmethod
    async def open(cls, socket_path: str, principal: str, secret: str, provider: str, model=None) -> Client:
        client = cls()
        try:
            if os.name == "nt":
                if not re.fullmatch(r"\\\\\.\\pipe\\e2em-[A-Za-z0-9-]+", socket_path) or len(socket_path) > 200:
                    raise E2EMError("MODEL_UNAVAILABLE")
                loop = asyncio.get_running_loop()
                reader = asyncio.StreamReader()
                protocol = asyncio.StreamReaderProtocol(reader)
                transport, _ = await asyncio.wait_for(loop.create_pipe_connection(lambda: protocol, socket_path), 5)
                client._reader = reader
                client._writer = asyncio.StreamWriter(transport, protocol, reader, loop)
            elif os.name == "posix":
                path = Path(socket_path)
                for target, is_socket in ((path.parent, False), (path, True)):
                    metadata = target.lstat()
                    if metadata.st_uid != os.getuid() or metadata.st_mode & 0o077 or (not stat.S_ISSOCK(metadata.st_mode) if is_socket else not stat.S_ISDIR(metadata.st_mode)):
                        raise E2EMError("MODEL_UNAVAILABLE")
                client._reader, client._writer = await asyncio.wait_for(asyncio.open_unix_connection(socket_path), 5)
                if hasattr(socket, "SO_PEERCRED"):
                    peer = client._writer.get_extra_info("socket").getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12)
                    if struct.unpack("3i", peer)[1] != os.getuid(): raise E2EMError("MODEL_UNAVAILABLE")
            else:
                raise E2EMError("MODEL_UNAVAILABLE")
            nonce = secrets.token_hex(32)
            await client._send({"principal": principal, "nonce": nonce})
            challenge = await asyncio.wait_for(client._frame(), 5)
            if set(challenge) != {"provider", "nonce", "proof"} or challenge["provider"] != provider or len(challenge["nonce"]) != 64 or not hmac.compare_digest(challenge["proof"], _proof(secret, "server", provider, principal, nonce, challenge["nonce"])):
                raise E2EMError("MODEL_UNAVAILABLE")
            await client._send({"proof": _proof(secret, "client", provider, principal, nonce, challenge["nonce"])})
            authenticated = await asyncio.wait_for(client._frame(), 5)
            if authenticated != {"authenticated": True, "provider": provider}: raise E2EMError("MODEL_UNAVAILABLE")
            client.provider = provider
            client._read_task = asyncio.create_task(client._receive())
            caps = await client.capabilities()
            if caps["api_version"] != "0.1" or not caps["backend_ready"]: raise E2EMError("MODEL_UNAVAILABLE")
            client.capability_manifest = caps
            client.model = model
            return client
        except asyncio.CancelledError:
            await client.close()
            raise
        except Exception as exc:
            await client.close()
            if isinstance(exc, E2EMError): raise
            raise E2EMError("MODEL_UNAVAILABLE") from exc

    @classmethod
    async def discover(cls, candidates: list[dict], required_detectors: list[str], pinned: str | None = None) -> Client:
        for candidate in sorted(candidates, key=lambda c: {"native": 0, "project": 1, "embedded": 2}[c["kind"]]):
            if pinned and candidate["provider"] != pinned: continue
            try:
                client = await cls.open(candidate["socket_path"], candidate["principal"], candidate["secret"], candidate["provider"])
                if all(d in client.capability_manifest["detectors"] for d in required_detectors): return client
                await client.close()
            except (OSError, E2EMError, asyncio.TimeoutError):
                continue
        raise E2EMError("MODEL_UNAVAILABLE")

    async def _frame(self):
        size = struct.unpack(">I", await self._reader.readexactly(4))[0]
        if not 0 < size <= MAX_FRAME: raise E2EMError("INTERNAL_ERROR")
        def no_constant(_): raise ValueError("invalid JSON constant")
        try: return json.loads((await self._reader.readexactly(size)).decode(), parse_constant=no_constant)
        except (UnicodeError, ValueError) as exc: raise E2EMError("INTERNAL_ERROR") from exc

    async def _send(self, value):
        try: encoded = json.dumps(value, ensure_ascii=False, allow_nan=False).encode()
        except (TypeError, ValueError, UnicodeError) as exc: raise E2EMError("INVALID_REQUEST") from exc
        if not 0 < len(encoded) <= MAX_FRAME: raise E2EMError("INVALID_REQUEST")
        async with self._write_lock:
            self._writer.write(struct.pack(">I", len(encoded)) + encoded)
            await self._writer.drain()

    async def _receive(self):
        try:
            while True:
                response = await self._frame()
                if not valid(response): raise E2EMError("INTERNAL_ERROR")
                future = self._pending.get(response["call_id"])
                if future and not future.done(): future.set_result(response["reply"])
        except Exception:
            for future in self._pending.values():
                if not future.done(): future.set_exception(E2EMError("MODEL_UNAVAILABLE"))
            self._closed = True

    async def _call(self, operation: dict, expected: str, timeout: float = 5):
        if self._closed or not self._writer: raise E2EMError("MODEL_UNAVAILABLE")
        if len(self._pending) >= 8: raise E2EMError("RESOURCE_EXHAUSTED")
        self._counter += 1
        call_id = str(self._counter)
        future = asyncio.get_running_loop().create_future()
        self._pending[call_id] = future
        try:
            await self._send({"call_id": call_id, "api_version": "0.1", "operation": operation})
            reply = await asyncio.wait_for(future, timeout)
            if not isinstance(reply, dict): raise E2EMError("INTERNAL_ERROR")
            if reply.get("kind") == "error": raise E2EMError(reply.get("error_code", "INTERNAL_ERROR"))
            if reply.get("kind") != expected: raise E2EMError("INTERNAL_ERROR")
            return reply
        except asyncio.TimeoutError as exc:
            raise E2EMError("DEADLINE_EXCEEDED") from exc
        finally:
            self._pending.pop(call_id, None)

    async def capabilities(self) -> Capabilities:
        return (await self._call({"op": "capabilities"}, "capabilities"))["capabilities"]
    async def validate_policy(self, policy: PolicyValue) -> PolicyRef:
        return (await self._call({"op": "validate_policy", "policy": policy}, "policy"))["policy_ref"]
    async def models(self):
        return await self._call({"op": "models"}, "models")
    async def install_model(self, source: str, *, name="custom", auto_update=False):
        return await self._call({"op": "install_model", "source": source, "name": name, "auto_update": auto_update}, "models", 900)
    async def assess(self, request: RequestValue | str, *, context=None, policies=None, custom_policies=None, model=None, deadline_ms=15000) -> Assessment:
        model = self.model if model is None else model
        if isinstance(request, str):
            try: request = message_request(request, context=context, policies=policies, custom_policies=custom_policies, model=model, deadline_ms=deadline_ms)
            except (TypeError, ValueError) as exc: raise E2EMError("INVALID_REQUEST") from exc
        elif context is not None or policies is not None or custom_policies is not None:
            raise E2EMError("INVALID_REQUEST")
        snapshot = copy.deepcopy(request)
        if model is not None:
            snapshot.setdefault("options", {}).setdefault("model", model)
        try:
            reply = await self._call({"op": "assess", "request": snapshot}, "assessment", snapshot.get("options", {}).get("deadline_ms", 15000)/1000 + 1)
            return _assessment(reply["assessment"], snapshot)
        except (asyncio.CancelledError, E2EMError):
            if not self._closed:
                try: await asyncio.shield(self.cancel(snapshot["request_id"]))
                except E2EMError: pass
            raise
    async def cancel(self, request_id: str) -> bool:
        return (await self._call({"op": "cancel", "request_id": request_id}, "cancelled"))["accepted"]
    async def close(self):
        self._closed = True
        if self._read_task:
            self._read_task.cancel()
            try: await self._read_task
            except asyncio.CancelledError: pass
        for future in self._pending.values():
            if not future.done(): future.set_exception(E2EMError("MODEL_UNAVAILABLE"))
        if self._writer:
            self._writer.close()
            try: await self._writer.wait_closed()
            except (OSError, ConnectionError): pass
    async def __aenter__(self): return self
    async def __aexit__(self, *_): await self.close()


class BlockingClient:
    """Optional blocking facade; owns one dedicated asyncio loop thread."""
    def __init__(self, *, app="my-app", config_path=None, model=None, **connection):
        import threading
        self._loop = asyncio.new_event_loop()
        self._thread = threading.Thread(target=self._loop.run_forever, daemon=True)
        self._thread.start()
        self._disposed = False
        try: self._client = self._run(Client.open(**connection, model=model) if connection else Client.connect(app, config_path=config_path, model=model))
        except BaseException:
            self._loop.call_soon_threadsafe(self._loop.stop)
            self._thread.join()
            self._loop.close()
            raise
    def _run(self, coroutine, timeout=35):
        if self._disposed:
            coroutine.close()
            raise E2EMError("MODEL_UNAVAILABLE")
        return asyncio.run_coroutine_threadsafe(coroutine, self._loop).result(timeout=timeout)
    def capabilities(self): return self._run(self._client.capabilities())
    def models(self): return self._run(self._client.models())
    def install_model(self, source, **options): return self._run(self._client.install_model(source, **options), timeout=905)
    def validate_policy(self, policy): return self._run(self._client.validate_policy(policy))
    def assess(self, request, **options): return self._run(self._client.assess(request, **options))
    def cancel(self, request_id): return self._run(self._client.cancel(request_id))
    def close(self):
        if self._disposed: return
        try: self._run(self._client.close())
        finally:
            self._disposed = True
            self._loop.call_soon_threadsafe(self._loop.stop)
            self._thread.join()
            self._loop.close()
    def __enter__(self): return self
    def __exit__(self, *_): self.close()


def assess(message: str, *, context=None, policies=None, custom_policies=None, app="my-app", config_path=None, model=None, deadline_ms=15000) -> Assessment:
    """Send just a message. Default presets and local app settings are automatic."""
    with BlockingClient(app=app, config_path=config_path, model=model) as client:
        return client.assess(message, context=context, policies=policies, custom_policies=custom_policies, deadline_ms=deadline_ms)
