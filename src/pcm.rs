//! Decoder PCM output to the stereo `f32` frames kira plays.

/// The sample format of a MediaCodec audio buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PcmEncoding {
    I16,
    F32,
}

/// Interleaved little-endian samples of `channels` channels as stereo frames: mono is
/// duplicated, and of more channels the first two are kept (kira plays stereo only). A trailing
/// partial frame is dropped; no channels give nothing.
pub fn to_stereo(data: &[u8], encoding: PcmEncoding, channels: usize) -> Vec<[f32; 2]> {
    let width = match encoding {
        PcmEncoding::I16 => 2,
        PcmEncoding::F32 => 4,
    };
    let Some(frame_bytes) = channels.checked_mul(width).filter(|n| *n > 0) else {
        return Vec::new();
    };
    let sample = |bytes: &[u8]| -> f32 {
        match encoding {
            PcmEncoding::I16 => match bytes {
                [a, b, ..] => f32::from(i16::from_le_bytes([*a, *b])) / 32768.0,
                _ => 0.0,
            },
            PcmEncoding::F32 => match bytes {
                [a, b, c, d, ..] => f32::from_le_bytes([*a, *b, *c, *d]),
                _ => 0.0,
            },
        }
    };
    data.chunks_exact(frame_bytes)
        .map(|frame| {
            let left = sample(frame);
            let right = if channels == 1 {
                left
            } else {
                sample(frame.get(width..).unwrap_or_default())
            };
            [left, right]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::arithmetic_side_effects
    )]

    use super::*;

    fn i16s(samples: &[i16]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    #[test]
    fn mono_is_duplicated() {
        let data = i16s(&[16384, -16384]);
        assert_eq!(
            to_stereo(&data, PcmEncoding::I16, 1),
            [[0.5, 0.5], [-0.5, -0.5]]
        );
    }

    #[test]
    fn six_channels_keep_the_first_two() {
        let data = i16s(&[8192, -8192, 1, 2, 3, 4, 16384, 0, 5, 6, 7, 8]);
        assert_eq!(
            to_stereo(&data, PcmEncoding::I16, 6),
            [[0.25, -0.25], [0.5, 0.0]]
        );
    }

    #[test]
    fn i16_extremes() {
        let data = i16s(&[i16::MIN, i16::MAX]);
        let frames = to_stereo(&data, PcmEncoding::I16, 2);
        assert_eq!(frames[0][0], -1.0);
        assert!((frames[0][1] - 32767.0 / 32768.0).abs() < f32::EPSILON);
    }

    #[test]
    fn f32_passes_through() {
        let data: Vec<u8> = [0.25f32, -0.75, 1.5, 0.0]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        assert_eq!(
            to_stereo(&data, PcmEncoding::F32, 2),
            [[0.25, -0.75], [1.5, 0.0]]
        );
    }

    #[test]
    fn no_channels_or_partial_frames() {
        assert!(to_stereo(&[1, 2, 3, 4], PcmEncoding::I16, 0).is_empty());
        // Three bytes of a 4-byte stereo frame.
        assert!(to_stereo(&[1, 2, 3], PcmEncoding::I16, 2).is_empty());
    }
}
