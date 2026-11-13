//! What is on the screens, and what happens to it next.
//!
//! The GTK build keeps this in its window: which monitors are covered, what
//! colour each of them is, whether the queue is playing, whether the badges
//! are up. Without a control window there is nowhere obvious to put it, so
//! it lives here and is driven by the message loop.
//!
//! Everything it decides comes from `whitescreen_core` -- the queue's
//! stepping, the colours, the fill geometry -- so the two builds behave the
//! same way even though neither shares a line of UI code with the other.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use whitescreen_core::queue::{DEFAULT, Steps};
use whitescreen_core::settings::Settings;
use whitescreen_core::{Fill, Pattern, Rgba, Side, glob};

use crate::badge::Badge;
use crate::cli::Options;
use crate::config::settings_path;
use crate::monitor::{Monitor, monitors};
use crate::overlay::{Overlay, OverlayEvent};
use crate::power::KeepAwake;

/// How often the monitor list is re-read while overlays are up.
///
/// Polled rather than driven by `WM_DISPLAYCHANGE`: the overlays are the
/// only windows this process has, and a screen being unplugged is exactly
/// the case where the window that would have been told has just been
/// destroyed. A second's delay on a hot-plug is not worth a message-only
/// window to avoid.
const RESCAN_EVERY: Duration = Duration::from_secs(1);

/// How long the loop sleeps between passes when nothing is happening.
const IDLE: Duration = Duration::from_millis(10);

/// The fill the settings describe, or the plain colour when they describe
/// none. The pattern names are the ones `--pattern` takes and the GTK build
/// stores, so a profile written by either is understood by both.
fn saved_fill(settings: &Settings) -> Fill {
    let side = settings
        .region_side()
        .as_deref()
        .and_then(Side::from_key)
        .unwrap_or(Side::Full);
    settings
        .pattern()
        .as_deref()
        .and_then(Pattern::from_key)
        .unwrap_or_default()
        .fill(side, settings.second_color())
}

/// The queue, playing.
struct Cycle {
    steps: Steps,
    interval: Duration,
    next_at: Instant,
    paused: bool,
}

/// Everything on the screens right now.
pub struct Session {
    shared: Rgba,
    brightness: f32,
    fill: Fill,
    /// Colours given to particular monitors, by key. Kept even while the
    /// mode is off, exactly as the sidebar keeps them.
    per_monitor: HashMap<String, Rgba>,
    per_monitor_on: bool,
    /// The patterns naming which monitors to cover; empty means all.
    patterns: Vec<String>,
    identify: bool,

    cycle: Option<Cycle>,
    /// The queue and the dwell, kept even while the cycle is disarmed, so
    /// that arming it again picks up what was there.
    saved_queue: Vec<Rgba>,
    saved_interval: f64,
    /// Minutes before the overlays take themselves down, and when that is.
    /// The delay outlives any one showing, so that hiding and showing again
    /// starts the countdown afresh rather than losing it.
    auto_hide: Option<u32>,
    deadline: Option<Instant>,
    /// Whether the overlays are meant to be up. Distinct from whether any
    /// exist: with no monitor selected there is nothing to create.
    showing: bool,

    screens: Vec<Monitor>,
    overlays: Vec<(String, Overlay)>,
    badges: Vec<(String, Badge)>,
    last_scan: Instant,

    /// Held for as long as anything is on screen.
    _awake: KeepAwake,

    /// What was read at startup and is written back on the way out, so that
    /// a colour chosen once is still there next time -- the promise the GTK
    /// build makes, through the same file format.
    settings: Settings,
}

impl Session {
    /// Set up from one invocation, on top of whatever was saved. Nothing is
    /// shown yet.
    ///
    /// The saved state is the starting point and the flags override it, so
    /// that `whitescreen --color red` changes the colour and leaves the rest
    /// of the setup alone.
    pub fn new(opts: &Options) -> windows::core::Result<Self> {
        let settings = Settings::load(settings_path());
        let screens = monitors();

        let mut per_monitor: HashMap<String, Rgba> = settings.monitor_colors();

        // --monitor-color takes the same patterns as --monitor, and a plain
        // name is kept even when that screen is not attached -- it is
        // waiting for it, exactly as the GTK build's sidebar does.
        for (pattern, color) in &opts.monitor_colors {
            let hits: Vec<String> = screens
                .iter()
                .filter(|m| glob::matches(pattern, &m.key) || glob::matches(pattern, &m.name))
                .map(|m| m.key.clone())
                .collect();
            if hits.is_empty() && !glob::is_pattern(pattern) {
                per_monitor.insert(pattern.clone(), *color);
            }
            for key in hits {
                per_monitor.insert(key, *color);
            }
        }

        // A queue named on the command line wins; otherwise the saved one,
        // and the standard dead-pixel sequence when there is neither.
        let saved_queue = settings.queue();
        let queue = opts
            .queue
            .as_deref()
            .or(Some(saved_queue.as_slice()).filter(|q| !q.is_empty()))
            .unwrap_or(DEFAULT)
            .to_vec();
        let interval = if opts.interval == crate::cli::INTERVAL_DEFAULT {
            settings.cycle_interval().unwrap_or(crate::cli::INTERVAL_DEFAULT)
        } else {
            opts.interval
        };
        let cycle = opts.cycle.then(|| {
            let mut steps = Steps::default();
            steps.set_colors(&queue);
            Cycle {
                steps,
                interval: Duration::from_secs_f64(
                    interval.clamp(crate::cli::INTERVAL_MIN, crate::cli::INTERVAL_MAX),
                ),
                next_at: Instant::now(),
                paused: false,
            }
        });

        // A flag beats the file; the file beats the built-in default.
        let shared = if opts.color == Rgba::WHITE {
            settings.color().unwrap_or(Rgba::WHITE)
        } else {
            opts.color
        };
        let brightness = if opts.brightness == 1.0 {
            settings.brightness().unwrap_or(1.0) as f32
        } else {
            opts.brightness
        };
        let fill = if opts.fill == Fill::default() { saved_fill(&settings) } else { opts.fill };
        let auto_hide = opts.auto_hide.or_else(|| settings.auto_hide().filter(|m| *m > 0));

        Ok(Self {
            shared,
            brightness,
            fill,
            // Naming a colour for a monitor switches the mode on, as it does
            // in the sidebar; --no-monitor-colors is how it goes off again.
            per_monitor_on: !opts.no_monitor_colors
                && (!opts.monitor_colors.is_empty() || settings.per_monitor_colors())
                && !per_monitor.is_empty(),
            per_monitor,
            patterns: opts.monitors.clone(),
            identify: opts.identify,
            cycle,
            saved_queue: queue,
            saved_interval: interval.clamp(crate::cli::INTERVAL_MIN, crate::cli::INTERVAL_MAX),
            auto_hide,
            deadline: None,
            showing: false,
            screens,
            overlays: Vec::new(),
            badges: Vec::new(),
            last_scan: Instant::now(),
            _awake: KeepAwake::new(),
            settings,
        })
    }

    /// The monitors this invocation asked for, and the patterns that matched
    /// nothing.
    pub fn chosen(&self) -> (Vec<Monitor>, Vec<String>) {
        if self.patterns.is_empty() {
            return (self.screens.clone(), Vec::new());
        }
        let (hits, unmatched) = crate::monitor::select_matching(&self.screens, &self.patterns);
        (hits.into_iter().cloned().collect(), unmatched)
    }

    /// Cover the chosen screens.
    pub fn show(&mut self) -> windows::core::Result<()> {
        self.showing = true;
        self.rebuild()?;
        if let Some(minutes) = self.auto_hide {
            self.deadline = Some(Instant::now() + Duration::from_secs(u64::from(minutes) * 60));
        }
        Ok(())
    }

    /// Take them all down. The setup is kept, so showing again is one call.
    pub fn hide(&mut self) {
        self.showing = false;
        self.deadline = None;
        self.overlays.clear();
        self.badges.clear();
    }

    pub fn is_showing(&self) -> bool {
        !self.overlays.is_empty()
    }

    /// One pass: deliver what the overlays saw, move the queue on, notice a
    /// hot-plug. False once there is nothing left on screen.
    ///
    /// Split from the loop so that both callers can drive it -- the
    /// command-line run below, and the control window, which has a message
    /// loop of its own.
    pub fn tick(&mut self) -> windows::core::Result<bool> {
        if !self.showing {
            return Ok(false);
        }
        if self.drain_events() {
            self.hide();
            return Ok(false);
        }
        self.tick_cycle();
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            self.hide();
            return Ok(false);
        }
        if self.last_scan.elapsed() >= RESCAN_EVERY {
            self.last_scan = Instant::now();
            self.rescan()?;
        }
        Ok(true)
    }

    /// Cover the chosen screens and run until something takes them down.
    ///
    /// What a command-line invocation does: no window, nothing to come back
    /// to, so the process ends with the colour.
    pub fn run(&mut self) -> windows::core::Result<()> {
        self.show()?;
        if self.overlays.is_empty() {
            return Ok(());
        }

        while crate::pump() {
            if !self.tick()? {
                break;
            }
            std::thread::sleep(IDLE);
        }
        self.save();
        Ok(())
    }

    /// Write the setup back, so the next run starts where this one left off.
    ///
    /// Not the colour the queue happened to stop on: while it is playing the
    /// colour belongs to the timer, and what deserves saving is the one the
    /// user chose. The GTK build draws the same line.
    fn save(&mut self) {
        let chosen = match &self.cycle {
            Some(_) => self.settings.color().unwrap_or(self.shared),
            None => self.shared,
        };
        self.settings.set_color(chosen);
        self.settings.set_brightness(f64::from(self.brightness));
        self.settings.set_per_monitor_colors(self.per_monitor_on);

        let mut colors: Vec<(String, Rgba)> =
            self.per_monitor.iter().map(|(k, v)| (k.clone(), *v)).collect();
        colors.sort_by(|a, b| a.0.cmp(&b.0));
        self.settings.set_monitor_colors(&colors);

        if let Some(cycle) = &self.cycle {
            self.settings.set_queue(cycle.steps.colors());
            self.settings.set_cycle_interval(cycle.interval.as_secs_f64());
        }
        match self.fill {
            Fill::Region { side, rest } => {
                self.settings.set_pattern(if side == Side::Full && rest.is_none() {
                    "flat"
                } else {
                    "region"
                });
                self.settings.set_region_side(side.as_key());
                self.settings.set_second_color(rest);
            }
            Fill::Noise => self.settings.set_pattern("exerciser"),
            Fill::Wedge => self.settings.set_pattern("wedge"),
            Fill::Grid => self.settings.set_pattern("grid"),
        }
        self.settings.save();
    }

    /// Handle what the overlays have seen. True when it is time to stop.
    fn drain_events(&mut self) -> bool {
        let events: Vec<OverlayEvent> =
            self.overlays.iter().filter_map(|(_, o)| o.take_event()).collect();

        for event in events {
            match event {
                // One key on one screen takes them all down: with an overlay
                // per monitor, anything else would mean pressing a key once
                // per screen.
                OverlayEvent::Dismissed => return true,
                OverlayEvent::Next => self.step(true),
                OverlayEvent::Previous => self.step(false),
                OverlayEvent::TogglePause => {
                    if let Some(cycle) = &mut self.cycle {
                        cycle.paused = !cycle.paused;
                        cycle.next_at = Instant::now() + cycle.interval;
                    }
                }
            }
        }
        false
    }

    /// Move the queue by hand. Works whether or not the timer is running --
    /// the point is to sweep a panel at your own pace while looking at it.
    fn step(&mut self, forward: bool) {
        let Some(cycle) = &mut self.cycle else { return };
        let Some(color) = cycle.steps.step(forward) else { return };
        // A manual step restarts the dwell, so the colour just chosen gets a
        // full interval rather than whatever was left of the last one.
        cycle.next_at = Instant::now() + cycle.interval;
        self.shared = color;
        self.apply_colors();
    }

    fn tick_cycle(&mut self) {
        let Some(cycle) = &mut self.cycle else { return };
        if cycle.paused || Instant::now() < cycle.next_at {
            return;
        }
        cycle.next_at = Instant::now() + cycle.interval;
        let Some(color) = cycle.steps.advance() else { return };
        self.shared = color;
        self.apply_colors();
    }

    // ── What the control window drives ───────────────────────────────────

    /// The colour the chips set: what every monitor without one of its own
    /// shows.
    pub fn shared_color(&self) -> Rgba {
        self.shared
    }

    pub fn set_shared_color(&mut self, color: Rgba) {
        self.shared = color;
        self.apply_colors();
    }

    /// Give one monitor a colour of its own, switching the mode on -- asking
    /// for a per-monitor colour is asking for per-monitor colours.
    pub fn set_monitor_color(&mut self, key: &str, color: Rgba) {
        self.per_monitor.insert(key.to_string(), color);
        self.per_monitor_on = true;
        self.apply_colors();
    }

    /// The colour a monitor has been given, if any.
    pub fn monitor_color(&self, key: &str) -> Option<Rgba> {
        self.per_monitor.get(key).copied()
    }

    pub fn per_monitor_colors(&self) -> bool {
        self.per_monitor_on
    }

    pub fn set_per_monitor_colors(&mut self, on: bool) {
        self.per_monitor_on = on;
        self.apply_colors();
    }

    pub fn brightness(&self) -> f32 {
        self.brightness
    }

    pub fn set_brightness(&mut self, fraction: f32) {
        self.brightness = fraction.clamp(0.05, 1.0);
        self.apply_colors();
    }

    pub fn fill(&self) -> Fill {
        self.fill
    }

    pub fn set_fill(&mut self, fill: Fill) {
        self.fill = fill;
        for (_, overlay) in &self.overlays {
            overlay.set_fill(fill);
        }
    }

    pub fn auto_hide(&self) -> Option<u32> {
        self.auto_hide
    }

    pub fn set_auto_hide(&mut self, minutes: Option<u32>) {
        self.auto_hide = minutes.filter(|m| *m > 0);
        // Changing the delay restarts the countdown rather than leaving a
        // timer running against the number that was there before.
        self.deadline = match (self.showing, self.auto_hide) {
            (true, Some(m)) => Some(Instant::now() + Duration::from_secs(u64::from(m) * 60)),
            _ => None,
        };
    }

    pub fn identify(&self) -> bool {
        self.identify
    }

    pub fn set_identify(&mut self, on: bool) -> windows::core::Result<()> {
        self.identify = on;
        if !self.identify {
            self.badges.clear();
        } else if self.showing {
            self.rebuild()?;
        }
        Ok(())
    }

    /// Which monitors the overlays cover, by key. Empty means all of them.
    pub fn selection(&self) -> &[String] {
        &self.patterns
    }

    pub fn set_selection(&mut self, keys: Vec<String>) -> windows::core::Result<()> {
        self.patterns = keys;
        if self.showing {
            self.rebuild()?;
        }
        Ok(())
    }

    /// Every attached screen, as last read.
    pub fn screens(&self) -> &[Monitor] {
        &self.screens
    }

    /// Whether the queue is armed, and the queue itself.
    pub fn cycle_armed(&self) -> bool {
        self.cycle.is_some()
    }

    pub fn queue_colors(&self) -> Vec<Rgba> {
        self.cycle.as_ref().map_or_else(|| self.saved_queue.clone(), |c| c.steps.colors().to_vec())
    }

    pub fn interval(&self) -> f64 {
        self.cycle.as_ref().map_or(self.saved_interval, |c| c.interval.as_secs_f64())
    }

    pub fn set_interval(&mut self, secs: f64) {
        let secs = secs.clamp(crate::cli::INTERVAL_MIN, crate::cli::INTERVAL_MAX);
        self.saved_interval = secs;
        if let Some(cycle) = &mut self.cycle {
            cycle.interval = Duration::from_secs_f64(secs);
            cycle.next_at = Instant::now() + cycle.interval;
        }
    }

    pub fn set_queue_colors(&mut self, colors: &[Rgba]) {
        if colors.is_empty() {
            return;
        }
        self.saved_queue = colors.to_vec();
        if let Some(cycle) = &mut self.cycle {
            cycle.steps.set_colors(colors);
        }
    }

    /// Arm or disarm the cycle. Arming does not start anything on its own:
    /// the queue plays onto the screens, so it runs while they are covered
    /// and not before -- the same rule as the GTK build.
    pub fn set_cycle_armed(&mut self, armed: bool) {
        if armed == self.cycle.is_some() {
            return;
        }
        if armed {
            let mut steps = Steps::default();
            steps.set_colors(&self.saved_queue);
            self.cycle = Some(Cycle {
                steps,
                interval: Duration::from_secs_f64(self.saved_interval),
                next_at: Instant::now() + Duration::from_secs_f64(self.saved_interval),
                paused: false,
            });
        } else {
            self.cycle = None;
        }
    }

    /// What one monitor should show: its own colour if it has one and the
    /// mode is on, the shared one otherwise, dimmed either way.
    fn color_for(&self, key: &str) -> Rgba {
        let chosen = if self.per_monitor_on {
            self.per_monitor.get(key).copied().unwrap_or(self.shared)
        } else {
            self.shared
        };
        chosen.scaled(self.brightness)
    }

    fn apply_colors(&self) {
        for (key, overlay) in &self.overlays {
            overlay.set_color(self.color_for(key));
        }
    }

    /// Re-read the monitors and put the overlays back where they belong.
    ///
    /// A screen plugged in while the colour is up gets covered too, rather
    /// than staying the one conspicuously bright panel in the room.
    fn rescan(&mut self) -> windows::core::Result<()> {
        self.refresh_screens().map(|_| ())
    }

    /// Re-read the attached monitors, and say whether they changed.
    ///
    /// The control window needs this on a display change whether or not
    /// anything is on screen, which is why it does not simply call
    /// `rebuild`: that creates the overlays, and a monitor plugged in while
    /// nothing is showing must not light one up.
    pub fn refresh_screens(&mut self) -> windows::core::Result<bool> {
        let now = monitors();
        if now == self.screens {
            return Ok(false);
        }
        self.screens = now;
        if self.showing {
            self.rebuild()?;
        }
        Ok(true)
    }

    /// Make the overlays and badges match the monitors now attached.
    fn rebuild(&mut self) -> windows::core::Result<()> {
        let (chosen, _) = self.chosen();
        let wanted: Vec<String> = chosen.iter().map(|m| m.key.clone()).collect();

        // A screen that has gone takes its windows with it.
        self.overlays.retain(|(key, _)| wanted.contains(key));
        self.badges.retain(|(key, _)| wanted.contains(key));

        for screen in &chosen {
            if !self.overlays.iter().any(|(key, _)| *key == screen.key) {
                let mut overlay = Overlay::new(screen)?;
                overlay.set_fill(self.fill);
                overlay.set_color(self.color_for(&screen.key));
                overlay.show(screen)?;
                self.overlays.push((screen.key.clone(), overlay));
            }
            if self.identify && !self.badges.iter().any(|(key, _)| *key == screen.key) {
                let badge = Badge::new(screen)?;
                badge.show(screen)?;
                self.badges.push((screen.key.clone(), badge));
            }
        }
        // The badges are created after the overlays and shown last, so they
        // sit above the colour rather than under it.
        for (key, badge) in &self.badges {
            if let Some(screen) = chosen.iter().find(|m| m.key == *key) {
                badge.show(screen)?;
            }
        }
        Ok(())
    }
}
