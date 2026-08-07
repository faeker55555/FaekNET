// Pluggable log sink so the same mesh engine can print to a terminal (CLI)
// or feed a scrollback buffer in a native GUI, without the engine itself
// knowing or caring which. Defaults to stdout so the CLI needs zero setup;
// the GUI installs its own sink at startup before calling into the engine.
use std::sync::{Mutex, OnceLock};

pub type SinkFn = Box<dyn Fn(&str) + Send + Sync>;

static SINK: OnceLock<Mutex<Option<SinkFn>>> = OnceLock::new();

fn cell() -> &'static Mutex<Option<SinkFn>> {
    SINK.get_or_init(|| Mutex::new(None))
}

/// Installs a custom sink that receives each already-timestamped log line.
/// Call this once, before starting the mesh, to redirect log output (e.g.
/// into a GUI's log buffer) instead of stdout.
pub fn set_sink(f: SinkFn) {
    *cell().lock().unwrap() = Some(f);
}

/// Formats and emits one log line: to the installed sink if one was set
/// via `set_sink`, or to stdout otherwise.
pub fn emit(msg: &str) {
    let now = chrono::Local::now().format("%H:%M:%S");
    let line = format!("[{}] {}", now, msg);
    let guard = cell().lock().unwrap();
    match guard.as_ref() {
        Some(sink) => sink(&line),
        None => println!("{line}"),
    }
}
