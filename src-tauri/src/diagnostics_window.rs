//! The Diagnostics window and how it docks to the main window.
//!
//! By default the window is *docked*: flush to the right of the main window
//! (to the left when the right side would leave the screen), top-aligned and
//! as tall as it, and it follows every move and resize of the main window.
//! Dragging it somewhere else undocks it; the pin in its header docks it
//! again. The choice is remembered in `<app_data_dir>/diagnostics.json`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, WebviewWindow, WindowEvent};

/// The window label; `App.tsx` renders the panel when it finds itself here.
pub const LABEL: &str = "debug";
/// Space between the two windows when docked.
pub const DOCK_GAP: f64 = 8.0;
/// The width a freshly opened window gets; the user's resize is kept after.
pub const DEFAULT_WIDTH: f64 = 560.0;
const DEFAULT_HEIGHT: f64 = 700.0;
/// How far a window may sit from where docking put it before it counts as
/// dragged away (window managers round to whole pixels).
const DRIFT_TOLERANCE: f64 = 1.5;

/// A window's position and size in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Where a docked diagnostics window of `width` goes next to `main`: to the
/// right, or to the left when the right side would run off `monitor` and the
/// left fits. Neither fitting, it still goes right (clipped at the edge is
/// recoverable; off the left edge is not).
pub fn docked_frame(main: &Frame, width: f64, monitor: Option<&Frame>) -> Frame {
    let right = Frame { x: main.x + main.width + DOCK_GAP, y: main.y, width, height: main.height };
    if let Some(m) = monitor {
        if right.x + width > m.x + m.width {
            let left_x = main.x - DOCK_GAP - width;
            if left_x >= m.x {
                return Frame { x: left_x, ..right };
            }
        }
    }
    right
}

/// Whether a window now at (`x`, `y`) has left the spot docking gave it.
pub fn moved_away(expected: &Frame, x: f64, y: f64) -> bool {
    (x - expected.x).abs() > DRIFT_TOLERANCE || (y - expected.y).abs() > DRIFT_TOLERANCE
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default = "default_docked")]
    docked: bool,
}

fn default_docked() -> bool {
    true
}

/// Managed state: whether the window is docked, and where docking last put
/// it (so its own Moved event is told apart from a drag).
pub struct DiagnosticsWindow {
    docked: AtomicBool,
    expected: Mutex<Option<Frame>>,
    path: Option<PathBuf>,
}

impl DiagnosticsWindow {
    /// The remembered choice from `data_dir`, docked when there is none.
    pub fn load(data_dir: Option<PathBuf>) -> Self {
        let path = data_dir.map(|d| d.join("diagnostics.json"));
        let docked = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<Saved>(&s).ok())
            .is_none_or(|s| s.docked);
        Self { docked: AtomicBool::new(docked), expected: Mutex::new(None), path }
    }

    pub fn docked(&self) -> bool {
        self.docked.load(Ordering::SeqCst)
    }

    fn set_docked(&self, docked: bool) {
        self.docked.store(docked, Ordering::SeqCst);
        if let Some(path) = &self.path {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(json) = serde_json::to_string_pretty(&Saved { docked }) {
                if let Err(e) = std::fs::write(path, json) {
                    log::warn!("could not save the diagnostics window setting: {e}");
                }
            }
        }
    }

    fn expected(&self) -> Option<Frame> {
        *self.expected.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn remember(&self, frame: Option<Frame>) {
        *self.expected.lock().unwrap_or_else(|p| p.into_inner()) = frame;
    }
}

/// Outer position and inner size, in logical pixels: the title bar's top is
/// what lines up, the content height is what gets matched.
fn frame_of(window: &WebviewWindow) -> Option<Frame> {
    let scale = window.scale_factor().ok()?;
    let pos = window.outer_position().ok()?;
    let size = window.inner_size().ok()?;
    Some(Frame {
        x: f64::from(pos.x) / scale,
        y: f64::from(pos.y) / scale,
        width: f64::from(size.width) / scale,
        height: f64::from(size.height) / scale,
    })
}

fn monitor_of(window: &WebviewWindow) -> Option<Frame> {
    let monitor = window.current_monitor().ok()??;
    let scale = monitor.scale_factor();
    let pos = monitor.position();
    let size = monitor.size();
    Some(Frame {
        x: f64::from(pos.x) / scale,
        y: f64::from(pos.y) / scale,
        width: f64::from(size.width) / scale,
        height: f64::from(size.height) / scale,
    })
}

/// Put a docked window where it belongs next to the main window. A no-op
/// when undocked or when either window is missing.
pub fn reposition(app: &AppHandle) {
    let state = app.state::<DiagnosticsWindow>();
    if !state.docked() {
        return;
    }
    let (Some(main), Some(diag)) = (app.get_webview_window("main"), app.get_webview_window(LABEL)) else {
        return;
    };
    let Some(main_frame) = frame_of(&main) else { return };
    let width = frame_of(&diag).map_or(DEFAULT_WIDTH, |f| f.width);
    let target = docked_frame(&main_frame, width, monitor_of(&main).as_ref());
    state.remember(Some(target));
    let _ = diag.set_size(LogicalSize::new(target.width, target.height));
    let _ = diag.set_position(LogicalPosition::new(target.x, target.y));
}

/// Dock or undock from the panel's pin. Docking snaps the window back.
pub fn set_docked(app: &AppHandle, docked: bool) {
    let state = app.state::<DiagnosticsWindow>();
    state.set_docked(docked);
    if docked {
        reposition(app);
    } else {
        state.remember(None);
    }
    let _ = app.emit("diagnostics-dock", docked);
}

/// Close the window with the main window; a diagnostics window on its own
/// is not a running phone.
pub fn close(app: &AppHandle) {
    if let Some(diag) = app.get_webview_window(LABEL) {
        let _ = diag.close();
    }
}

/// Open the window, or bring it forward if it is already open.
pub fn open(app: &AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(LABEL) {
        return window.set_focus().map_err(|e| format!("Focus error: {e}"));
    }
    let state = app.state::<DiagnosticsWindow>();
    let main = app.get_webview_window("main");
    let main_frame = main.as_ref().and_then(frame_of);
    let frame = match main_frame {
        Some(mf) if state.docked() => docked_frame(&mf, DEFAULT_WIDTH, main.as_ref().and_then(monitor_of).as_ref()),
        Some(mf) => Frame { x: mf.x + mf.width + DOCK_GAP, y: mf.y, width: DEFAULT_WIDTH, height: DEFAULT_HEIGHT },
        None => Frame { x: 100.0, y: 100.0, width: DEFAULT_WIDTH, height: DEFAULT_HEIGHT },
    };
    state.remember(state.docked().then_some(frame));

    let window = tauri::WebviewWindowBuilder::new(app, LABEL, tauri::WebviewUrl::App("/".into()))
        .title("Aria — Diagnostics")
        .inner_size(frame.width, frame.height)
        .min_inner_size(420.0, 480.0)
        .position(frame.x, frame.y)
        .build()
        .map_err(|e| format!("Failed to open the diagnostics window: {e}"))?;

    // A drag away from the docked spot undocks; the window's own Moved
    // events from docking land where `expected` says and change nothing.
    let handle = app.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Moved(pos) = event {
            let state = handle.state::<DiagnosticsWindow>();
            if !state.docked() {
                return;
            }
            let Some(expected) = state.expected() else { return };
            let scale = handle
                .get_webview_window(LABEL)
                .and_then(|w| w.scale_factor().ok())
                .unwrap_or(1.0);
            if moved_away(&expected, f64::from(pos.x) / scale, f64::from(pos.y) / scale) {
                log::info!("diagnostics window dragged away; undocked");
                state.set_docked(false);
                state.remember(None);
                let _ = handle.emit("diagnostics-dock", false);
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Docked, the diagnostics window sits flush to the right of the main
    /// window, top-aligned and as tall as it.
    #[test]
    fn docked_frame_sits_to_the_right_of_the_main_window() {
        let main = Frame { x: 100.0, y: 50.0, width: 420.0, height: 720.0 };
        let frame = docked_frame(&main, 560.0, None);
        assert_eq!(frame, Frame { x: 100.0 + 420.0 + DOCK_GAP, y: 50.0, width: 560.0, height: 720.0 });
    }

    /// When the right side would leave the screen, the window docks to the
    /// left instead; when neither side fits it still goes right, clipped,
    /// rather than disappearing off the left edge.
    #[test]
    fn docked_frame_flips_to_the_left_when_the_right_does_not_fit() {
        let main = Frame { x: 1200.0, y: 50.0, width: 420.0, height: 720.0 };
        let monitor = Frame { x: 0.0, y: 0.0, width: 1920.0, height: 1080.0 };
        let frame = docked_frame(&main, 560.0, Some(&monitor));
        assert_eq!(frame.x, 1200.0 - DOCK_GAP - 560.0);
        assert_eq!(frame.y, 50.0);

        let narrow = Frame { x: 0.0, y: 0.0, width: 900.0, height: 700.0 };
        let main = Frame { x: 200.0, y: 50.0, width: 420.0, height: 600.0 };
        let frame = docked_frame(&main, 560.0, Some(&narrow));
        assert_eq!(frame.x, 200.0 + 420.0 + DOCK_GAP, "right, clipped, not off the left edge");
    }

    /// A window the user dragged somewhere else is no longer where docking
    /// put it; a window moved by docking itself (within a pixel) still is.
    #[test]
    fn a_drag_away_from_the_docked_spot_is_detected() {
        let expected = Frame { x: 528.0, y: 50.0, width: 560.0, height: 720.0 };
        assert!(!moved_away(&expected, 528.4, 50.0));
        assert!(!moved_away(&expected, 527.0, 51.0));
        assert!(moved_away(&expected, 600.0, 50.0));
        assert!(moved_away(&expected, 528.0, 200.0));
    }
}
