//! Deterministic time-injected animation tweens (`WHEEL_ENGINE_DESIGN.md` §11).

pub use crate::geometry::Millis;

/// Easing curve for [`Tween`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ease {
    /// Linear interpolation `t`.
    Linear,
    /// Decelerating cubic `1 - (1 - t)^3`.
    OutCubic,
    /// Symmetric cubic ease-in-out.
    InOutCubic,
}

impl Ease {
    /// Evaluates the easing function for `t` in `[0.0, 1.0]`.
    #[inline]
    #[must_use]
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::OutCubic => {
                let inv = 1.0 - t;
                1.0 - inv * inv * inv
            }
            Self::InOutCubic => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    let u = -2.0 * t + 2.0;
                    1.0 - (u * u * u) * 0.5
                }
            }
        }
    }
}

/// Deterministic scalar tween with interruptible retargeting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tween {
    /// Starting value at `start`.
    pub from: f32,
    /// Target value at `start + dur`.
    pub to: f32,
    /// Start timestamp in milliseconds.
    pub start: Millis,
    /// Duration in milliseconds (`0` = instantaneous).
    pub dur: u16,
    /// Easing curve.
    pub ease: Ease,
}

impl Tween {
    /// Creates a new tween from `from` to `to` starting at `start`.
    #[inline]
    #[must_use]
    pub const fn new(from: f32, to: f32, start: Millis, dur: u16, ease: Ease) -> Self {
        Self {
            from,
            to,
            start,
            dur,
            ease,
        }
    }

    /// Creates a settled tween already at `val`.
    #[inline]
    #[must_use]
    pub const fn constant(val: f32) -> Self {
        Self {
            from: val,
            to: val,
            start: 0,
            dur: 0,
            ease: Ease::Linear,
        }
    }

    /// Evaluates the tween at timestamp `now`.
    #[inline]
    #[must_use]
    pub fn value(&self, now: Millis) -> f32 {
        if now < self.start {
            return self.from;
        }
        if self.dur == 0 || now >= self.end_time() {
            return self.to;
        }
        let elapsed = (now - self.start) as f32;
        let t = elapsed / f32::from(self.dur);
        let k = self.ease.apply(t);
        self.from + (self.to - self.from) * k
    }

    /// Smoothly retargets the tween to `to` starting at `now` from its current interpolated value.
    #[inline]
    pub fn retarget(&mut self, to: f32, now: Millis, dur: u16, ease: Ease) {
        let cur = self.value(now);
        self.from = cur;
        self.to = to;
        self.start = now;
        self.dur = dur;
        self.ease = ease;
    }

    /// Returns `true` if the tween has reached `to` at `now`.
    #[inline]
    #[must_use]
    pub fn done(&self, now: Millis) -> bool {
        self.dur == 0 || now >= self.end_time() || (self.from - self.to).abs() <= 1e-5
    }

    /// Timestamp when the tween finishes.
    #[inline]
    #[must_use]
    pub const fn end_time(&self) -> Millis {
        self.start.saturating_add(self.dur as u64)
    }
}

/// Default animation durations (`WHEEL_ENGINE_DESIGN.md` §11.2).
pub mod timings {
    /// Root wheel appear duration (140 ms).
    pub const ROOT_APPEAR_MS: u16 = 140;
    /// Per-segment appear stagger (10 ms, max 60 ms).
    pub const SEGMENT_STAGGER_MS: u16 = 10;
    /// Maximum segment appear stagger (60 ms).
    pub const SEGMENT_STAGGER_MAX_MS: u16 = 60;
    /// Parent->child stem grow duration (90 ms).
    pub const STEM_GROW_MS: u16 = 90;
    /// Delay after stem starts before child wheel emerges (40 ms).
    pub const CHILD_EMERGE_DELAY_MS: u16 = 40;
    /// Child wheel emerge duration (130 ms).
    pub const CHILD_EMERGE_MS: u16 = 130;
    /// Child wheel retract duration (110 ms).
    pub const CHILD_RETRACT_MS: u16 = 110;
    /// Stem retract duration (80 ms).
    pub const STEM_RETRACT_MS: u16 = 80;
    /// Segment hover-in duration (90 ms).
    pub const HOVER_IN_MS: u16 = 90;
    /// Segment hover-out duration (120 ms).
    pub const HOVER_OUT_MS: u16 = 120;
    /// Ancestor dim duration (120 ms).
    pub const ANCESTOR_DIM_MS: u16 = 120;
    /// Caption crossfade duration (80 ms).
    pub const CAPTION_FADE_MS: u16 = 80;
    /// Commit flash phase duration (90 ms).
    pub const COMMIT_FLASH_MS: u16 = 90;
    /// Commit fade-out phase duration (120 ms).
    pub const COMMIT_FADE_MS: u16 = 120;
    /// Cancel dismiss fade-out duration (100 ms).
    pub const CANCEL_DISMISS_MS: u16 = 100;
    /// Reduced-motion opacity fade duration (60 ms).
    pub const REDUCED_MOTION_FADE_MS: u16 = 60;
}
