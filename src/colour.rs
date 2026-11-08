//! Decoded YUV 4:2:0 pictures (planar, NV12 or NV21, with any strides and crop) to packed RGBA.

use yuvutils_rs::{YuvPlanarImage, YuvRange, YuvStandardMatrix};

/// The matrix of a MediaFormat `color-standard`: BT.709 (1), BT.601 (2 PAL, 4 NTSC) or BT.2020
/// (6). Unknown or missing, it is guessed from the height as FFmpeg's converter does: HD is
/// BT.709, SD BT.601.
pub fn matrix(standard: Option<i32>, height: u32) -> YuvStandardMatrix {
    match standard {
        Some(1) => YuvStandardMatrix::Bt709,
        Some(2 | 4) => YuvStandardMatrix::Bt601,
        Some(6) => YuvStandardMatrix::Bt2020,
        _ if height >= 720 => YuvStandardMatrix::Bt709,
        _ => YuvStandardMatrix::Bt601,
    }
}

/// The range of a MediaFormat `color-range`: full (1), else limited.
pub fn range(range: Option<i32>) -> YuvRange {
    match range {
        Some(1) => YuvRange::Full,
        _ => YuvRange::Limited,
    }
}

/// One plane of a picture: sample `(row, col)` is at `row * row_stride + col * pixel_stride`.
#[derive(Clone, Copy, Debug)]
pub struct Plane<'a> {
    pub data: &'a [u8],
    pub row_stride: usize,
    pub pixel_stride: usize,
}

/// The `crop` (left, top, width, height) of a 4:2:0 picture as RGBA rows without padding, alpha
/// 255. `None` when the crop is empty or reaches outside a plane.
pub fn to_rgba(
    y: Plane<'_>,
    u: Plane<'_>,
    v: Plane<'_>,
    crop: (u32, u32, u32, u32),
    matrix: YuvStandardMatrix,
    range: YuvRange,
) -> Option<Vec<u8>> {
    let (left, top, width, height) = crop;
    let (l, t) = (usize::try_from(left).ok()?, usize::try_from(top).ok()?);
    let (w, h) = (usize::try_from(width).ok()?, usize::try_from(height).ok()?);
    if w == 0 || h == 0 {
        return None;
    }
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let y_plane = repack(y, l, t, w, h)?;
    let u_plane = repack(u, l / 2, t / 2, cw, ch)?;
    let v_plane = repack(v, l / 2, t / 2, cw, ch)?;
    let row = w.checked_mul(4)?;
    let mut rgba = vec![0; row.checked_mul(h)?];
    let chroma_stride = u32::try_from(cw).ok()?;
    let image = YuvPlanarImage {
        y_plane: &y_plane,
        y_stride: width,
        u_plane: &u_plane,
        u_stride: chroma_stride,
        v_plane: &v_plane,
        v_stride: chroma_stride,
        width,
        height,
    };
    yuvutils_rs::yuv420_to_rgba(&image, &mut rgba, u32::try_from(row).ok()?, range, matrix)
        .ok()?;
    for alpha in rgba.iter_mut().skip(3).step_by(4) {
        *alpha = 255;
    }
    Some(rgba)
}

/// The `w`×`h` samples of `plane` from (`left`, `top`) as a tight plane.
fn repack(plane: Plane<'_>, left: usize, top: usize, w: usize, h: usize) -> Option<Vec<u8>> {
    let step = plane.pixel_stride;
    if step == 0 {
        return None;
    }
    // The bytes from a row's first sample to its last, inclusive.
    let span = w.checked_sub(1)?.checked_mul(step)?.checked_add(1)?;
    let mut out = Vec::with_capacity(w.checked_mul(h)?);
    for r in 0..h {
        let start = top
            .checked_add(r)?
            .checked_mul(plane.row_stride)?
            .checked_add(left.checked_mul(step)?)?;
        let line = plane.data.get(start..start.checked_add(span)?)?;
        out.extend(line.iter().step_by(step));
    }
    Some(out)
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

    const Y: [u8; 8] = [16, 60, 120, 235, 80, 100, 140, 200];
    const U: [u8; 2] = [90, 160];
    const V: [u8; 2] = [200, 70];

    fn planar(data: &[u8], row_stride: usize) -> Plane<'_> {
        Plane {
            data,
            row_stride,
            pixel_stride: 1,
        }
    }

    #[test]
    fn nv12_matches_planar() {
        let (m, r) = (YuvStandardMatrix::Bt601, YuvRange::Limited);
        let from_planar = to_rgba(
            planar(&Y, 4),
            planar(&U, 2),
            planar(&V, 2),
            (0, 0, 4, 2),
            m,
            r,
        )
        .unwrap();
        let uv = [U[0], V[0], U[1], V[1]];
        let interleaved = |data| Plane {
            data,
            row_stride: 4,
            pixel_stride: 2,
        };
        let from_nv12 = to_rgba(
            planar(&Y, 4),
            interleaved(&uv),
            interleaved(&uv[1..]),
            (0, 0, 4, 2),
            m,
            r,
        )
        .unwrap();
        assert_eq!(from_planar.len(), 4 * 2 * 4);
        assert_eq!(from_planar, from_nv12);
        assert!(from_planar.iter().skip(3).step_by(4).all(|a| *a == 255));
    }

    #[test]
    fn crop_offsets_select_the_samples() {
        // A 2×2 crop at (2, 0) of the 4×2 picture equals a 2×2 picture of those samples.
        let (m, r) = (YuvStandardMatrix::Bt709, YuvRange::Limited);
        let cropped = to_rgba(
            planar(&Y, 4),
            planar(&U, 2),
            planar(&V, 2),
            (2, 0, 2, 2),
            m,
            r,
        )
        .unwrap();
        let y = [Y[2], Y[3], Y[6], Y[7]];
        let alone = to_rgba(
            planar(&y, 2),
            planar(&U[1..], 1),
            planar(&V[1..], 1),
            (0, 0, 2, 2),
            m,
            r,
        )
        .unwrap();
        assert_eq!(cropped, alone);
    }

    #[test]
    fn full_range_white() {
        let y = [255; 4];
        let c = [128];
        let rgba = to_rgba(
            planar(&y, 2),
            planar(&c, 1),
            planar(&c, 1),
            (0, 0, 2, 2),
            YuvStandardMatrix::Bt709,
            YuvRange::Full,
        )
        .unwrap();
        assert_eq!(rgba, [255; 16]);
    }

    #[test]
    fn short_plane_is_none() {
        assert!(
            to_rgba(
                planar(&Y[..6], 4),
                planar(&U, 2),
                planar(&V, 2),
                (0, 0, 4, 2),
                YuvStandardMatrix::Bt601,
                YuvRange::Limited,
            )
            .is_none()
        );
    }

    #[test]
    fn matrix_and_range() {
        assert_eq!(matrix(Some(1), 480), YuvStandardMatrix::Bt709);
        assert_eq!(matrix(Some(4), 1080), YuvStandardMatrix::Bt601);
        assert_eq!(matrix(Some(6), 480), YuvStandardMatrix::Bt2020);
        assert_eq!(matrix(None, 720), YuvStandardMatrix::Bt709);
        assert_eq!(matrix(Some(99), 719), YuvStandardMatrix::Bt601);
        assert_eq!(range(Some(1)), YuvRange::Full);
        assert_eq!(range(Some(2)), YuvRange::Limited);
        assert_eq!(range(None), YuvRange::Limited);
    }
}
