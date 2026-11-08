//! Which decoded frame is on screen at a playback position, seeking when the position jumps:
//! the frame selection of `kagari-video`'s `VideoDecoder::frame_at`, over any frame source.

use std::sync::Arc;

/// A decoded frame as sRGB RGBA, alpha 255, rows packed (`width * 4` bytes each).
#[derive(Debug, PartialEq, Eq)]
pub struct RgbaFrame {
    /// The frame's presentation time from the start of the movie.
    pub pts_ms: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Decoded frames in presentation order. A frame is held (a decoder buffer) until it is
/// discarded or converted.
pub trait Source {
    type Held;
    /// The next frame and its time (ms from the start, at least 0); `None` at the end.
    fn next(&mut self) -> Result<Option<(Self::Held, i64)>, String>;
    /// Moves to the keyframe at or before `target_ms` and drops what the decoder buffered.
    fn seek(&mut self, target_ms: i64) -> Result<(), String>;
    /// Gives a frame back unshown.
    fn discard(&mut self, held: Self::Held);
    /// The frame as RGBA.
    fn convert(&mut self, held: Self::Held, pts_ms: i64) -> Result<RgbaFrame, String>;
}

/// Seek forward instead of decoding when the target is this far past the next frame.
const SEEK_AHEAD_MS: i64 = 2000;

/// The frame selection state of one movie.
pub struct Picker<S: Source> {
    /// The newest frame at or before the last target; its buffer until converted.
    current: Option<(Option<S::Held>, i64)>,
    /// `current` was taken although it is after the target (nothing earlier exists).
    early: bool,
    /// The decoded frame after `current`.
    next: Option<(S::Held, i64)>,
    /// The frame last converted.
    last: Option<Arc<RgbaFrame>>,
}

impl<S: Source> Default for Picker<S> {
    fn default() -> Self {
        Picker {
            current: None,
            early: false,
            next: None,
            last: None,
        }
    }
}

impl<S: Source> Picker<S> {
    pub fn new() -> Self {
        Picker::default()
    }

    /// The frame shown at `target_ms`: the last frame at or before it, or the first frame when
    /// the target is before it. `None` when the stream has no frame at all. The same frame as
    /// the last call gives the same `Arc`.
    pub fn frame_at(
        &mut self,
        src: &mut S,
        target_ms: u64,
    ) -> Result<Option<Arc<RgbaFrame>>, String> {
        let target = i64::try_from(target_ms).unwrap_or(i64::MAX);
        let behind = self
            .current
            .as_ref()
            .is_some_and(|(_, pts)| target < *pts && !self.early);
        let ahead = match (&self.current, &self.next) {
            (_, Some((_, pts))) => target > pts.saturating_add(SEEK_AHEAD_MS),
            (None, None) => target > SEEK_AHEAD_MS,
            (Some(_), None) => false,
        };
        if behind || ahead {
            self.release(src);
            src.seek(target)?;
        }
        loop {
            if self.next.is_none() {
                self.next = src.next()?;
            }
            match &self.next {
                Some((_, pts)) if *pts <= target => {
                    if let Some((Some(held), _)) = self.current.take() {
                        src.discard(held);
                    }
                    self.current = self.next.take().map(|(held, pts)| (Some(held), pts));
                    self.early = false;
                }
                _ => break,
            }
        }
        if self.current.is_none() {
            self.current = self.next.take().map(|(held, pts)| (Some(held), pts));
            self.early = true;
        }
        let Some((held, pts)) = self.current.as_mut() else {
            return Ok(None);
        };
        let pts = *pts;
        let pts_ms = u64::try_from(pts).unwrap_or(0);
        if let Some(last) = self.last.as_ref().filter(|l| l.pts_ms == pts_ms) {
            if let Some(held) = held.take() {
                src.discard(held);
            }
            return Ok(Some(Arc::clone(last)));
        }
        let Some(held) = held.take() else {
            // Converted before; `last` has moved on only if a conversion failed.
            return Ok(self.last.clone());
        };
        let frame = Arc::new(src.convert(held, pts)?);
        self.last = Some(Arc::clone(&frame));
        Ok(Some(frame))
    }

    /// Gives back every held frame and forgets the position (before a seek or a decoder swap).
    pub fn release(&mut self, src: &mut S) {
        if let Some((Some(held), _)) = self.current.take() {
            src.discard(held);
        }
        if let Some((held, _)) = self.next.take() {
            src.discard(held);
        }
        self.early = false;
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::arithmetic_side_effects
    )]

    use std::collections::HashSet;

    use super::*;

    /// Frames every 40 ms from `first` to 400, a keyframe every 4th.
    struct Fake {
        pts: Vec<i64>,
        pos: usize,
        seeks: Vec<i64>,
        converted: usize,
        out: HashSet<usize>,
    }

    impl Fake {
        fn new(first: i64) -> Fake {
            Fake {
                pts: (first..=400).step_by(40).collect(),
                pos: 0,
                seeks: Vec::new(),
                converted: 0,
                out: HashSet::new(),
            }
        }
    }

    impl Source for Fake {
        type Held = usize;

        fn next(&mut self) -> Result<Option<(usize, i64)>, String> {
            let Some(pts) = self.pts.get(self.pos).copied() else {
                return Ok(None);
            };
            assert!(self.out.insert(self.pos), "frame handed out twice");
            self.pos += 1;
            Ok(Some((self.pos - 1, pts)))
        }

        fn seek(&mut self, target_ms: i64) -> Result<(), String> {
            assert!(self.out.is_empty(), "seek with frames held");
            self.seeks.push(target_ms);
            let at = self.pts.iter().rposition(|p| *p <= target_ms).unwrap_or(0);
            self.pos = at - at % 4;
            Ok(())
        }

        fn discard(&mut self, held: usize) {
            assert!(self.out.remove(&held));
        }

        fn convert(&mut self, held: usize, pts_ms: i64) -> Result<RgbaFrame, String> {
            assert!(self.out.remove(&held));
            assert_eq!(self.pts[held], pts_ms);
            self.converted += 1;
            Ok(RgbaFrame {
                pts_ms: pts_ms as u64,
                width: 1,
                height: 1,
                rgba: vec![0, 0, 0, 255],
            })
        }
    }

    fn pts(frame: &Option<Arc<RgbaFrame>>) -> u64 {
        frame.as_ref().unwrap().pts_ms
    }

    #[test]
    fn sequential_targets() {
        let (mut src, mut p) = (Fake::new(0), Picker::new());
        assert_eq!(pts(&p.frame_at(&mut src, 0).unwrap()), 0);
        assert_eq!(pts(&p.frame_at(&mut src, 50).unwrap()), 40);
        assert_eq!(pts(&p.frame_at(&mut src, 119).unwrap()), 80);
        assert_eq!(pts(&p.frame_at(&mut src, 120).unwrap()), 120);
        assert!(src.seeks.is_empty());
        assert_eq!(src.converted, 4);
        // At most `current`'s buffer is not converted and `next` is held.
        assert!(src.out.len() <= 1);
    }

    #[test]
    fn same_frame_is_the_same_arc() {
        let (mut src, mut p) = (Fake::new(0), Picker::new());
        let a = p.frame_at(&mut src, 40).unwrap().unwrap();
        let b = p.frame_at(&mut src, 79).unwrap().unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(src.converted, 1);
    }

    #[test]
    fn target_before_the_first_frame() {
        let (mut src, mut p) = (Fake::new(40), Picker::new());
        let a = p.frame_at(&mut src, 10).unwrap().unwrap();
        assert_eq!(a.pts_ms, 40);
        // Still early: no seek back, the same frame.
        let b = p.frame_at(&mut src, 20).unwrap().unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert!(src.seeks.is_empty());
        assert_eq!(pts(&p.frame_at(&mut src, 85).unwrap()), 80);
    }

    #[test]
    fn backward_jump_seeks() {
        let (mut src, mut p) = (Fake::new(0), Picker::new());
        assert_eq!(pts(&p.frame_at(&mut src, 300).unwrap()), 280);
        assert_eq!(pts(&p.frame_at(&mut src, 100).unwrap()), 80);
        assert_eq!(src.seeks, [100]);
    }

    #[test]
    fn far_jump_seeks() {
        let mut src = Fake::new(0);
        src.pts = (0..=4000).step_by(40).collect();
        let mut p = Picker::new();
        assert_eq!(pts(&p.frame_at(&mut src, 0).unwrap()), 0);
        // Within 2 s of the next frame: decoded through.
        assert_eq!(pts(&p.frame_at(&mut src, 2040).unwrap()), 2040);
        assert!(src.seeks.is_empty());
        assert_eq!(pts(&p.frame_at(&mut src, 4090).unwrap()), 4000);
        assert_eq!(src.seeks, [4090]);
    }

    #[test]
    fn past_the_end_holds_the_last_frame() {
        let (mut src, mut p) = (Fake::new(0), Picker::new());
        let a = p.frame_at(&mut src, 1000).unwrap().unwrap();
        assert_eq!(a.pts_ms, 400);
        let b = p.frame_at(&mut src, 1500).unwrap().unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert!(src.seeks.is_empty());
    }

    #[test]
    fn empty_stream_has_no_frame() {
        let mut src = Fake::new(0);
        src.pts.clear();
        let mut p = Picker::new();
        assert!(p.frame_at(&mut src, 0).unwrap().is_none());
        assert!(p.frame_at(&mut src, 5000).unwrap().is_none());
    }
}
