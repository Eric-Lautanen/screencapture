//! A live take's audio switches: which sources the worker got running,
//! which of them the recording bar has muted, and which ones the bar has
//! asked to bring in after the take started.
//!
//! The mixer reads these on every chunk, so a click takes effect within one
//! 10 ms chunk instead of at the next recording. Nothing here blocks: a
//! switch is three atomics, and a button only ever flips one.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::Wanted;

/// Which sources a take has capturing, named rather than passed as a pair
/// of booleans: a swapped pair here would mute the system mix and unmute a
/// microphone that was never opened, which is a silent recording with no
/// error to show for it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Running {
    /// The microphone is capturing.
    pub mic: bool,
    /// The system mix is capturing.
    pub system: bool,
}

/// One source's state: asked for, capturing, and muted by its button.
#[derive(Debug, Default)]
struct Switch {
    /// The take wants this source. Settings set it at the start; a button
    /// sets it again for a source that never started. Clearing it stops
    /// the worker from retrying a source that refused.
    wanted: AtomicBool,
    /// Set once the source is capturing.
    opened: AtomicBool,
    /// Set while the take mutes it.
    muted: AtomicBool,
}

impl Switch {
    /// A source the take asked for, not yet capturing, and audible once
    /// it is.
    fn new(wanted: bool) -> Self {
        Self {
            wanted: AtomicBool::new(wanted),
            opened: AtomicBool::new(false),
            muted: AtomicBool::new(false),
        }
    }

    /// Whether this source is capturing.
    fn opened(&self) -> bool {
        self.opened.load(Ordering::Relaxed)
    }

    /// Whether it is capturing and not muted: what the mixer adds.
    fn audible(&self) -> bool {
        self.opened() && !self.muted.load(Ordering::Relaxed)
    }

    /// Whether the button offers this source: asked for and not muted.
    /// A source that is still opening counts as on, so a click paints the
    /// state it asked for in the same frame instead of waiting for the
    /// device; one that refused is no longer wanted and reads as off.
    fn requested(&self) -> bool {
        (self.opened() || self.wanted.load(Ordering::Relaxed))
            && !self.muted.load(Ordering::Relaxed)
    }

    /// Whether the worker has a start to make: a source that was asked
    /// for, is not capturing, and nobody has muted.
    fn awaiting(&self) -> bool {
        self.wanted.load(Ordering::Relaxed) && !self.opened() && !self.muted.load(Ordering::Relaxed)
    }

    /// Record what the worker managed: a source that did not start stops
    /// being asked for, so the worker does not retry it every chunk. Its
    /// button asks again.
    fn settle(&self, opened: bool) {
        self.opened.store(opened, Ordering::Relaxed);
        if !opened {
            self.wanted.store(false, Ordering::Relaxed);
        }
    }

    /// The switch: mute a source that is on (or still opening), or ask for
    /// one that is off. Answers the mute state afterwards.
    fn toggle(&self) -> bool {
        if self.requested() {
            self.muted.store(true, Ordering::Relaxed);
            return true;
        }
        self.wanted.store(true, Ordering::Relaxed);
        self.muted.store(false, Ordering::Relaxed);
        false
    }
}

/// The audio switches of one recording, shared by the recorder, the mixer,
/// the worker, and the bar's buttons. Cloning is cheap and every clone sees
/// the same state.
#[derive(Clone, Debug, Default)]
pub(crate) struct Live {
    /// The microphone. `None` only where no capture exists at all.
    mic: Option<Arc<Switch>>,
    /// The system mix.
    system: Option<Arc<Switch>>,
}

impl Live {
    /// Switches for both sources; `wanted` says which ones this take opens
    /// by itself. The other one keeps its button, so a take that started
    /// without a source can still bring it in.
    pub(crate) fn new(wanted: Wanted) -> Self {
        Self {
            mic: Some(Arc::new(Switch::new(wanted.mic))),
            system: Some(Arc::new(Switch::new(wanted.system))),
        }
    }

    /// `Some(muted)` for the microphone's button: off means the button
    /// offers to bring the source in, whether it is open or still opening.
    pub(crate) fn mic(&self) -> Option<bool> {
        self.mic.as_ref().map(|switch| !switch.requested())
    }

    /// `Some(muted)` for the system mix's button.
    pub(crate) fn system(&self) -> Option<bool> {
        self.system.as_ref().map(|switch| !switch.requested())
    }

    /// Whether the worker still owes a source a start, which is what keeps
    /// a button repainting until its device answers.
    pub(crate) fn starting(&self) -> bool {
        self.mic.as_ref().is_some_and(|switch| switch.awaiting())
            || self.system.as_ref().is_some_and(|switch| switch.awaiting())
    }

    /// Mute, unmute, or ask for the microphone; answers the mute state.
    pub(crate) fn toggle_mic(&self) -> Option<bool> {
        self.mic.as_ref().map(|switch| switch.toggle())
    }

    /// Mute, unmute, or ask for the system mix.
    pub(crate) fn toggle_system(&self) -> Option<bool> {
        self.system.as_ref().map(|switch| switch.toggle())
    }

    /// The sources the worker still has to start, in `(microphone, system)`
    /// order. Read every pass, so a button is a device away from a take.
    pub(crate) fn awaiting(&self) -> (bool, bool) {
        (
            self.mic.as_ref().is_some_and(|switch| switch.awaiting()),
            self.system.as_ref().is_some_and(|switch| switch.awaiting()),
        )
    }

    /// Record which sources the worker got running, and stop asking for
    /// the ones that refused.
    pub(crate) fn opened(&self, running: Running) {
        self.microphone_started(running.mic);
        self.system_started(running.system);
    }

    /// Record one microphone start, from the initial attempt or from a
    /// button pressed mid-take.
    pub(crate) fn microphone_started(&self, running: bool) {
        if let Some(switch) = &self.mic {
            switch.settle(running);
        }
    }

    /// Record one system-mix start.
    pub(crate) fn system_started(&self, running: bool) {
        if let Some(switch) = &self.system {
            switch.settle(running);
        }
    }

    /// `(microphone, system)` audible right now: what the mixer adds.
    pub(crate) fn audible(&self) -> (bool, bool) {
        (
            self.mic.as_ref().is_some_and(|switch| switch.audible()),
            self.system.as_ref().is_some_and(|switch| switch.audible()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{Live, Running, Wanted};
    use crate::audio::Quality;

    fn both() -> Live {
        Live::new(Wanted {
            mic: true,
            system: true,
            quality: Quality::High,
        })
    }

    #[test]
    fn a_source_the_take_asked_for_reads_as_on_before_it_opens() {
        let live = both();
        // The button shows what the take asked for, so the bar's first
        // frame is not a pair of switches that look off while the devices
        // connect; the mix still carries silence until they answer.
        assert_eq!(live.mic(), Some(false));
        assert_eq!(live.system(), Some(false));
        assert_eq!(live.audible(), (false, false));
        assert_eq!(live.awaiting(), (true, true), "both were asked for");
        assert!(live.starting());
    }

    #[test]
    fn the_worker_starts_what_was_asked_for_and_stops_asking() {
        let live = both();
        live.opened(Running {
            mic: true,
            system: false,
        });
        assert_eq!(live.mic(), Some(false), "capturing and audible");
        assert_eq!(live.system(), Some(true), "the system mix refused");
        assert_eq!(live.audible(), (true, false));
        assert_eq!(
            live.awaiting(),
            (false, false),
            "a source that refused is not retried behind the user's back"
        );
    }

    #[test]
    fn toggling_mutes_and_unmutes_the_mix() {
        let live = both();
        live.opened(Running {
            mic: true,
            system: true,
        });
        assert_eq!(live.toggle_mic(), Some(true));
        assert_eq!(live.mic(), Some(true));
        assert_eq!(live.audible(), (false, true), "only the mic went quiet");
        assert_eq!(live.toggle_mic(), Some(false));
        assert_eq!(live.audible(), (true, true));
        assert_eq!(live.toggle_system(), Some(true));
        assert_eq!(live.audible(), (true, false));
    }

    #[test]
    fn a_button_brings_in_a_source_the_settings_left_off() {
        let live = Live::new(Wanted {
            mic: false,
            system: true,
            quality: Quality::High,
        });
        assert_eq!(live.audible(), (false, false));
        assert_eq!(live.awaiting(), (false, true), "only the plan's source");
        // The microphone's button asks for it, and the worker picks it up
        // on its next pass.
        assert_eq!(live.toggle_mic(), Some(false));
        assert_eq!(live.awaiting(), (true, true));
        live.microphone_started(true);
        assert_eq!(live.audible(), (true, false));
        assert_eq!(live.mic(), Some(false));
        assert_eq!(live.toggle_mic(), Some(true), "and mutes it again");
    }

    #[test]
    fn a_source_that_refuses_mid_take_stops_being_asked_for() {
        let live = Live::new(Wanted::default());
        assert_eq!(live.toggle_mic(), Some(false));
        assert_eq!(live.awaiting(), (true, false));
        live.microphone_started(false);
        assert_eq!(live.awaiting(), (false, false), "no retry loop");
        assert_eq!(live.mic(), Some(true), "the button reads as off");
    }

    /// A click on a source that has to open answers at once: the button
    /// reads on in the click's own frame, and the worker's start only
    /// makes it true.
    #[test]
    fn a_click_on_an_opening_source_reads_on_before_the_device_answers() {
        let live = Live::new(Wanted::default());
        assert_eq!(live.toggle_mic(), Some(false), "the click asks for it");
        assert_eq!(live.mic(), Some(false), "and the button already says on");
        assert!(live.starting(), "the worker still owes the device");
        // A second click mutes it while it opens, rather than asking again.
        assert_eq!(live.toggle_mic(), Some(true));
        assert_eq!(live.mic(), Some(true));
        assert!(!live.starting(), "a muted source is not worth starting");
        // Unmuting after the start lands is a plain flag flip.
        live.microphone_started(true);
        assert_eq!(live.toggle_mic(), Some(false));
        assert_eq!(live.audible(), (true, false));
    }

    #[test]
    fn clones_share_one_set_of_switches() {
        let live = both();
        live.opened(Running {
            mic: true,
            system: false,
        });
        let other = live.clone();
        assert_eq!(other.toggle_mic(), Some(true));
        assert_eq!(live.mic(), Some(true), "the recorder sees the same switch");
        assert_eq!(live.audible(), (false, false));
    }

    #[test]
    fn a_plan_that_asked_for_nothing_asks_nothing() {
        let live = Live::new(Wanted::default());
        assert_eq!(live.audible(), (false, false));
        assert_eq!(live.awaiting(), (false, false));
        assert_eq!(live.mic(), Some(true), "but both buttons are there");
        assert_eq!(live.system(), Some(true));
    }
}
