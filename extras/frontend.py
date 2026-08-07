INDEX_HTML = r"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>lan_mesh messenger</title>
<style>
  :root {
    color-scheme: dark;
    --bg-deepest: #1e1f22;
    --bg-sidebar: #2b2d31;
    --bg-chat: #313338;
    --bg-members: #2b2d31;
    --bg-elevated: #383a40;
    --bg-input: #383a40;
    --accent: #5865F2;
    --accent-hover: #4752c4;
    --text-normal: #dbdee1;
    --text-muted: #949ba4;
    --text-bright: #f2f3f5;
    --green: #23a55a;
    --red: #f23f42;
    --divider: #1e1f22;
  }
  * { box-sizing: border-box; }
  body {
    margin: 0; font-family: "gg sans", -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
    background: var(--bg-deepest); color: var(--text-normal); height: 100vh; overflow: hidden;
  }
  #app { display: none; height: 100vh; }
  #app.ready { display: flex; }

  /* ---------- Auth screen ---------- */
  #authscreen {
    display: flex; align-items: center; justify-content: center; height: 100vh;
    background: var(--bg-deepest);
  }
  #authscreen.hidden { display: none; }
  .auth-card {
    background: var(--bg-chat); border-radius: 8px; padding: 32px; width: 420px; max-width: 90vw;
    box-shadow: 0 8px 32px rgba(0,0,0,.4);
  }
  .auth-card h1 { text-align: center; font-size: 24px; margin: 0 0 8px; color: var(--text-bright); }
  .auth-card p.sub { text-align: center; color: var(--text-muted); margin: 0 0 20px; font-size: 14px; }
  .auth-card label { display: block; font-size: 12px; font-weight: 700; text-transform: uppercase; color: var(--text-muted); margin: 14px 0 6px; }
  .auth-card input {
    width: 100%; padding: 10px; border-radius: 4px; border: none; background: var(--bg-deepest);
    color: var(--text-normal); font-size: 15px;
  }
  .auth-card button.primary {
    width: 100%; margin-top: 20px; padding: 12px; border-radius: 4px; border: none;
    background: var(--accent); color: white; font-size: 15px; font-weight: 600; cursor: pointer;
  }
  .auth-card button.primary:hover { background: var(--accent-hover); }
  .auth-card .switch { text-align: center; margin-top: 14px; font-size: 13px; color: var(--text-muted); }
  .auth-card .switch a { color: #00a8fc; cursor: pointer; text-decoration: none; }
  .auth-card .err { color: var(--red); font-size: 13px; margin-top: 10px; min-height: 16px; }
  .auth-card .guest-link { text-align: center; margin-top: 10px; font-size: 12px; }
  .auth-card .guest-link a { color: var(--text-muted); cursor: pointer; }

  /* ---------- Main layout: channel sidebar | chat | member list ---------- */
  #channelbar {
    width: 240px; background: var(--bg-sidebar); display: flex; flex-direction: column; flex-shrink: 0;
  }
  #channelbar .server-header {
    padding: 14px 16px; font-weight: 700; color: var(--text-bright); box-shadow: 0 1px 0 var(--divider);
    font-size: 15px;
  }
  #channelbar .channels { flex: 1; padding: 12px 8px; overflow-y: auto; }
  .channel-cat { font-size: 11px; font-weight: 700; color: var(--text-muted); text-transform: uppercase; padding: 6px 8px; letter-spacing: .02em; }
  .channel-row {
    display: flex; align-items: center; gap: 6px; padding: 7px 8px; border-radius: 4px; cursor: pointer;
    color: var(--text-muted); font-size: 15px; font-weight: 500; margin-bottom: 2px;
  }
  .channel-row.active, .channel-row:hover { background: #35373c; color: var(--text-bright); }
  .channel-row .hash { opacity: .7; }
  #voice-users-inline { padding-left: 26px; }
  .voice-user-inline { display: flex; align-items: center; gap: 6px; padding: 4px 8px; font-size: 13px; color: var(--text-muted); }
  .voice-user-inline.speaking { color: var(--green); }
  .voice-user-inline .mini-avatar { width: 18px; height: 18px; border-radius: 50%; font-size: 10px; }

  #userbar {
    background: #232428; padding: 8px; display: flex; align-items: center; gap: 8px;
  }
  .avatar {
    width: 32px; height: 32px; border-radius: 50%; display: flex; align-items: center; justify-content: center;
    font-weight: 700; font-size: 13px; color: white; flex-shrink: 0; position: relative;
  }
  .avatar .status-dot {
    position: absolute; bottom: -1px; right: -1px; width: 10px; height: 10px; border-radius: 50%;
    background: var(--green); border: 2px solid #232428;
  }
  #userbar .who { flex: 1; min-width: 0; }
  #userbar .uname { font-size: 13px; font-weight: 600; color: var(--text-bright); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  #userbar .ustate { font-size: 11px; color: var(--text-muted); }
  #userbar button {
    background: none; border: none; color: var(--text-muted); font-size: 16px; cursor: pointer; padding: 6px; border-radius: 4px;
  }
  #userbar button:hover { background: #35373c; color: var(--text-bright); }
  #userbar button.active { color: var(--green); }
  #userbar button.muted-btn.active-mute { color: var(--red); }

  #chatcol { flex: 1; display: flex; flex-direction: column; min-width: 0; background: var(--bg-chat); }
  #chatheader {
    padding: 12px 16px; box-shadow: 0 1px 0 var(--divider); display: flex; align-items: center; gap: 8px;
    font-weight: 700; color: var(--text-bright); flex-shrink: 0;
  }
  #chatheader .status { font-weight: 400; font-size: 12px; color: var(--text-muted); margin-left: auto; }

  #messages { flex: 1; overflow-y: auto; padding: 16px 16px 8px; display: flex; flex-direction: column; gap: 2px; }
  .msg-row { display: flex; gap: 14px; padding: 4px 8px; border-radius: 6px; }
  .msg-row:hover { background: #2e3035; }
  .msg-row .avatar { margin-top: 2px; }
  .msg-body { flex: 1; min-width: 0; }
  .msg-body .head { display: flex; align-items: baseline; gap: 8px; }
  .msg-body .aname { font-weight: 600; color: var(--text-bright); font-size: 15px; }
  .msg-body .atime { font-size: 11px; color: var(--text-muted); }
  .msg-body .content { font-size: 15px; line-height: 1.375; white-space: pre-wrap; word-break: break-word; color: var(--text-normal); }
  .msg-body .content a { color: #00a8fc; }
  .msg-body img.embed { max-width: 320px; max-height: 320px; border-radius: 8px; display: block; margin-top: 6px; cursor: pointer; }
  .reactions { display: flex; gap: 4px; flex-wrap: wrap; margin-top: 4px; }
  .reaction-badge {
    background: #2b2d31; border: 1px solid #3f4147; border-radius: 8px; padding: 2px 8px;
    font-size: 13px; cursor: pointer; user-select: none; display: inline-flex; gap: 4px; align-items: center;
  }
  .reaction-badge:hover { border-color: var(--accent); }
  .reaction-badge.mine { border-color: var(--accent); background: #3b3d8f33; }
  .react-add { font-size: 13px; cursor: pointer; opacity: .5; padding: 2px 8px; border-radius: 8px; border: 1px solid transparent; }
  .react-add:hover { opacity: 1; background: #2b2d31; border-color: #3f4147; }

  #typing { font-size: 13px; color: var(--text-muted); padding: 0 16px 4px; min-height: 18px; }

  #composer-wrap { padding: 0 16px 20px; flex-shrink: 0; position: relative; }
  #composer { display: flex; align-items: flex-end; gap: 8px; background: var(--bg-input); border-radius: 8px; padding: 8px 10px; }
  #composer button.icon-btn { background: none; border: none; color: var(--text-muted); font-size: 20px; cursor: pointer; padding: 4px 6px; }
  #composer button.icon-btn:hover { color: var(--text-bright); }
  #text { flex: 1; background: none; border: none; color: var(--text-normal); font-size: 15px; resize: none; outline: none; padding: 6px 0; max-height: 140px; font-family: inherit; }
  #text::placeholder { color: var(--text-muted); }
  #composer.dragover { outline: 2px solid var(--accent); }

  /* ---------- Emoji picker ---------- */
  #emojipicker {
    display: none; position: absolute; bottom: 64px; right: 8px; width: 340px; height: 400px;
    background: var(--bg-elevated); border-radius: 8px; box-shadow: 0 8px 24px rgba(0,0,0,.5);
    flex-direction: column; overflow: hidden; z-index: 30;
  }
  #emojipicker.show { display: flex; }
  #emoji-search { margin: 8px; padding: 8px 10px; border-radius: 4px; border: none; background: var(--bg-deepest); color: var(--text-normal); font-size: 13px; }
  #emoji-tabs { display: flex; padding: 0 8px 6px; gap: 2px; overflow-x: auto; flex-shrink: 0; }
  #emoji-tabs button {
    background: none; border: none; font-size: 17px; padding: 5px 7px; cursor: pointer; border-radius: 4px; opacity: .6;
  }
  #emoji-tabs button.active { opacity: 1; background: #43444b; }
  #emoji-list { flex: 1; overflow-y: auto; padding: 4px 8px 8px; }
  #emoji-list .cat-label { font-size: 11px; font-weight: 700; color: var(--text-muted); text-transform: uppercase; padding: 6px 4px 2px; }
  #emoji-list .cat-grid { display: grid; grid-template-columns: repeat(8, 1fr); gap: 2px; }
  #emoji-list .em { font-size: 22px; text-align: center; padding: 4px 0; border-radius: 4px; cursor: pointer; }
  #emoji-list .em:hover { background: #43444b; }

  /* ---------- Members sidebar ---------- */
  #members { width: 240px; background: var(--bg-members); flex-shrink: 0; overflow-y: auto; padding: 16px 8px; }
  #members h3 { font-size: 12px; text-transform: uppercase; color: var(--text-muted); padding: 0 8px; margin: 0 0 6px; }
  .member-row { display: flex; align-items: center; gap: 10px; padding: 6px 8px; border-radius: 4px; cursor: default; }
  .member-row:hover { background: #35373c; }
  .member-row .name { font-size: 15px; color: var(--text-normal); }
  .member-row.speaking .name { color: var(--green); }
  .member-row .mic-icon { margin-left: auto; font-size: 12px; opacity: .8; }

  /* ---------- Settings modal ---------- */
  #settings-modal {
    display: none; position: fixed; inset: 0; background: var(--bg-chat); z-index: 100;
  }
  #settings-modal.show { display: flex; }
  #settings-sidebar { width: 220px; background: #2b2d31; padding: 60px 8px 20px; }
  #settings-sidebar h4 { font-size: 12px; text-transform: uppercase; color: var(--text-muted); padding: 0 12px; margin: 18px 0 6px; }
  #settings-sidebar .item { padding: 8px 12px; border-radius: 4px; color: var(--text-normal); cursor: pointer; font-size: 15px; }
  #settings-sidebar .item.active, #settings-sidebar .item:hover { background: #35373c; color: var(--text-bright); }
  #settings-body { flex: 1; padding: 60px 40px; overflow-y: auto; max-width: 700px; }
  #settings-body h2 { color: var(--text-bright); border-bottom: 1px solid var(--divider); padding-bottom: 16px; }
  .setting-row { padding: 16px 0; border-bottom: 1px solid #3a3c42; }
  .setting-row label.title { display: block; font-size: 12px; font-weight: 700; text-transform: uppercase; color: var(--text-muted); margin-bottom: 8px; }
  .setting-row select, .setting-row input[type=range] { width: 100%; }
  .setting-row select {
    padding: 10px; border-radius: 4px; border: none; background: var(--bg-deepest); color: var(--text-normal); font-size: 14px;
  }
  .ptt-key-btn {
    padding: 8px 14px; border-radius: 4px; border: none; background: var(--bg-deepest); color: var(--text-normal);
    cursor: pointer; font-size: 13px;
  }
  .ptt-key-btn.listening { background: var(--accent); color: white; }
  #settings-close {
    position: fixed; top: 20px; right: 40px; width: 36px; height: 36px; border-radius: 50%; border: 2px solid var(--text-muted);
    background: none; color: var(--text-muted); font-size: 18px; cursor: pointer; z-index: 101;
  }
  #settings-close:hover { color: white; border-color: white; }

  .lightbox {
    position: fixed; inset: 0; background: rgba(0,0,0,.85); display: none;
    align-items: center; justify-content: center; z-index: 200; cursor: zoom-out;
  }
  .lightbox.show { display: flex; }
  .lightbox img { max-width: 92vw; max-height: 92vh; border-radius: 8px; }

  ::-webkit-scrollbar { width: 8px; }
  ::-webkit-scrollbar-thumb { background: #1a1b1e; border-radius: 4px; }
  ::-webkit-scrollbar-track { background: transparent; }
</style>
</head>
<body>

<div id="authscreen">
  <div class="auth-card">
    <h1 id="auth-title">Welcome back!</h1>
    <p class="sub" id="auth-sub">We're so excited to see you again.</p>
    <label>Username</label>
    <input id="auth-username" maxlength="24" autocomplete="username">
    <label>Password</label>
    <input id="auth-password" type="password" autocomplete="current-password">
    <div class="err" id="auth-err"></div>
    <button class="primary" id="auth-submit">Log In</button>
    <div class="switch" id="auth-switch">Need an account? <a id="auth-switch-link">Register</a></div>
    <div class="guest-link"><a id="auth-guest-link">or continue as a guest, no account</a></div>
  </div>
</div>

<div id="app">
  <div id="channelbar">
    <div class="server-header">lan_mesh messenger</div>
    <div class="channels">
      <div class="channel-cat">Text Channels</div>
      <div class="channel-row active"><span class="hash">#</span> general</div>
      <div class="channel-cat" style="margin-top:16px;">Voice Channels</div>
      <div class="channel-row" id="voice-channel-row"><span class="hash">🔊</span> General</div>
      <div id="voice-users-inline"></div>
    </div>
    <div id="userbar">
      <div class="avatar" id="my-avatar"><span class="status-dot"></span></div>
      <div class="who">
        <div class="uname" id="my-name">guest</div>
        <div class="ustate" id="my-state">Not connected</div>
      </div>
      <button id="voicejoin-btn" title="Join Voice">🎙</button>
      <button id="mute-btn" title="Mute">🔊</button>
      <button id="settings-btn" title="Settings">⚙️</button>
    </div>
  </div>

  <div id="chatcol">
    <div id="chatheader"><span class="hash">#</span> general <span class="status" id="status">connecting...</span></div>
    <div id="messages"></div>
    <div id="typing">&nbsp;</div>
    <div id="composer-wrap">
      <div id="composer">
        <button class="icon-btn" id="uploadbtn" title="Upload image/GIF">➕</button>
        <textarea id="text" placeholder="Message #general" rows="1" maxlength="2000"></textarea>
        <button class="icon-btn" id="emojibtn" title="Emoji">😀</button>
      </div>
      <input type="file" id="filepicker" accept="image/*" style="display:none">
      <div id="emojipicker">
        <input id="emoji-search" placeholder="Search emoji">
        <div id="emoji-tabs"></div>
        <div id="emoji-list"></div>
      </div>
    </div>
  </div>

  <div id="members">
    <h3>Online — <span id="online-count">0</span></h3>
    <div id="memberlist"></div>
  </div>
</div>

<div id="settings-modal">
  <button id="settings-close">✕</button>
  <div id="settings-sidebar">
    <h4>User Settings</h4>
    <div class="item active" data-pane="voice">Voice &amp; Audio</div>
    <div class="item" data-pane="account">My Account</div>
  </div>
  <div id="settings-body">
    <div class="pane" id="pane-voice">
      <h2>Voice &amp; Audio</h2>
      <div class="setting-row">
        <label class="title">Input Mode</label>
        <select id="set-voice-mode">
          <option value="vad">Voice Activity</option>
          <option value="ptt">Push to Talk</option>
        </select>
      </div>
      <div class="setting-row" id="ptt-row" style="display:none;">
        <label class="title">Push to Talk Key</label>
        <button class="ptt-key-btn" id="ptt-key-btn">Space</button>
      </div>
      <div class="setting-row">
        <label class="title">Input Volume</label>
        <input type="range" min="0" max="200" id="set-input-volume">
      </div>
      <div class="setting-row">
        <label class="title">Output Volume</label>
        <input type="range" min="0" max="200" id="set-output-volume">
      </div>
    </div>
    <div class="pane" id="pane-account" style="display:none;">
      <h2>My Account</h2>
      <div class="setting-row">
        <label class="title">Username</label>
        <div id="account-username" style="font-size:16px;color:var(--text-bright);"></div>
      </div>
      <div class="setting-row">
        <button class="ptt-key-btn" id="logout-btn" style="background:var(--red);color:white;">Log Out</button>
      </div>
    </div>
  </div>
</div>

<div class="lightbox" id="lightbox"><img id="lightbox-img"></div>

<script>
// ================= State =================
let myName = '';
let myColor = '#5865F2';
let authenticated = false;
let settings = { voice_mode: 'vad', ptt_key: 'Space', input_volume: 100, output_volume: 100 };
let ws = null;
let lastMsgId = 0;

function initials(name) {
  return (name || '?').trim().slice(0, 2).toUpperCase();
}

function renderAvatar(el, name, color) {
  el.style.background = color;
  el.textContent = '';
  const span = document.createElement('span');
  span.textContent = initials(name);
  el.appendChild(span);
  const dot = document.createElement('span');
  dot.className = 'status-dot';
  el.appendChild(dot);
}

// ================= Auth screen =================
let authMode = 'login';
function setAuthMode(mode) {
  authMode = mode;
  document.getElementById('auth-title').textContent = mode === 'login' ? 'Welcome back!' : 'Create an account';
  document.getElementById('auth-sub').textContent = mode === 'login' ? "We're so excited to see you again." : 'Just a nickname + password to remember you.';
  document.getElementById('auth-submit').textContent = mode === 'login' ? 'Log In' : 'Register';
  document.getElementById('auth-switch').innerHTML = mode === 'login'
    ? 'Need an account? <a id="auth-switch-link">Register</a>'
    : 'Already have one? <a id="auth-switch-link">Log In</a>';
  document.getElementById('auth-switch-link').addEventListener('click', () => setAuthMode(mode === 'login' ? 'register' : 'login'));
  document.getElementById('auth-err').textContent = '';
}

document.getElementById('auth-switch-link').addEventListener('click', () => setAuthMode('register'));

document.getElementById('auth-submit').addEventListener('click', async () => {
  const username = document.getElementById('auth-username').value.trim();
  const password = document.getElementById('auth-password').value;
  const errEl = document.getElementById('auth-err');
  if (!username || !password) { errEl.textContent = 'Please fill in both fields.'; return; }
  const endpoint = authMode === 'login' ? '/api/login' : '/api/register';
  try {
    const res = await fetch(endpoint, { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({username, password}) });
    const data = await res.json();
    if (!res.ok) { errEl.textContent = data.error || 'Something went wrong.'; return; }
    myName = data.username;
    settings = Object.assign(settings, data.settings || {});
    authenticated = true;
    enterApp();
  } catch (e) {
    errEl.textContent = 'Could not reach server: ' + e.message;
  }
});

document.getElementById('auth-guest-link').addEventListener('click', () => {
  myName = 'guest' + Math.floor(Math.random() * 10000);
  authenticated = false;
  enterApp();
});

async function checkExistingSession() {
  try {
    const res = await fetch('/api/me');
    const data = await res.json();
    if (data.authenticated) {
      myName = data.username;
      settings = Object.assign(settings, data.settings || {});
      authenticated = true;
      enterApp();
      return;
    }
  } catch (e) { /* fall through to auth screen */ }
  setAuthMode('login');
}

function enterApp() {
  document.getElementById('authscreen').classList.add('hidden');
  document.getElementById('app').classList.add('ready');
  myColor = colorFor(myName);
  document.getElementById('my-name').textContent = myName;
  renderAvatar(document.getElementById('my-avatar'), myName, myColor);
  document.getElementById('account-username').textContent = authenticated ? myName : `${myName} (guest, not saved)`;
  applySettingsToUI();
  loadHistory().then(connectWs);
}

function colorFor(name) {
  const palette = ['#5865F2','#57F287','#FEE75C','#EB459E','#ED4245','#F0B232','#3BA55D','#7289DA','#43B581','#FAA61A'];
  let sum = 0;
  for (const c of name) sum += c.charCodeAt(0);
  return palette[sum % palette.length];
}

// ================= Chat rendering =================
function escapeHtml(s) {
  const d = document.createElement('div');
  d.innerText = s;
  return d.innerHTML;
}

function linkifyAndEmbed(text) {
  const escaped = escapeHtml(text);
  return escaped.replace(/https?:\/\/\S+/gi, (url) => {
    const clean = url.replace(/&quot;|&#39;/g, '');
    if (/\.(gif|png|jpe?g|webp)(\?\S*)?$/i.test(clean)) {
      return `<a href="${clean}" target="_blank" rel="noopener">${clean}</a><img class="embed" src="${clean}" loading="lazy" onclick="openLightbox('${clean}')">`;
    }
    return `<a href="${clean}" target="_blank" rel="noopener">${clean}</a>`;
  });
}

function openLightbox(src) {
  document.getElementById('lightbox-img').src = src;
  document.getElementById('lightbox').classList.add('show');
}
document.getElementById('lightbox').addEventListener('click', () => {
  document.getElementById('lightbox').classList.remove('show');
});

const REACTION_QUICKSET = ['👍','😂','❤️','🔥','😮','🎉'];

function renderReactions(msg) {
  const container = document.createElement('div');
  container.className = 'reactions';
  const reactions = msg.reactions || {};
  for (const emoji in reactions) {
    const users = reactions[emoji];
    if (!users.length) continue;
    const badge = document.createElement('span');
    badge.className = 'reaction-badge' + (users.includes(myName) ? ' mine' : '');
    badge.textContent = `${emoji} ${users.length}`;
    badge.title = users.join(', ');
    badge.onclick = () => sendReaction(msg.id, emoji);
    container.appendChild(badge);
  }
  const addBtn = document.createElement('span');
  addBtn.className = 'react-add';
  addBtn.textContent = '+ 😀';
  addBtn.onclick = (e) => openQuickReactPicker(e, msg.id);
  container.appendChild(addBtn);
  return container;
}

let quickReactTarget = null;
function openQuickReactPicker(evt, msgId) {
  quickReactTarget = msgId;
  openEmojiPicker(evt.target, (emoji) => { sendReaction(msgId, emoji); closeEmojiPicker(); });
}

function renderMessage(m) {
  const container = document.getElementById('messages');
  const row = document.createElement('div');
  row.className = 'msg-row';
  const avatar = document.createElement('div');
  avatar.className = 'avatar';
  renderAvatar(avatar, m.name, colorFor(m.name));
  avatar.querySelector('.status-dot').style.display = 'none';
  const body = document.createElement('div');
  body.className = 'msg-body';
  const time = new Date(m.ts * 1000).toLocaleTimeString([], {hour: '2-digit', minute:'2-digit'});
  const head = document.createElement('div');
  head.className = 'head';
  head.innerHTML = `<span class="aname">${escapeHtml(m.name)}</span><span class="atime">${time}</span>`;
  const content = document.createElement('div');
  content.className = 'content';
  content.innerHTML = linkifyAndEmbed(m.text);
  body.appendChild(head);
  body.appendChild(content);
  body.appendChild(renderReactions(m));
  row.appendChild(avatar);
  row.appendChild(body);
  container.appendChild(row);
  container.scrollTop = container.scrollHeight;
  lastMsgId = Math.max(lastMsgId, m.id);
}

function sendReaction(id, emoji) {
  wsSend({type: 'react', id, emoji, name: myName});
}

async function loadHistory() {
  const res = await fetch('/api/messages?since=0');
  const data = await res.json();
  for (const m of data.messages) renderMessage(m);
}

function refreshAllMessages() {
  document.getElementById('messages').innerHTML = '';
  lastMsgId = 0;
  fetch('/api/messages?since=0').then(r => r.json()).then(data => {
    for (const msg of data.messages) renderMessage(msg);
  });
}

// ================= WebSocket =================
function wsSend(obj) {
  if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(obj));
}

function connectWs() {
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  ws = new WebSocket(`${proto}//${location.host}/ws`);
  ws.binaryType = 'arraybuffer';
  ws.onopen = () => {
    document.getElementById('status').textContent = 'connected';
    wsSend({type: 'hello', name: myName});
  };
  ws.onclose = () => {
    document.getElementById('status').textContent = 'reconnecting...';
    stopVoice();
    setTimeout(connectWs, 1500);
  };
  ws.onerror = () => { try { ws.close(); } catch (e) {} };
  ws.onmessage = (ev) => {
    if (typeof ev.data === 'string') handleControlMessage(JSON.parse(ev.data));
    else handleVoiceFrame(ev.data);
  };
}

let typingTimeout = null;
function handleControlMessage(msg) {
  if (msg.type === 'chat') {
    renderMessage(msg.message);
  } else if (msg.type === 'react_update') {
    refreshAllMessages();
  } else if (msg.type === 'presence') {
    renderMemberList(msg.users);
  } else if (msg.type === 'typing') {
    const el = document.getElementById('typing');
    el.textContent = `${msg.name} is typing...`;
    clearTimeout(typingTimeout);
    typingTimeout = setTimeout(() => { el.textContent = '\u00a0'; }, 2500);
  }
}

function renderMemberList(users) {
  document.getElementById('online-count').textContent = users.length;
  const list = document.getElementById('memberlist');
  list.innerHTML = '';
  for (const u of users) {
    const row = document.createElement('div');
    row.className = 'member-row' + (u.speaking ? ' speaking' : '');
    const av = document.createElement('div');
    av.className = 'avatar';
    av.style.width = '28px'; av.style.height = '28px'; av.style.fontSize = '11px';
    renderAvatar(av, u.name, u.color || colorFor(u.name));
    av.querySelector('.status-dot').style.display = 'none';
    const name = document.createElement('span');
    name.className = 'name';
    name.textContent = u.name;
    row.appendChild(av);
    row.appendChild(name);
    if (u.in_voice) {
      const mic = document.createElement('span');
      mic.className = 'mic-icon';
      mic.textContent = u.muted ? '🔇' : '🎙';
      row.appendChild(mic);
    }
    list.appendChild(row);
  }

  const inVoiceUsers = users.filter(u => u.in_voice);
  const inline = document.getElementById('voice-users-inline');
  inline.innerHTML = '';
  for (const u of inVoiceUsers) {
    const row = document.createElement('div');
    row.className = 'voice-user-inline' + (u.speaking ? ' speaking' : '');
    const av = document.createElement('div');
    av.className = 'avatar mini-avatar';
    renderAvatar(av, u.name, u.color || colorFor(u.name));
    av.querySelector('.status-dot').style.display = 'none';
    row.appendChild(av);
    const span = document.createElement('span');
    span.textContent = u.name + (u.muted ? ' 🔇' : '');
    row.appendChild(span);
    inline.appendChild(row);
  }
}

// ================= Composer =================
async function send() {
  const textEl = document.getElementById('text');
  const text = textEl.value.trim();
  if (!text) return;
  textEl.value = '';
  autoGrow(textEl);
  wsSend({type: 'chat', name: myName, text});
}

function autoGrow(el) {
  el.style.height = 'auto';
  el.style.height = Math.min(el.scrollHeight, 140) + 'px';
}

const textEl = document.getElementById('text');
textEl.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); send(); }
});
let lastTypingSent = 0;
textEl.addEventListener('input', (e) => {
  autoGrow(e.target);
  const now = Date.now();
  if (now - lastTypingSent > 1200) { lastTypingSent = now; wsSend({type: 'typing', name: myName}); }
});

async function uploadFile(file) {
  if (!file) return;
  const res = await fetch('/api/upload', { method: 'POST', headers: { 'X-Filename': encodeURIComponent(file.name || 'upload') }, body: file });
  if (!res.ok) { alert('Upload failed'); return; }
  const data = await res.json();
  wsSend({type: 'chat', name: myName, text: location.origin + data.url});
}

document.getElementById('uploadbtn').addEventListener('click', () => document.getElementById('filepicker').click());
document.getElementById('filepicker').addEventListener('change', (e) => {
  if (e.target.files[0]) uploadFile(e.target.files[0]);
  e.target.value = '';
});
document.getElementById('composer').addEventListener('dragover', (e) => { e.preventDefault(); document.getElementById('composer').classList.add('dragover'); });
document.getElementById('composer').addEventListener('dragleave', () => document.getElementById('composer').classList.remove('dragover'));
document.getElementById('composer').addEventListener('drop', (e) => {
  e.preventDefault();
  document.getElementById('composer').classList.remove('dragover');
  if (e.dataTransfer.files[0]) uploadFile(e.dataTransfer.files[0]);
});
textEl.addEventListener('paste', (e) => {
  const items = e.clipboardData.items;
  for (const item of items) {
    if (item.type.startsWith('image/')) { uploadFile(item.getAsFile()); e.preventDefault(); return; }
  }
});

// ================= Emoji picker =================
let emojiData = null;
let emojiPickerCallback = null;

async function loadEmojiData() {
  const res = await fetch('/api/emoji');
  emojiData = await res.json();
}

const CATEGORY_ICONS = {
  'Smileys & Emotion': '😀', 'People & Body': '🖐️', 'Animals & Nature': '🐻',
  'Food & Drink': '🍔', 'Travel & Places': '✈️', 'Activities': '⚽',
  'Objects': '💡', 'Symbols': '❤️', 'Flags': '🏳️',
};

function buildEmojiTabs() {
  const tabs = document.getElementById('emoji-tabs');
  tabs.innerHTML = '';
  Object.keys(emojiData).forEach((group, idx) => {
    const btn = document.createElement('button');
    btn.textContent = CATEGORY_ICONS[group] || '❓';
    btn.title = group;
    btn.className = idx === 0 ? 'active' : '';
    btn.onclick = () => { scrollToCategory(group); setActiveTab(btn); };
    tabs.appendChild(btn);
  });
}

function setActiveTab(activeBtn) {
  document.querySelectorAll('#emoji-tabs button').forEach(b => b.classList.remove('active'));
  activeBtn.classList.add('active');
}

function scrollToCategory(group) {
  const el = document.getElementById('cat-' + slug(group));
  if (el) el.scrollIntoView({block: 'start'});
}

function slug(s) { return s.replace(/[^a-zA-Z0-9]+/g, '-'); }

function buildEmojiList(filter) {
  const list = document.getElementById('emoji-list');
  list.innerHTML = '';
  const f = (filter || '').trim().toLowerCase();
  for (const group in emojiData) {
    const subgroups = emojiData[group];
    let groupHasMatch = false;
    const groupWrap = document.createElement('div');
    const label = document.createElement('div');
    label.className = 'cat-label';
    label.id = 'cat-' + slug(group);
    label.textContent = group;
    groupWrap.appendChild(label);
    const grid = document.createElement('div');
    grid.className = 'cat-grid';
    for (const sub in subgroups) {
      for (const item of subgroups[sub]) {
        if (f && !item.n.toLowerCase().includes(f)) continue;
        groupHasMatch = true;
        const span = document.createElement('span');
        span.className = 'em';
        span.textContent = item.e;
        span.title = item.n;
        span.onclick = () => {
          if (emojiPickerCallback) emojiPickerCallback(item.e);
          else insertEmojiIntoComposer(item.e);
        };
        grid.appendChild(span);
      }
    }
    groupWrap.appendChild(grid);
    if (groupHasMatch) list.appendChild(groupWrap);
  }
}

function insertEmojiIntoComposer(emoji) {
  const el = document.getElementById('text');
  const start = el.selectionStart, end = el.selectionEnd;
  el.value = el.value.slice(0, start) + emoji + el.value.slice(end);
  el.selectionStart = el.selectionEnd = start + emoji.length;
  el.focus();
}

function openEmojiPicker(anchorEl, callback) {
  emojiPickerCallback = callback || null;
  const picker = document.getElementById('emojipicker');
  picker.classList.add('show');
  if (!emojiData) loadEmojiData().then(() => { buildEmojiTabs(); buildEmojiList(''); });
  else { buildEmojiTabs(); buildEmojiList(''); }
}

function closeEmojiPicker() {
  document.getElementById('emojipicker').classList.remove('show');
  emojiPickerCallback = null;
}

document.getElementById('emojibtn').addEventListener('click', (e) => {
  const picker = document.getElementById('emojipicker');
  if (picker.classList.contains('show') && emojiPickerCallback === null) { closeEmojiPicker(); return; }
  openEmojiPicker(e.target, null);
});
document.getElementById('emoji-search').addEventListener('input', (e) => buildEmojiList(e.target.value));
document.addEventListener('click', (e) => {
  const picker = document.getElementById('emojipicker');
  if (!picker.contains(e.target) && e.target.id !== 'emojibtn' && !e.target.closest('.react-add')) {
    closeEmojiPicker();
  }
});

// ================= Voice chat =================
let audioCtx = null, micStream = null, processorNode = null, sourceNode = null;
let inVoice = false, muted = true, pttActive = false;
const SEND_SAMPLE_RATE = 16000;
const nextPlayTime = {};
const gainNodes = {};

function downsampleTo16k(float32, inRate) {
  if (inRate === SEND_SAMPLE_RATE) return float32;
  const ratio = inRate / SEND_SAMPLE_RATE;
  const outLen = Math.floor(float32.length / ratio);
  const out = new Float32Array(outLen);
  for (let i = 0; i < outLen; i++) out[i] = float32[Math.floor(i * ratio)];
  return out;
}

function floatTo16BitPCM(float32, gain) {
  const out = new Int16Array(float32.length);
  for (let i = 0; i < float32.length; i++) {
    let s = Math.max(-1, Math.min(1, float32[i] * gain));
    out[i] = s < 0 ? s * 0x8000 : s * 0x7FFF;
  }
  return out;
}

function rms(float32) {
  let sum = 0;
  for (let i = 0; i < float32.length; i++) sum += float32[i] * float32[i];
  return Math.sqrt(sum / float32.length);
}

let lastSpeakingSent = false, lastSpeakingSentAt = 0;

function effectiveMuted() {
  if (settings.voice_mode === 'ptt') return !pttActive;
  return muted;
}

async function startVoice() {
  try {
    micStream = await navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true } });
  } catch (e) {
    alert('Could not access microphone: ' + e.message + '\n\nVoice chat requires HTTPS. If the address bar shows http://, ask the host to enable TLS (the default).');
    return;
  }
  audioCtx = new (window.AudioContext || window.webkitAudioContext)();
  sourceNode = audioCtx.createMediaStreamSource(micStream);
  processorNode = audioCtx.createScriptProcessor(4096, 1, 1);
  sourceNode.connect(processorNode);
  const silentSink = audioCtx.createGain();
  silentSink.gain.value = 0;
  processorNode.connect(silentSink);
  silentSink.connect(audioCtx.destination);

  processorNode.onaudioprocess = (e) => {
    const input = e.inputBuffer.getChannelData(0);
    const currentlyMuted = effectiveMuted();
    const level = rms(input);
    const speakingNow = level > 0.02 && !currentlyMuted;
    const now = Date.now();
    if (speakingNow !== lastSpeakingSent && now - lastSpeakingSentAt > 150) {
      lastSpeakingSent = speakingNow;
      lastSpeakingSentAt = now;
      wsSend({type: 'speaking', value: speakingNow});
    }
    if (currentlyMuted) return;
    const down = downsampleTo16k(input, audioCtx.sampleRate);
    const gain = (settings.input_volume || 100) / 100;
    const pcm16 = floatTo16BitPCM(down, gain);
    if (ws && ws.readyState === WebSocket.OPEN) ws.send(pcm16.buffer);
  };

  inVoice = true;
  updateVoiceButtons();
  wsSend({type: 'voice-join'});
  wsSend({type: 'mute', muted: effectiveMuted()});
}

function stopVoice() {
  if (micStream) micStream.getTracks().forEach(t => t.stop());
  if (processorNode) { processorNode.disconnect(); processorNode.onaudioprocess = null; }
  if (sourceNode) sourceNode.disconnect();
  if (inVoice) wsSend({type: 'voice-leave'});
  inVoice = false; muted = true; pttActive = false;
  updateVoiceButtons();
}

function toggleMute() {
  if (!inVoice || settings.voice_mode === 'ptt') return;
  muted = !muted;
  wsSend({type: 'mute', muted: effectiveMuted()});
  updateVoiceButtons();
}

function updateVoiceButtons() {
  const joinBtn = document.getElementById('voicejoin-btn');
  const muteBtn = document.getElementById('mute-btn');
  joinBtn.classList.toggle('active', inVoice);
  joinBtn.title = inVoice ? 'Leave Voice' : 'Join Voice';
  joinBtn.textContent = inVoice ? '📴' : '🎙';
  muteBtn.disabled = !inVoice;
  const isMuted = effectiveMuted();
  muteBtn.classList.toggle('active-mute', isMuted && inVoice);
  muteBtn.textContent = isMuted ? '🔇' : '🔊';
  muteBtn.title = settings.voice_mode === 'ptt' ? `Push-to-talk: hold ${settings.ptt_key}` : (isMuted ? 'Unmute' : 'Mute');
  document.getElementById('my-state').textContent = inVoice ? (isMuted ? 'Voice connected (muted)' : 'Voice connected') : 'Not connected';
}

document.getElementById('voicejoin-btn').addEventListener('click', () => { if (inVoice) stopVoice(); else startVoice(); });
document.getElementById('mute-btn').addEventListener('click', toggleMute);
document.getElementById('voice-channel-row').addEventListener('click', () => { if (!inVoice) startVoice(); });

// Push-to-talk key handling
window.addEventListener('keydown', (e) => {
  if (settings.voice_mode !== 'ptt' || !inVoice) return;
  if (document.activeElement === document.getElementById('text')) return;
  if (keyLabel(e) === settings.ptt_key && !pttActive) {
    pttActive = true;
    wsSend({type: 'mute', muted: false});
    updateVoiceButtons();
  }
});
window.addEventListener('keyup', (e) => {
  if (settings.voice_mode !== 'ptt' || !inVoice) return;
  if (keyLabel(e) === settings.ptt_key && pttActive) {
    pttActive = false;
    wsSend({type: 'mute', muted: true});
    updateVoiceButtons();
  }
});
function keyLabel(e) {
  if (e.code === 'Space') return 'Space';
  return e.key.length === 1 ? e.key.toUpperCase() : e.key;
}

function handleVoiceFrame(buf) {
  const bytes = new Uint8Array(buf);
  if (bytes.length < 3) return;
  const senderId = bytes[0];
  const pcmBytes = bytes.slice(1);
  const int16 = new Int16Array(pcmBytes.buffer, pcmBytes.byteOffset, pcmBytes.length / 2);
  const float32 = new Float32Array(int16.length);
  for (let i = 0; i < int16.length; i++) float32[i] = int16[i] / 32768;

  if (!audioCtx) audioCtx = new (window.AudioContext || window.webkitAudioContext)();
  const buffer = audioCtx.createBuffer(1, float32.length, SEND_SAMPLE_RATE);
  buffer.copyToChannel(float32, 0);
  const src = audioCtx.createBufferSource();
  src.buffer = buffer;
  if (!gainNodes[senderId]) {
    gainNodes[senderId] = audioCtx.createGain();
    gainNodes[senderId].connect(audioCtx.destination);
  }
  gainNodes[senderId].gain.value = (settings.output_volume || 100) / 100;
  src.connect(gainNodes[senderId]);
  const now = audioCtx.currentTime;
  const startAt = Math.max(now + 0.02, nextPlayTime[senderId] || 0);
  src.start(startAt);
  nextPlayTime[senderId] = startAt + buffer.duration;
}

// ================= Settings modal =================
function applySettingsToUI() {
  document.getElementById('set-voice-mode').value = settings.voice_mode;
  document.getElementById('ptt-row').style.display = settings.voice_mode === 'ptt' ? 'block' : 'none';
  document.getElementById('ptt-key-btn').textContent = settings.ptt_key;
  document.getElementById('set-input-volume').value = settings.input_volume;
  document.getElementById('set-output-volume').value = settings.output_volume;
  updateVoiceButtons();
}

document.getElementById('settings-btn').addEventListener('click', () => {
  document.getElementById('settings-modal').classList.add('show');
});
document.getElementById('settings-close').addEventListener('click', () => {
  document.getElementById('settings-modal').classList.remove('show');
});
document.querySelectorAll('#settings-sidebar .item').forEach(item => {
  item.addEventListener('click', () => {
    document.querySelectorAll('#settings-sidebar .item').forEach(i => i.classList.remove('active'));
    item.classList.add('active');
    document.querySelectorAll('.pane').forEach(p => p.style.display = 'none');
    document.getElementById('pane-' + item.dataset.pane).style.display = 'block';
  });
});

async function persistSettings() {
  if (!authenticated) return; // guests: settings apply locally only for the session
  await fetch('/api/settings', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify(settings) });
}

document.getElementById('set-voice-mode').addEventListener('change', (e) => {
  settings.voice_mode = e.target.value;
  document.getElementById('ptt-row').style.display = settings.voice_mode === 'ptt' ? 'block' : 'none';
  updateVoiceButtons();
  persistSettings();
});
document.getElementById('set-input-volume').addEventListener('input', (e) => { settings.input_volume = parseInt(e.target.value); persistSettings(); });
document.getElementById('set-output-volume').addEventListener('input', (e) => { settings.output_volume = parseInt(e.target.value); persistSettings(); });

let listeningForKey = false;
document.getElementById('ptt-key-btn').addEventListener('click', (e) => {
  listeningForKey = true;
  e.target.textContent = 'Press a key...';
  e.target.classList.add('listening');
});
window.addEventListener('keydown', (e) => {
  if (!listeningForKey) return;
  e.preventDefault();
  settings.ptt_key = keyLabel(e);
  const btn = document.getElementById('ptt-key-btn');
  btn.textContent = settings.ptt_key;
  btn.classList.remove('listening');
  listeningForKey = false;
  persistSettings();
});

document.getElementById('logout-btn').addEventListener('click', async () => {
  await fetch('/api/logout', { method: 'POST' });
  location.reload();
});

// ================= Boot =================
checkExistingSession();
</script>
</body>
</html>
"""
