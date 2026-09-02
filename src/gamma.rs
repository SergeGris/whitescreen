//! gamma.rs
//!
//! Low-CPU, non-intrusive monitor for `zwlr-gamma-control-v1`. It reports
//! whether *another* client (a night-light / colour-temperature tool such as
//! wlsunset, gammastep or wl-gammarelay) currently holds gamma control.
//!
//! # How detection works (and why it must poll)
//!
//! `wlr-gamma-control-v1` has **no event** that fires when some other client
//! grabs or releases gamma control. The only way to learn the state is to ask
//! for control yourself:
//!
//! * the compositor replies `gamma_size` → control was free, we now hold it
//!   (we immediately release it again), so → **inactive**;
//! * the compositor replies `failed` → another client already holds exclusive
//!   control, so → **active**.
//!
//! Because there is no change notification, *both* transitions are discovered
//! by polling. The earlier version of this file claimed sub-millisecond
//! "reactive" deactivation by blocking on the socket; that is not possible with
//! this protocol — once you receive `failed` the object is dead and no further
//! event arrives. That code in fact polled too, just with extra machinery.
//!
//! # Why it is not CPU-heavy
//!
//! The Wayland connection is opened **once** and the manager + outputs stay
//! bound for the life of the listener. Each poll is therefore a single
//! `get_gamma_control` request and one reply on the already-open socket — a
//! handful of syscalls. Between polls the worker thread is fully suspended in
//! `park_timeout`, so idle CPU is effectively zero.
//!
//! | Transition          | Detection latency      |
//! |---------------------|------------------------|
//! | inactive → active   | ≤ `POLL_INACTIVE`      |
//! | active → inactive   | ≤ `POLL_ACTIVE`        |
//!
//! # Non-intrusiveness
//!
//! The probe acquires and instantly releases control. On a spec-compliant
//! compositor (single-owner semantics, where a second requester receives
//! `failed`) this never disturbs the active client. The poll intervals are
//! deliberately gentle; raise them if a particular compositor ever flickers.
//!
//! # Threading / delivery
//!
//! One background thread owns every Wayland object. The user callback is
//! invoked **on that worker thread** — it must be `Send` and must not touch GTK
//! widgets directly. Marshal the `bool` to the GLib main thread on the caller's
//! side (e.g. an `async-channel` drained by `glib::spawn_future_local`).
//!
//! # Cargo.toml (only needed with the `gamma` feature)
//! ```toml
//! wayland-client        = "0.31"
//! wayland-protocols-wlr = { version = "0.3", features = ["client"] }
//! ```

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use wayland_client::{
    protocol::{
        wl_output::WlOutput,
        wl_registry::{self, WlRegistry},
    },
    Connection, Dispatch, EventQueue, QueueHandle,
};
use wayland_protocols_wlr::gamma_control::v1::client::{
    zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1,
    zwlr_gamma_control_v1::{self, ZwlrGammaControlV1},
};

/// Poll cadence while *no* external gamma client is active (waiting for one to
/// appear). This is the common idle case, so it is the slower of the two.
const POLL_INACTIVE: Duration = Duration::from_millis(1000);

/// Poll cadence while an external gamma client *is* active (waiting for it to
/// release). Slightly snappier so the UI clears promptly when a filter is
/// switched off — the case where the user is actually watching.
const POLL_ACTIVE: Duration = Duration::from_millis(500);

// ─────────────────────────────────────────────────────────────────────────────
// Public handle
// ─────────────────────────────────────────────────────────────────────────────

/// Reactive, non-intrusive gamma-control status monitor.
///
/// The callback receives:
/// - `true`  → another client holds the lock (a colour filter is active),
/// - `false` → the lock is free (no external gamma adjustment).
///
/// It fires once immediately with the current state and again on every change.
/// Dropping the listener stops monitoring promptly.
pub struct GammaListener {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl GammaListener {
    /// Start monitoring. `callback` runs on a background thread (see the module
    /// docs on delivery); it must be `Send`.
    pub fn new<F>(callback: F) -> Self
    where
        F: Fn(bool) + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_worker = stop.clone();

        let worker = thread::Builder::new()
            .name("gamma-monitor".into())
            .spawn(move || {
                // If Wayland or the protocol is unavailable, there is nothing
                // to watch — exit quietly.
                let Some(mut prober) = Prober::new() else {
                    return;
                };

                let mut last: Option<bool> = None;

                while !stop_worker.load(Ordering::Relaxed) {
                    let active = prober.probe();

                    if Some(active) != last {
                        last = Some(active);
                        callback(active);
                    }

                    if stop_worker.load(Ordering::Relaxed) {
                        break;
                    }

                    // Fully suspended here; Drop wakes us early via unpark().
                    thread::park_timeout(if active { POLL_ACTIVE } else { POLL_INACTIVE });
                }
            })
            .expect("failed to spawn gamma-monitor thread");

        GammaListener {
            stop,
            worker: Some(worker),
        }
    }

    /// Synchronous one-shot check. Independent of any running monitor.
    pub fn is_gamma_active() -> bool {
        Prober::new().map(|mut p| p.probe()).unwrap_or(false)
    }
}

impl Drop for GammaListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            // Wake the thread immediately instead of waiting out park_timeout.
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Persistent prober
// ─────────────────────────────────────────────────────────────────────────────

/// A live Wayland connection with the gamma-control manager and every output
/// bound. Reused across all probes so no per-probe connection setup is needed.
struct Prober {
    conn: Connection,
    eq: EventQueue<State>,
    qh: QueueHandle<State>,
    state: State,
}

impl Prober {
    /// Connect and bind globals. Returns `None` if the compositor does not
    /// expose `zwlr-gamma-control-v1` or has no outputs.
    fn new() -> Option<Self> {
        let conn = Connection::connect_to_env().ok()?;
        let mut eq: EventQueue<State> = conn.new_event_queue();
        let qh = eq.handle();
        let _registry = conn.display().get_registry(&qh, ());

        let mut state = State::default();
        // One roundtrip collects every wl_registry::global event.
        eq.roundtrip(&mut state).ok()?;

        if state.manager.is_none() || state.outputs.is_empty() {
            return None;
        }

        Some(Prober {
            conn,
            eq,
            qh,
            state,
        })
    }

    /// Returns `true` if any output is currently under another client's
    /// exclusive gamma control.
    ///
    /// For each output we request control and read the single immediate reply:
    /// `failed` ⇒ taken, `gamma_size` ⇒ free (we release it again at once).
    fn probe(&mut self) -> bool {
        // Pick up output hot-plug/unplug since the previous probe.
        if self.eq.roundtrip(&mut self.state).is_err() {
            return false;
        }

        let Some(manager) = self.state.manager.clone() else {
            return false;
        };
        let outputs: Vec<WlOutput> =
            self.state.outputs.iter().map(|(_, o)| o.clone()).collect();

        let mut active = false;
        for output in &outputs {
            self.state.done = false;
            self.state.taken = false;

            let gc = manager.get_gamma_control(output, &self.qh, ());

            // The reply (gamma_size or failed) is sent immediately on creation,
            // so a single roundtrip is guaranteed to deliver it.
            if self.eq.roundtrip(&mut self.state).is_err() {
                gc.destroy();
                return active;
            }

            // Release control right away; wayland-client does not send the
            // destructor on drop, so we must call destroy() explicitly.
            gc.destroy();
            let _ = self.conn.flush();

            if self.state.done && self.state.taken {
                active = true;
            }
        }

        active
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Wayland state
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct State {
    manager: Option<ZwlrGammaControlManagerV1>,
    /// (registry name, output) pairs, kept current via registry events.
    outputs: Vec<(u32, WlOutput)>,

    /// Set once the current probe's gamma control has replied.
    done: bool,
    /// `true`  = `failed` received (another client holds the lock),
    /// `false` = `gamma_size` received (we acquired it → lock was free).
    taken: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Dispatch implementations
// ─────────────────────────────────────────────────────────────────────────────

impl Dispatch<WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => match interface.as_str() {
                "zwlr_gamma_control_manager_v1" if state.manager.is_none() => {
                    state.manager = Some(
                        registry.bind::<ZwlrGammaControlManagerV1, (), Self>(name, 1, qh, ()),
                    );
                }
                "wl_output" => {
                    let bind_ver = version.min(4);
                    let output = registry.bind::<WlOutput, (), Self>(name, bind_ver, qh, ());
                    state.outputs.push((name, output));
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => {
                state.outputs.retain(|(n, _)| *n != name);
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrGammaControlV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrGammaControlV1,
        event: zwlr_gamma_control_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            // We acquired it → the lock was free → nobody else holds it.
            zwlr_gamma_control_v1::Event::GammaSize { .. } => {
                state.taken = false;
                state.done = true;
            }
            // Another client already holds exclusive control.
            zwlr_gamma_control_v1::Event::Failed => {
                state.taken = true;
                state.done = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrGammaControlManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrGammaControlManagerV1,
        _: <ZwlrGammaControlManagerV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlOutput, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlOutput,
        _: <WlOutput as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

// Required because roundtrip() issues a wl_display.sync() internally.
impl Dispatch<wayland_client::protocol::wl_callback::WlCallback, ()> for State {
    fn event(
        _: &mut Self,
        _: &wayland_client::protocol::wl_callback::WlCallback,
        _: <wayland_client::protocol::wl_callback::WlCallback as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
