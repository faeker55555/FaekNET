#!/usr/bin/env python3
"""
lan_mesh messenger -- a self-hosted, Discord-styled text + voice channel,
meant to be run by one person on your mesh and reached by everyone else via
their browser at https://<your virtual mesh IP>:<port>/

No external dependencies (Python 3 standard library only, no pip installs
needed anywhere, other than relying on the `openssl` CLI being present to
generate a TLS certificate). Single text channel, single voice channel,
lightweight accounts, per-user settings, push-to-talk, and the full
standard Unicode emoji set in a Discord-style categorized picker.

On accounts: since everyone who can reach this server already has your
mesh's shared encryption key, an account here is NOT an access-control
boundary against strangers -- it's a lightweight, persistent identity
(like registering a nickname) so your name/avatar/settings follow you
across sessions and devices, and so nobody else can casually post as you.
Passwords are hashed (PBKDF2-HMAC-SHA256, salted) before being stored --
never in plaintext -- but this is not meant to withstand a determined
attacker who already has full access to your mesh; it's meant to stop
casual impersonation among friends.

Voice/GIF-picker notes:
  - Voice audio is relayed through this server as raw PCM over a hand-rolled
    WebSocket (no external STUN/TURN/WebRTC signaling service involved --
    it rides on the same "no third-party services" design as the rest of
    lan_mesh). Quality is modest (16kHz mono) by design, to keep bandwidth
    reasonable alongside whatever game you're also playing over the mesh.
  - Browsers only allow microphone access on a "secure context" (HTTPS, or
    localhost). Since peers open this over your virtual mesh IP (not
    localhost), this script generates a self-signed TLS certificate on
    first run and serves over HTTPS so voice chat actually works. Your
    browser will show a one-time "not trusted" warning for the self-signed
    cert -- that's expected, click through it (Advanced -> Proceed). Your
    real confidentiality already comes from the mesh's own encryption
    underneath; this cert exists only to satisfy the browser's mic-access
    security check.
  - "GIF support" here means: paste/type a link to a .gif/.png/.jpg/.webp
    and it auto-embeds, and you can drag-and-drop or paste an image/GIF
    file to upload and share it directly. There's no built-in GIF search
    (that would require calling a third-party API like Tenor/Giphy, which
    breaks the "no external services" design) -- find a GIF anywhere else
    and paste/upload it here.
  - The emoji picker uses the real, complete standard Unicode emoji set
    (the same underlying characters Discord's picker offers), rendered by
    your OS/browser's native emoji font -- not Discord's proprietary
    Twemoji-style artwork, and this project has no affiliation with
    Discord. The picker's category layout mirrors Discord's for
    familiarity.

Usage:
    python3 messenger.py                       # HTTPS on 0.0.0.0:8765
    python3 messenger.py --host 10.66.0.1 --port 8765
    python3 messenger.py --no-tls              # plain HTTP (voice/mic won't work; text/images still do)
"""
import argparse
import base64
import hashlib
import hmac
import html
import json
import mimetypes
import os
import re
import secrets
import socket
import ssl
import struct
import subprocess
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs, unquote

BASE_DIR = os.path.dirname(os.path.abspath(__file__))
HISTORY_FILE = os.path.join(BASE_DIR, "messenger_history.jsonl")
UPLOADS_DIR = os.path.join(BASE_DIR, "messenger_uploads")
CERTS_DIR = os.path.join(BASE_DIR, "messenger_certs")
ACCOUNTS_FILE = os.path.join(BASE_DIR, "messenger_accounts.json")
EMOJI_DATA_FILE = os.path.join(BASE_DIR, "emoji_data.json")

MAX_MESSAGES_IN_MEMORY = 500
MAX_NAME_LEN = 32
MAX_TEXT_LEN = 2000
MAX_UPLOAD_BYTES = 15 * 1024 * 1024  # 15 MB, generous for GIFs/screenshots
SESSION_COOKIE = "lm_session"
SESSION_TTL_SECS = 60 * 60 * 24 * 30  # 30 days

WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
OP_CONT, OP_TEXT, OP_BINARY, OP_CLOSE, OP_PING, OP_PONG = 0x0, 0x1, 0x2, 0x8, 0x9, 0xA

DEFAULT_SETTINGS = {
    "voice_mode": "vad",  # "vad" (voice-activity) or "ptt" (push-to-talk)
    "ptt_key": "Space",
    "input_volume": 100,
    "output_volume": 100,
    "theme": "dark",
}

AVATAR_PALETTE = [
    "#5865F2", "#57F287", "#FEE75C", "#EB459E", "#ED4245",
    "#F0B232", "#3BA55D", "#7289DA", "#43B581", "#FAA61A",
]

# ---------------------------------------------------------------------------
# Accounts (lightweight identity, not a security boundary against mesh peers)
# ---------------------------------------------------------------------------

accounts_lock = threading.Lock()
accounts = {}          # username_lower -> {username, salt, hash, avatar_color, settings}
sessions = {}          # session_token -> {username, expires}


def load_accounts():
    global accounts
    if not os.path.exists(ACCOUNTS_FILE):
        return
    try:
        with open(ACCOUNTS_FILE, "r", encoding="utf-8") as f:
            accounts = json.load(f)
    except (OSError, json.JSONDecodeError):
        accounts = {}


def save_accounts():
    try:
        tmp = ACCOUNTS_FILE + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(accounts, f, ensure_ascii=False, indent=2)
        os.replace(tmp, ACCOUNTS_FILE)
    except OSError:
        pass


def hash_password(password, salt=None):
    if salt is None:
        salt = secrets.token_hex(16)
    digest = hashlib.pbkdf2_hmac("sha256", password.encode("utf-8"), bytes.fromhex(salt), 200_000)
    return salt, digest.hex()


def verify_password(password, salt, expected_hash):
    _, computed = hash_password(password, salt)
    return hmac.compare_digest(computed, expected_hash)


def create_account(username, password):
    key = username.lower()
    with accounts_lock:
        if key in accounts:
            return False, "That username is already taken."
        if not re.match(r"^[A-Za-z0-9_\-]{2,24}$", username):
            return False, "Username must be 2-24 characters: letters, numbers, _ or -."
        if len(password) < 4:
            return False, "Password must be at least 4 characters."
        salt, pw_hash = hash_password(password)
        accounts[key] = {
            "username": username,
            "salt": salt,
            "hash": pw_hash,
            "avatar_color": AVATAR_PALETTE[len(accounts) % len(AVATAR_PALETTE)],
            "settings": dict(DEFAULT_SETTINGS),
        }
        save_accounts()
    return True, None


def check_login(username, password):
    with accounts_lock:
        acct = accounts.get(username.lower())
    if not acct:
        return False, "No such account."
    if not verify_password(password, acct["salt"], acct["hash"]):
        return False, "Incorrect password."
    return True, None


def create_session(username):
    token = secrets.token_urlsafe(32)
    with accounts_lock:
        sessions[token] = {"username": username, "expires": time.time() + SESSION_TTL_SECS}
    return token


def resolve_session(token):
    if not token:
        return None
    with accounts_lock:
        sess = sessions.get(token)
        if not sess:
            return None
        if sess["expires"] < time.time():
            sessions.pop(token, None)
            return None
        acct = accounts.get(sess["username"].lower())
        return acct


def get_settings(username):
    with accounts_lock:
        acct = accounts.get(username.lower())
        if not acct:
            return dict(DEFAULT_SETTINGS)
        merged = dict(DEFAULT_SETTINGS)
        merged.update(acct.get("settings", {}))
        return merged


def update_settings(username, patch):
    with accounts_lock:
        acct = accounts.get(username.lower())
        if not acct:
            return None
        settings = dict(DEFAULT_SETTINGS)
        settings.update(acct.get("settings", {}))
        for k in DEFAULT_SETTINGS:
            if k in patch:
                settings[k] = patch[k]
        acct["settings"] = settings
        save_accounts()
        return settings


# ---------------------------------------------------------------------------
# Message store (text channel)
# ---------------------------------------------------------------------------

store_lock = threading.Lock()
messages = []  # list of dicts: {id, ts, name, text, reactions: {emoji: [names]}}
next_msg_id = 1


def load_history():
    global messages, next_msg_id
    if not os.path.exists(HISTORY_FILE):
        return
    try:
        with open(HISTORY_FILE, "r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    msg = json.loads(line)
                    msg.setdefault("reactions", {})
                    messages.append(msg)
                except json.JSONDecodeError:
                    continue
        messages[:] = messages[-MAX_MESSAGES_IN_MEMORY:]
        if messages:
            next_msg_id = max(m["id"] for m in messages) + 1
    except OSError:
        pass


def append_history(msg):
    try:
        with open(HISTORY_FILE, "a", encoding="utf-8") as f:
            f.write(json.dumps(msg, ensure_ascii=False) + "\n")
    except OSError:
        pass


def rewrite_history():
    try:
        with open(HISTORY_FILE, "w", encoding="utf-8") as f:
            for m in messages:
                f.write(json.dumps(m, ensure_ascii=False) + "\n")
    except OSError:
        pass


def add_message(name, text):
    global next_msg_id
    with store_lock:
        msg = {"id": next_msg_id, "ts": time.time(), "name": name, "text": text, "reactions": {}}
        next_msg_id += 1
        messages.append(msg)
        if len(messages) > MAX_MESSAGES_IN_MEMORY:
            del messages[0]
    append_history(msg)
    return msg


def toggle_reaction(msg_id, emoji, name):
    with store_lock:
        for m in messages:
            if m["id"] == msg_id:
                users = m["reactions"].setdefault(emoji, [])
                if name in users:
                    users.remove(name)
                    if not users:
                        del m["reactions"][emoji]
                else:
                    users.append(name)
                rewrite_history()
                return m
    return None


# ---------------------------------------------------------------------------
# Presence / client registry
# ---------------------------------------------------------------------------

clients_lock = threading.RLock()
clients = {}  # id -> ClientState
next_client_id = 1


def avatar_color_for(name):
    with accounts_lock:
        acct = accounts.get(name.lower())
        if acct:
            return acct.get("avatar_color", AVATAR_PALETTE[0])
    idx = sum(ord(c) for c in name) % len(AVATAR_PALETTE)
    return AVATAR_PALETTE[idx]


class ClientState:
    def __init__(self, cid, conn):
        self.id = cid
        self.conn = conn
        self.send_lock = threading.Lock()
        self.name = f"guest{cid}"
        self.in_voice = False
        self.muted = True
        self.speaking = False
        self.alive = True


def register_client(conn):
    global next_client_id
    with clients_lock:
        cid = next_client_id
        next_client_id += 1
        state = ClientState(cid, conn)
        clients[cid] = state
    return state


def unregister_client(cid):
    with clients_lock:
        clients.pop(cid, None)


def presence_snapshot():
    with clients_lock:
        return [
            {
                "id": c.id,
                "name": c.name,
                "in_voice": c.in_voice,
                "muted": c.muted,
                "speaking": c.speaking,
                "color": avatar_color_for(c.name),
            }
            for c in clients.values()
        ]


def broadcast_json(obj, exclude_id=None):
    payload = json.dumps(obj).encode("utf-8")
    with clients_lock:
        targets = [c for c in clients.values() if c.id != exclude_id]
    for c in targets:
        try:
            send_ws_frame(c.conn, OP_TEXT, payload, c.send_lock)
        except OSError:
            c.alive = False


def send_json_to(client, obj):
    payload = json.dumps(obj).encode("utf-8")
    try:
        send_ws_frame(client.conn, OP_TEXT, payload, client.send_lock)
    except OSError:
        client.alive = False


def broadcast_presence():
    broadcast_json({"type": "presence", "users": presence_snapshot()})


def relay_voice(sender_id, pcm_bytes):
    header = bytes([sender_id & 0xFF])
    frame_payload = header + pcm_bytes
    with clients_lock:
        targets = [c for c in clients.values() if c.in_voice and c.id != sender_id]
    for c in targets:
        try:
            send_ws_frame(c.conn, OP_BINARY, frame_payload, c.send_lock)
        except OSError:
            c.alive = False


# ---------------------------------------------------------------------------
# Minimal RFC 6455 WebSocket framing (client<->server), stdlib only
# ---------------------------------------------------------------------------


def recv_exact(conn, n):
    buf = bytearray()
    while len(buf) < n:
        chunk = conn.recv(n - len(buf))
        if not chunk:
            return None
        buf.extend(chunk)
    return bytes(buf)


def read_ws_frame(conn):
    header = recv_exact(conn, 2)
    if header is None:
        return None
    b1, b2 = header[0], header[1]
    opcode = b1 & 0x0F
    masked = (b2 & 0x80) != 0
    length = b2 & 0x7F
    if length == 126:
        ext = recv_exact(conn, 2)
        if ext is None:
            return None
        length = struct.unpack("!H", ext)[0]
    elif length == 127:
        ext = recv_exact(conn, 8)
        if ext is None:
            return None
        length = struct.unpack("!Q", ext)[0]
    mask_key = None
    if masked:
        mask_key = recv_exact(conn, 4)
        if mask_key is None:
            return None
    payload = recv_exact(conn, length) if length else b""
    if payload is None:
        return None
    if masked and mask_key:
        payload = bytes(b ^ mask_key[i % 4] for i, b in enumerate(payload))
    return opcode, payload


def send_ws_frame(conn, opcode, payload, lock):
    if isinstance(payload, str):
        payload = payload.encode("utf-8")
    length = len(payload)
    header = bytes([0x80 | opcode])
    if length < 126:
        header += bytes([length])
    elif length < 65536:
        header += bytes([126]) + struct.pack("!H", length)
    else:
        header += bytes([127]) + struct.pack("!Q", length)
    with lock:
        conn.sendall(header + payload)


def ws_accept_key(client_key):
    sha1 = hashlib.sha1((client_key + WS_GUID).encode("utf-8")).digest()
    return base64.b64encode(sha1).decode("ascii")


# ---------------------------------------------------------------------------
# TLS self-signed certificate (generated once, reused across runs)
# ---------------------------------------------------------------------------


def ensure_self_signed_cert(cert_path, key_path):
    if os.path.exists(cert_path) and os.path.exists(key_path):
        return True
    os.makedirs(os.path.dirname(cert_path), exist_ok=True)
    try:
        subprocess.run(
            [
                "openssl", "req", "-x509", "-newkey", "rsa:2048",
                "-keyout", key_path, "-out", cert_path,
                "-days", "3650", "-nodes",
                "-subj", "/CN=lan_mesh",
                "-addext", "subjectAltName=DNS:lan_mesh,DNS:localhost,IP:127.0.0.1",
            ],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        return True
    except (OSError, subprocess.CalledProcessError):
        return False


def load_emoji_data():
    try:
        with open(EMOJI_DATA_FILE, "r", encoding="utf-8") as f:
            return f.read()
    except OSError:
        return "{}"


# ---------------------------------------------------------------------------
# HTTP(S) request handler
# ---------------------------------------------------------------------------


class Handler(BaseHTTPRequestHandler):
    server_version = "lan_mesh_messenger/3.0"
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        print(f"[messenger] {self.address_string()} - {fmt % args}")

    def _send_json(self, obj, status=200, extra_headers=None):
        body = json.dumps(obj).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        for k, v in (extra_headers or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body)

    def _get_cookie(self, name):
        cookie_header = self.headers.get("Cookie", "")
        for part in cookie_header.split(";"):
            part = part.strip()
            if part.startswith(name + "="):
                return part[len(name) + 1:]
        return None

    def _current_account(self):
        token = self._get_cookie(SESSION_COOKIE)
        return resolve_session(token)

    def _read_json_body(self):
        length = int(self.headers.get("Content-Length", 0))
        if length <= 0 or length > 65536:
            return None
        raw = self.rfile.read(length)
        try:
            return json.loads(raw)
        except json.JSONDecodeError:
            return None

    # -- routing --

    def do_GET(self):
        parsed = urlparse(self.path)

        if parsed.path == "/ws":
            self._handle_ws_upgrade()
            return

        if parsed.path in ("/", "/index.html"):
            body = render_index_html().encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return

        if parsed.path == "/api/emoji":
            body = load_emoji_data().encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "public, max-age=86400")
            self.end_headers()
            self.wfile.write(body)
            return

        if parsed.path == "/api/me":
            acct = self._current_account()
            if not acct:
                self._send_json({"authenticated": False})
                return
            self._send_json({
                "authenticated": True,
                "username": acct["username"],
                "avatar_color": acct.get("avatar_color"),
                "settings": get_settings(acct["username"]),
            })
            return

        if parsed.path == "/api/messages":
            qs = parse_qs(parsed.query)
            since = int(qs.get("since", ["0"])[0])
            with store_lock:
                new_msgs = [m for m in messages if m["id"] > since]
            self._send_json({"messages": new_msgs})
            return

        if parsed.path.startswith("/uploads/"):
            self._serve_upload(parsed.path[len("/uploads/"):])
            return

        self.send_response(404)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def do_POST(self):
        parsed = urlparse(self.path)
        if parsed.path == "/api/upload":
            self._handle_upload()
            return
        if parsed.path == "/api/register":
            self._handle_register()
            return
        if parsed.path == "/api/login":
            self._handle_login()
            return
        if parsed.path == "/api/logout":
            self._handle_logout()
            return
        if parsed.path == "/api/settings":
            self._handle_settings_update()
            return
        self.send_response(404)
        self.send_header("Content-Length", "0")
        self.end_headers()

    # -- accounts --

    def _handle_register(self):
        data = self._read_json_body()
        if not data:
            self._send_json({"error": "bad request"}, status=400)
            return
        username = str(data.get("username", "")).strip()
        password = str(data.get("password", ""))
        ok, err = create_account(username, password)
        if not ok:
            self._send_json({"error": err}, status=400)
            return
        token = create_session(username)
        self._send_json(
            {"ok": True, "username": username, "settings": get_settings(username)},
            extra_headers={"Set-Cookie": f"{SESSION_COOKIE}={token}; Path=/; Max-Age={SESSION_TTL_SECS}; SameSite=Strict"},
        )

    def _handle_login(self):
        data = self._read_json_body()
        if not data:
            self._send_json({"error": "bad request"}, status=400)
            return
        username = str(data.get("username", "")).strip()
        password = str(data.get("password", ""))
        ok, err = check_login(username, password)
        if not ok:
            self._send_json({"error": err}, status=400)
            return
        token = create_session(username)
        self._send_json(
            {"ok": True, "username": username, "settings": get_settings(username)},
            extra_headers={"Set-Cookie": f"{SESSION_COOKIE}={token}; Path=/; Max-Age={SESSION_TTL_SECS}; SameSite=Strict"},
        )

    def _handle_logout(self):
        token = self._get_cookie(SESSION_COOKIE)
        if token:
            with accounts_lock:
                sessions.pop(token, None)
        self._send_json({"ok": True}, extra_headers={"Set-Cookie": f"{SESSION_COOKIE}=; Path=/; Max-Age=0"})

    def _handle_settings_update(self):
        acct = self._current_account()
        if not acct:
            self._send_json({"error": "not logged in"}, status=401)
            return
        data = self._read_json_body()
        if not data:
            self._send_json({"error": "bad request"}, status=400)
            return
        updated = update_settings(acct["username"], data)
        self._send_json({"ok": True, "settings": updated})

    # -- static upload serving --

    def _serve_upload(self, filename):
        filename = os.path.basename(filename)
        path = os.path.join(UPLOADS_DIR, filename)
        if not os.path.isfile(path):
            self.send_response(404)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        ctype, _ = mimetypes.guess_type(path)
        with open(path, "rb") as f:
            data = f.read()
        self.send_response(200)
        self.send_header("Content-Type", ctype or "application/octet-stream")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "public, max-age=31536000, immutable")
        self.end_headers()
        self.wfile.write(data)

    def _handle_upload(self):
        length = int(self.headers.get("Content-Length", 0))
        if length <= 0 or length > MAX_UPLOAD_BYTES:
            self._send_json({"error": "file too large or empty"}, status=400)
            return
        data = self.rfile.read(length)
        orig_name = self.headers.get("X-Filename", "upload")
        try:
            orig_name = unquote(orig_name)
        except Exception:
            pass
        ext = os.path.splitext(orig_name)[1].lower()
        if ext not in (".gif", ".png", ".jpg", ".jpeg", ".webp"):
            ext = ".bin"
        fname = f"{uuid.uuid4().hex}{ext}"
        os.makedirs(UPLOADS_DIR, exist_ok=True)
        with open(os.path.join(UPLOADS_DIR, fname), "wb") as f:
            f.write(data)
        self._send_json({"url": f"/uploads/{fname}"})

    # -- WebSocket upgrade + client loop --

    def _handle_ws_upgrade(self):
        key = self.headers.get("Sec-WebSocket-Key")
        if not key or self.headers.get("Upgrade", "").lower() != "websocket":
            self.send_response(400)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return

        accept = ws_accept_key(key)
        self.send_response(101, "Switching Protocols")
        self.send_header("Upgrade", "websocket")
        self.send_header("Connection", "Upgrade")
        self.send_header("Sec-WebSocket-Accept", accept)
        self.end_headers()

        conn = self.connection
        state = register_client(conn)
        acct = self._current_account()
        if acct:
            state.name = acct["username"]
        try:
            self._ws_client_loop(state)
        finally:
            unregister_client(state.id)
            broadcast_presence()

    def _ws_client_loop(self, state):
        send_json_to(state, {"type": "welcome", "id": state.id, "name": state.name})
        broadcast_presence()
        while True:
            frame = read_ws_frame(self.connection)
            if frame is None:
                break
            opcode, payload = frame
            if opcode == OP_CLOSE:
                break
            elif opcode == OP_TEXT:
                self._handle_control_message(state, payload)
            elif opcode == OP_BINARY:
                if state.in_voice and not state.muted:
                    relay_voice(state.id, payload)
            elif opcode == OP_PING:
                try:
                    send_ws_frame(self.connection, OP_PONG, payload, state.send_lock)
                except OSError:
                    break

    def _handle_control_message(self, state, payload):
        try:
            msg = json.loads(payload.decode("utf-8"))
        except (json.JSONDecodeError, UnicodeDecodeError):
            return
        mtype = msg.get("type")

        if mtype == "hello":
            name = str(msg.get("name", "")).strip()[:MAX_NAME_LEN]
            state.name = name or state.name
            broadcast_presence()

        elif mtype == "chat":
            name = str(msg.get("name", state.name)).strip()[:MAX_NAME_LEN] or state.name
            text = str(msg.get("text", "")).strip()[:MAX_TEXT_LEN]
            if not text:
                return
            state.name = name
            saved = add_message(name, text)
            broadcast_json({"type": "chat", "message": saved})

        elif mtype == "react":
            try:
                msg_id = int(msg.get("id"))
            except (TypeError, ValueError):
                return
            emoji = str(msg.get("emoji", ""))[:16]
            name = str(msg.get("name", state.name)).strip()[:MAX_NAME_LEN] or state.name
            if not emoji:
                return
            updated = toggle_reaction(msg_id, emoji, name)
            if updated:
                broadcast_json({"type": "react_update", "message": updated})

        elif mtype == "typing":
            name = str(msg.get("name", state.name)).strip()[:MAX_NAME_LEN] or state.name
            broadcast_json({"type": "typing", "name": name}, exclude_id=state.id)

        elif mtype == "voice-join":
            state.in_voice = True
            broadcast_presence()

        elif mtype == "voice-leave":
            state.in_voice = False
            state.speaking = False
            broadcast_presence()

        elif mtype == "mute":
            state.muted = bool(msg.get("muted", True))
            if state.muted:
                state.speaking = False
            broadcast_presence()

        elif mtype == "speaking":
            state.speaking = bool(msg.get("value", False))
            broadcast_presence()


class ThreadingHTTPSServer(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True

    def __init__(self, server_address, handler_cls, certfile=None, keyfile=None):
        super().__init__(server_address, handler_cls)
        if certfile and keyfile:
            ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            ctx.load_cert_chain(certfile=certfile, keyfile=keyfile)
            self.socket = ctx.wrap_socket(self.socket, server_side=True)


def render_index_html():
    from frontend import INDEX_HTML
    return INDEX_HTML


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--host", default="0.0.0.0", help="Address to bind (default: 0.0.0.0, all interfaces)")
    ap.add_argument("--port", type=int, default=8765, help="Port to listen on (default: 8765)")
    ap.add_argument("--no-tls", action="store_true", help="Serve plain HTTP (voice/mic access will not work in browsers)")
    args = ap.parse_args()

    load_history()
    load_accounts()

    certfile = keyfile = None
    scheme = "http"
    if not args.no_tls:
        cert_path = os.path.join(CERTS_DIR, "cert.pem")
        key_path = os.path.join(CERTS_DIR, "key.pem")
        if ensure_self_signed_cert(cert_path, key_path):
            certfile, keyfile = cert_path, key_path
            scheme = "https"
        else:
            print("WARNING: could not generate a TLS certificate (is `openssl` installed?).")
            print("Falling back to plain HTTP -- text chat/images will work, but browsers")
            print("will block microphone access for voice chat over a non-HTTPS origin.")

    server = ThreadingHTTPSServer((args.host, args.port), Handler, certfile, keyfile)
    print(f"lan_mesh messenger listening on {scheme}://{args.host}:{args.port}/")
    print("Share this with your mesh peers, using YOUR virtual mesh IP instead of 0.0.0.0, e.g.:")
    print(f"    {scheme}://10.66.0.X:{args.port}/")
    if scheme == "https":
        print("(Self-signed certificate: browsers will show a one-time warning -- click")
        print(" Advanced -> Proceed. This is expected and needed for voice chat to work.)")
    print(f"Loaded {len(messages)} messages, {len(accounts)} accounts. Press Ctrl+C to stop.")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nShutting down.")


if __name__ == "__main__":
    main()
