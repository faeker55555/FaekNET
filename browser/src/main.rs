// Suppress the console window on Windows release builds, matching the
// main GUI's convention (see gui/src/main.rs) -- debug builds still show
// a console so stderr/panic output stays visible during development.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! meow-meow's in-app browser: a small standalone window with its own
//! chrome (address bar + back/forward/reload/home, styled to match the
//! main GUI's dark "ops console" identity) wrapping a real embedded
//! webview for the actual page content. See Cargo.toml's top comment for
//! why this is a separate process from the main native GUI rather than a
//! panel embedded inside it.
//!
//! Usage: `meow-meow_browser [start-url]`. With no argument, starts on a
//! built-in "mesh home page" listing every currently-configured peer's
//! local domain name as a clickable shortcut (read directly from
//! mesh.toml in the current directory, same as the CLI/GUI do) -- so
//! launching the browser from the main GUI with no specific target still
//! gives you something immediately useful: one click to open any
//! friend's locally-hosted game server admin panel, Plex, whatever.
//!
//! ## Linux: X11 vs Wayland
//!
//! wry's webviews are WebKitGTK widgets under the hood. Embedding one via
//! a raw window handle (`WebViewBuilder::build`/`build_as_child`, the
//! same cross-platform API used on Windows/macOS) only actually works
//! under X11 -- under Wayland (the default session on many modern
//! distros: CachyOS, Fedora Workstation, recent Ubuntu/GNOME, ...) tao's
//! `window_handle()` returns a Wayland surface handle, which wry's X11-
//! only embedding path rejects outright with `Error::UnsupportedWindowHandle`.
//! That's the exact crash this file works around: instead of using
//! `HasWindowHandle` on Linux, both webviews are built as native GTK
//! widgets (`WebViewBuilderExtUnix::build_gtk`) inside a `gtk::Fixed`
//! container obtained from the tao window via `WindowExtUnix::gtk_window`,
//! which works identically under X11 and Wayland. Non-Linux platforms
//! keep using the ordinary `HasWindowHandle`-based path, which is the
//! only one available (and the correct/idiomatic one) there anyway.

use std::cell::RefCell;
use std::rc::Rc;

use tao::dpi::{LogicalPosition, LogicalSize};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
use tao::window::WindowBuilder;
#[cfg(target_os = "linux")]
use tao::platform::unix::WindowBuilderExtUnix;
#[cfg(target_os = "linux")]
use tao::platform::unix::WindowExtUnix;
#[cfg(target_os = "linux")]
use gtk::prelude::{ContainerExt, WidgetExt};
use wry::{WebView, WebViewBuilder};
#[cfg(target_os = "linux")]
use wry::WebViewBuilderExtUnix;

const CHROME_HEIGHT: f64 = 44.0;

// Visual identity matching gui/src/theme.rs: dark "network operations
// console" palette. Duplicated here (rather than sharing a crate) since
// this chrome is plain HTML/CSS, not egui widgets -- but the actual color
// values are kept in lockstep with theme.rs by eye; see that file if the
// palette ever changes.
const CHROME_HTML: &str = r##"<!DOCTYPE html>
<html><head><meta charset="utf-8"><style>
  * { box-sizing: border-box; }
  html, body { margin: 0; padding: 0; background: #121518; overflow: hidden; }
  body {
    background: #121518;
    display: flex; align-items: center; gap: 6px;
    padding: 6px 8px;
    font-family: 'JetBrains Mono', 'Consolas', 'DejaVu Sans Mono', monospace;
    height: 100vh;
    border-bottom: 1px solid #242a2e;
  }
  button {
    background: #1a1e22; color: #a8b4ba; border: 1px solid #242a2e;
    font-family: inherit; font-size: 13px; cursor: pointer;
    padding: 5px 10px; border-radius: 0;
  }
  button:hover { background: #22282c; color: #e8edf0; border-color: #2dd4bf; }
  button:active { background: #175c54; }
  button:disabled { opacity: 0.35; cursor: default; }
  button:disabled:hover { background: #1a1e22; border-color: #242a2e; color: #a8b4ba; }
  #url {
    flex: 1; background: #0e1013; color: #e8edf0; border: 1px solid #242a2e;
    font-family: inherit; font-size: 13px; padding: 6px 10px; outline: none;
  }
  #url:focus { border-color: #2dd4bf; }
  #status {
    color: #5f6b72; font-size: 11px; min-width: 70px; text-align: right;
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }
  #brand {
    color: #2dd4bf; font-weight: bold; font-size: 12px; letter-spacing: 0.5px;
    padding-right: 4px; white-space: nowrap;
  }
</style></head>
<body>
  <div id="brand">MEOW_MEOW ▸ BROWSER</div>
  <button id="back" title="Back">&#8592;</button>
  <button id="fwd" title="Forward">&#8594;</button>
  <button id="reload" title="Reload">&#8635;</button>
  <button id="home" title="Home">&#8962;</button>
  <input id="url" spellcheck="false" placeholder="alice.mesh or https://..." />
  <div id="status">idle</div>
<script>
  const send = (kind, value) => window.ipc.postMessage(JSON.stringify({kind, value}));
  document.getElementById('back').onclick = () => send('back', '');
  document.getElementById('fwd').onclick = () => send('forward', '');
  document.getElementById('reload').onclick = () => send('reload', '');
  document.getElementById('home').onclick = () => send('home', '');
  const urlBox = document.getElementById('url');
  urlBox.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') { send('navigate', urlBox.value); urlBox.blur(); }
  });
  window.__setUrl = (u) => { if (document.activeElement !== urlBox) urlBox.value = u; };
  window.__setStatus = (s) => { document.getElementById('status').textContent = s; };
  window.__setNavState = (canBack, canFwd) => {
    document.getElementById('back').disabled = !canBack;
    document.getElementById('fwd').disabled = !canFwd;
  };
</script>
</body></html>"##;

enum ChromeMsg {
    Navigate(String),
    Back,
    Forward,
    Reload,
    Home,
}

/// Reads mesh.toml in the current directory (same convention the CLI and
/// GUI use) and builds a tiny local "mesh home page" listing every
/// currently-known name -> address mapping as a clickable link, so the
/// browser is immediately useful even with no starting URL.
fn build_home_page() -> String {
    let entries: Vec<(String, String)> = match meow-meow_core::config::Config::load() {
        Ok(cfg) => {
            let mut infos = vec![meow-meow_core::hosts::PeerDomainInfo {
                name: cfg.me.name.clone(),
                virtual_ip: cfg.me.virtual_ip,
                services: cfg.services.iter().map(|s| (s.name.clone(), s.port)).collect(),
            }];
            for p in &cfg.peers {
                infos.push(meow-meow_core::hosts::PeerDomainInfo {
                    name: p.name.clone(),
                    virtual_ip: p.virtual_ip,
                    // Peers' own services are only known once the mesh is
                    // actually running and gossip has delivered them --
                    // mesh.toml (what this standalone reader can see)
                    // never stores other peers' services.
                    services: Vec::new(),
                });
            }
            meow-meow_core::hosts::build_entries_with_services(&cfg.me.domain_suffix, &infos)
                .into_iter()
                .map(|e| (e.hostname, e.virtual_ip.to_string()))
                .collect()
        }
        Err(_) => Vec::new(),
    };

    let rows = if entries.is_empty() {
        "<p class=\"dim\">No mesh.toml found in the current directory, or no peers configured yet. \
         Start the mesh first, then relaunch the browser -- or just type an address above.</p>"
            .to_string()
    } else {
        let mut s = String::from("<div class=\"grid\">");
        for (name, ip) in &entries {
            s.push_str(&format!(
                "<a class=\"card\" href=\"http://{name}/\"><div class=\"n\">{name}</div><div class=\"i\">{ip}</div></a>"
            ));
        }
        s.push_str("</div>");
        s
    };

    format!(
        r##"<!DOCTYPE html><html><head><meta charset="utf-8"><style>
        body {{ background:#0b0d0f; color:#a8b4ba; font-family:'JetBrains Mono','Consolas',monospace;
               margin:0; padding:40px; }}
        h1 {{ color:#2dd4bf; font-size:20px; letter-spacing:1px; margin-bottom:4px; }}
        .sub {{ color:#5f6b72; font-size:12px; margin-bottom:28px; }}
        .grid {{ display:grid; grid-template-columns: repeat(auto-fill, minmax(180px,1fr)); gap:12px; }}
        .card {{ display:block; background:#121518; border:1px solid #242a2e; padding:14px 16px;
                 text-decoration:none; color:#e8edf0; transition: border-color .1s; }}
        .card:hover {{ border-color:#2dd4bf; background:#1a1e22; }}
        .n {{ font-size:14px; font-weight:bold; }}
        .i {{ font-size:11px; color:#5f6b72; margin-top:4px; }}
        .dim {{ color:#5f6b72; }}
        </style></head><body>
        <h1>MEOW_MEOW ▸ MESH HOME</h1>
        <div class="sub">Local domain names on this mesh -- click to open.</div>
        {rows}
        </body></html>"##
    )
}

fn resolve_start_target(arg: Option<String>) -> String {
    match arg {
        Some(a) if !a.is_empty() => normalize_address(&a),
        _ => "about:home".to_string(),
    }
}

/// Turns whatever the user typed (a bare mesh name, a host:port, a full
/// URL, ...) into something a webview will actually load: bare
/// hostnames/host:port get an `http://` prefix (mesh-hosted admin panels
/// are overwhelmingly plain HTTP on a LAN), anything that already looks
/// like it has a scheme is passed through untouched.
fn normalize_address(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed == "about:home" {
        return trimmed.to_string();
    }
    if trimmed.contains("://") {
        return trimmed.to_string();
    }
    format!("http://{trimmed}")
}

fn data_url_for(html: &str) -> String {
    format!("data:text/html;charset=utf-8,{}", percent_encode(html))
}

/// Minimal percent-encoding sufficient for embedding our own known-safe
/// generated HTML in a data: URL -- not a general-purpose encoder, this
/// only needs to handle the characters that actually show up in
/// `build_home_page()`'s output.
fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

/// Best-effort push of the current URL into the chrome bar's text field
/// right after a navigation we initiated ourselves -- the definitive,
/// always-correct sync (including for in-page link clicks / redirects
/// wry doesn't tell us about directly) happens in `poll_and_sync_chrome`.
fn sync_chrome(chrome_view: &WebView, content_view: &WebView, start_target: &str) {
    let displayed = if start_target == "about:home" {
        "about:home".to_string()
    } else {
        content_view.url().unwrap_or_else(|_| start_target.to_string())
    };
    let escaped = displayed.replace('\\', "\\\\").replace('"', "\\\"");
    let _ = chrome_view.evaluate_script(&format!("window.__setUrl && window.__setUrl(\"{escaped}\")"));
}

fn poll_and_sync_chrome(chrome_view: &WebView, content_view: &WebView) {
    if let Ok(url) = content_view.url() {
        let escaped = url.replace('\\', "\\\\").replace('"', "\\\"");
        let _ = chrome_view.evaluate_script(&format!("window.__setUrl && window.__setUrl(\"{escaped}\")"));
        let status = if url.starts_with("data:") { "mesh home".to_string() } else { url };
        let escaped_status = status.replace('\\', "\\\\").replace('"', "\\\"");
        let _ = chrome_view.evaluate_script(&format!("window.__setStatus && window.__setStatus(\"{escaped_status}\")"));
    }
}

fn main() -> wry::Result<()> {
    let start_arg = std::env::args().nth(1);
    let start_target = resolve_start_target(start_arg);

    let event_loop: EventLoop<ChromeMsg> = EventLoopBuilder::<ChromeMsg>::with_user_event().build();
    let proxy: EventLoopProxy<ChromeMsg> = event_loop.create_proxy();

    #[allow(unused_mut)]
    let mut window_builder = WindowBuilder::new()
        .with_title("meow-meow browser")
        .with_inner_size(LogicalSize::new(1000.0, 720.0));
    #[cfg(target_os = "linux")]
    {
        // We build our own gtk::Fixed as the sole child instead of using
        // tao's automatically-created gtk::Box, since Fixed is what lets
        // us position both webviews with absolute pixel coordinates (the
        // same layout model the non-Linux Rect-based positioning uses).
        window_builder = window_builder.with_default_vbox(false);
    }
    let window = window_builder.build(&event_loop).expect("failed to create browser window");

    let size = window.inner_size().to_logical::<f64>(window.scale_factor());

    let chrome_proxy = proxy.clone();
    let ipc_handler = move |msg: wry::http::Request<String>| {
        let body = msg.into_body();
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&body) {
            let kind = parsed.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            let value = parsed.get("value").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let event = match kind {
                "navigate" => Some(ChromeMsg::Navigate(value)),
                "back" => Some(ChromeMsg::Back),
                "forward" => Some(ChromeMsg::Forward),
                "reload" => Some(ChromeMsg::Reload),
                "home" => Some(ChromeMsg::Home),
                _ => None,
            };
            if let Some(event) = event {
                let _ = chrome_proxy.send_event(event);
            }
        }
    };

    let home_page = Rc::new(build_home_page());
    let start_url = if start_target == "about:home" {
        data_url_for(&home_page)
    } else {
        start_target.clone()
    };

    #[cfg(target_os = "linux")]
    let (chrome_view, content_view, fixed) = {
        // gtk::Fixed lets us `put`/`move_` children at absolute pixel
        // coordinates and lets wry's build_gtk() see it as a
        // recognized-for-positioning container type (see
        // add_to_container in wry's webkitgtk backend) -- the same
        // capability the Rect-based `with_bounds`/`set_bounds` API gives
        // on Windows/macOS, just reached through GTK's own widget tree
        // instead of a raw window handle.
        let fixed = gtk::Fixed::new();
        window.gtk_window().add(&fixed);
        window.gtk_window().show_all();

        let chrome_view = WebViewBuilder::new()
            .with_bounds(wry::Rect {
                position: LogicalPosition::new(0.0, 0.0).into(),
                size: LogicalSize::new(size.width, CHROME_HEIGHT).into(),
            })
            .with_html(CHROME_HTML)
            .with_ipc_handler(ipc_handler)
            .build_gtk(&fixed)?;

        let content_view = WebViewBuilder::new()
            .with_bounds(wry::Rect {
                position: LogicalPosition::new(0.0, CHROME_HEIGHT).into(),
                size: LogicalSize::new(size.width, (size.height - CHROME_HEIGHT).max(0.0)).into(),
            })
            .with_back_forward_navigation_gestures(true)
            .with_url(&start_url)
            .build_gtk(&fixed)?;

        (chrome_view, content_view, fixed)
    };

    #[cfg(not(target_os = "linux"))]
    let (chrome_view, content_view) = {
        let chrome_view = WebViewBuilder::new()
            .with_bounds(wry::Rect {
                position: LogicalPosition::new(0.0, 0.0).into(),
                size: LogicalSize::new(size.width, CHROME_HEIGHT).into(),
            })
            .with_html(CHROME_HTML)
            .with_ipc_handler(ipc_handler)
            .build_as_child(&window)?;

        let content_view = WebViewBuilder::new()
            .with_bounds(wry::Rect {
                position: LogicalPosition::new(0.0, CHROME_HEIGHT).into(),
                size: LogicalSize::new(size.width, (size.height - CHROME_HEIGHT).max(0.0)).into(),
            })
            .with_back_forward_navigation_gestures(true)
            .with_url(&start_url)
            .build_as_child(&window)?;

        (chrome_view, content_view)
    };

    let content_view = Rc::new(RefCell::new(content_view));

    sync_chrome(&chrome_view, &content_view.borrow(), &start_target);

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent { event: WindowEvent::Resized(new_size), .. } => {
                let logical = new_size.to_logical::<f64>(window.scale_factor());
                #[cfg(target_os = "linux")]
                {
                    let _ = &fixed; // keep container alive; positions set via set_bounds below
                }
                let _ = chrome_view.set_bounds(wry::Rect {
                    position: LogicalPosition::new(0.0, 0.0).into(),
                    size: LogicalSize::new(logical.width, CHROME_HEIGHT).into(),
                });
                let _ = content_view.borrow().set_bounds(wry::Rect {
                    position: LogicalPosition::new(0.0, CHROME_HEIGHT).into(),
                    size: LogicalSize::new(logical.width, (logical.height - CHROME_HEIGHT).max(0.0)).into(),
                });
            }
            Event::UserEvent(msg) => {
                let view = content_view.borrow();
                match msg {
                    ChromeMsg::Navigate(raw) => {
                        let target = normalize_address(&raw);
                        let url = if target == "about:home" {
                            data_url_for(&home_page)
                        } else {
                            target
                        };
                        let _ = view.load_url(&url);
                    }
                    ChromeMsg::Back => {
                        let _ = view.evaluate_script("history.back()");
                    }
                    ChromeMsg::Forward => {
                        let _ = view.evaluate_script("history.forward()");
                    }
                    ChromeMsg::Reload => {
                        let _ = view.evaluate_script("location.reload()");
                    }
                    ChromeMsg::Home => {
                        let _ = view.load_url(&data_url_for(&home_page));
                    }
                }
                drop(view);
                poll_and_sync_chrome(&chrome_view, &content_view.borrow());
            }
            _ => {}
        }
    });
}
