//! Colour blotches (a Fotofficina addition): the low-frequency colour noise a raw denoiser leaves behind.
//!
//! A neural denoiser works on small tiles, so it removes grain but not the wide patches of colour noise at high ISO.
//! Those patches are low-frequency and live in the chroma, so they need a wide filter on the chroma alone; on the
//! luminance the same filter would erase detail. This pass splits camera RGB into luminance `Y = (R + G + B) / 3` and
//! two colour differences, smooths the colour differences with a guided filter (guide: the luminance, so colour does
//! not run across luminance edges) on a quarter-size copy, and puts the result back only where it differs from the
//! original by about the noise level: a real colour edge or a small coloured detail is left alone. The luminance is
//! never changed.

use lightcraft_raster::Rgb32f;
use rayon::prelude::*;

/// The smoothing runs on a copy this many times smaller.
const FACTOR: usize = 4;
/// Guided-filter radius on the small copy (so `RADIUS * FACTOR` pixels on the picture).
const RADIUS: usize = 8;
/// How far from the smoothed colour an original may be, in noise levels, and still be replaced.
const REACH: f32 = 3.0;
/// Pictures smaller than this are left as they are.
const MIN_SIDE: usize = 8 * FACTOR;

/// Smooth the colour blotches of `img` (camera RGB). Leaves the luminance and any colour detail as it is.
pub fn smooth(img: &mut Rgb32f) {
    let (w, h) = (img.width, img.height);
    if w < MIN_SIDE || h < MIN_SIDE || img.data.len() != w.saturating_mul(h) {
        return;
    }
    let (sw, sh) = (w / FACTOR, h / FACTOR);
    // luminance and colour differences, full size and quarter size
    let n = w * h;
    let mut cr = vec![0f32; n];
    let mut cb = vec![0f32; n];
    let mut y = vec![0f32; n];
    for (i, p) in img.data.iter().enumerate() {
        let l = (p[0] + p[1] + p[2]) / 3.0;
        if let (Some(a), Some(b), Some(c)) = (y.get_mut(i), cr.get_mut(i), cb.get_mut(i)) {
            *a = l;
            *b = p[0] - l;
            *c = p[2] - l;
        }
    }
    let (ys, crs, cbs) = (shrink(&y, w, h, sw, sh), shrink(&cr, w, h, sw, sh), shrink(&cb, w, h, sw, sh));
    let mean_y = ys.iter().sum::<f32>() / ys.len().max(1) as f32;
    // a luminance step of about 4 % of the mean separates two regions; smaller ones are the same surface
    let eps = (0.04 * mean_y).powi(2) + 1e-10;
    let (crf, cbf) = (guided(&ys, &crs, sw, sh, RADIUS, eps), guided(&ys, &cbs, sw, sh, RADIUS, eps));
    let (crs_full, cbs_full) = (grow(&crf, sw, sh, w, h), grow(&cbf, sw, sh, w, h));
    let (sig_r, sig_b) = (noise_level(&cr, &crs_full), noise_level(&cb, &cbs_full));
    img.data.par_chunks_mut(w).enumerate().for_each(|(row, line)| {
        for (x, p) in line.iter_mut().enumerate() {
            let i = row * w + x;
            let (Some(&l), Some(&r0), Some(&b0), Some(&rs), Some(&bs)) = (y.get(i), cr.get(i), cb.get(i), crs_full.get(i), cbs_full.get(i)) else {
                continue;
            };
            let blend = |c: f32, s: f32, sigma: f32| {
                let reach = (REACH * sigma).max(1e-9);
                let d = (c - s) / reach;
                let wgt = (-d * d).exp();
                s * wgt + c * (1.0 - wgt)
            };
            let r = l + blend(r0, rs, sig_r);
            let b = l + blend(b0, bs, sig_b);
            // green takes what keeps the luminance
            let g = 3.0 * l - r - b;
            *p = [r.max(0.0), g.max(0.0), b.max(0.0)];
        }
    });
}

/// Block means of `src` (`w × h`) on a `sw × sh` grid.
fn shrink(src: &[f32], w: usize, h: usize, sw: usize, sh: usize) -> Vec<f32> {
    let mut out = vec![0f32; sw * sh];
    out.par_chunks_mut(sw.max(1)).enumerate().for_each(|(sy, line)| {
        for (sx, v) in line.iter_mut().enumerate() {
            let (x0, y0) = (sx * FACTOR, sy * FACTOR);
            let (x1, y1) = ((x0 + FACTOR).min(w), (y0 + FACTOR).min(h));
            let mut sum = 0.0;
            let mut count = 0.0;
            for yy in y0..y1 {
                for xx in x0..x1 {
                    if let Some(s) = src.get(yy * w + xx) {
                        sum += *s;
                        count += 1.0;
                    }
                }
            }
            *v = if count > 0.0 { sum / count } else { 0.0 };
        }
    });
    out
}

/// Bilinear enlargement of `src` (`sw × sh`) to `w × h`.
fn grow(src: &[f32], sw: usize, sh: usize, w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0f32; w * h];
    let at = |x: usize, y: usize| src.get(y.min(sh - 1) * sw + x.min(sw - 1)).copied().unwrap_or(0.0);
    out.par_chunks_mut(w.max(1)).enumerate().for_each(|(y, line)| {
        // the centre of a small pixel is at (i + 0.5) * FACTOR
        let fy = ((y as f32 + 0.5) / FACTOR as f32 - 0.5).max(0.0);
        let (y0, ty) = (fy.floor() as usize, fy.fract());
        for (x, v) in line.iter_mut().enumerate() {
            let fx = ((x as f32 + 0.5) / FACTOR as f32 - 0.5).max(0.0);
            let (x0, tx) = (fx.floor() as usize, fx.fract());
            let top = at(x0, y0) * (1.0 - tx) + at(x0 + 1, y0) * tx;
            let bottom = at(x0, y0 + 1) * (1.0 - tx) + at(x0 + 1, y0 + 1) * tx;
            *v = top * (1.0 - ty) + bottom * ty;
        }
    });
    out
}

/// Mean over the `(2r + 1)²` window around each pixel (clamped at the border), by a running sum on rows then columns.
fn box_mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut rows = vec![0f32; w * h];
    for y in 0..h {
        let base = y * w;
        let mut sum = 0f32;
        let mut count = 0usize;
        // window [x - r, x + r]
        for x in 0..r.min(w) {
            sum += src.get(base + x).copied().unwrap_or(0.0);
            count += 1;
        }
        for x in 0..w {
            if x + r < w {
                sum += src.get(base + x + r).copied().unwrap_or(0.0);
                count += 1;
            }
            if x > r {
                sum -= src.get(base + x - r - 1).copied().unwrap_or(0.0);
                count -= 1;
            }
            if let Some(o) = rows.get_mut(base + x) {
                *o = sum / count.max(1) as f32;
            }
        }
    }
    let mut out = vec![0f32; w * h];
    for x in 0..w {
        let mut sum = 0f32;
        let mut count = 0usize;
        for y in 0..r.min(h) {
            sum += rows.get(y * w + x).copied().unwrap_or(0.0);
            count += 1;
        }
        for y in 0..h {
            if y + r < h {
                sum += rows.get((y + r) * w + x).copied().unwrap_or(0.0);
                count += 1;
            }
            if y > r {
                sum -= rows.get((y - r - 1) * w + x).copied().unwrap_or(0.0);
                count -= 1;
            }
            if let Some(o) = out.get_mut(y * w + x) {
                *o = sum / count.max(1) as f32;
            }
        }
    }
    out
}

/// Guided filter (He, Sun, Tang): `input` smoothed along the edges of `guide`; `eps` is the squared guide step that
/// counts as an edge.
fn guided(guide: &[f32], input: &[f32], w: usize, h: usize, r: usize, eps: f32) -> Vec<f32> {
    let mul = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).collect::<Vec<f32>>();
    let (mean_g, mean_i) = (box_mean(guide, w, h, r), box_mean(input, w, h, r));
    let (corr_gg, corr_gi) = (box_mean(&mul(guide, guide), w, h, r), box_mean(&mul(guide, input), w, h, r));
    let n = w * h;
    let mut a = vec![0f32; n];
    let mut b = vec![0f32; n];
    for i in 0..n {
        let (mg, mi) = (mean_g.get(i).copied().unwrap_or(0.0), mean_i.get(i).copied().unwrap_or(0.0));
        let var_g = corr_gg.get(i).copied().unwrap_or(0.0) - mg * mg;
        let cov = corr_gi.get(i).copied().unwrap_or(0.0) - mg * mi;
        let ai = cov / (var_g.max(0.0) + eps);
        if let (Some(x), Some(y)) = (a.get_mut(i), b.get_mut(i)) {
            *x = ai;
            *y = mi - ai * mg;
        }
    }
    let (mean_a, mean_b) = (box_mean(&a, w, h, r), box_mean(&b, w, h, r));
    (0..n).map(|i| mean_a.get(i).copied().unwrap_or(0.0) * guide.get(i).copied().unwrap_or(0.0) + mean_b.get(i).copied().unwrap_or(0.0)).collect()
}

/// The noise level of `c` around its smooth version `s`: 1.4826 × the median absolute difference, from a sample of the
/// picture (so a real colour detail, which is rare, does not count).
fn noise_level(c: &[f32], s: &[f32]) -> f32 {
    let step = (c.len() / 100_000).max(1);
    let mut d: Vec<f32> = c.iter().zip(s).step_by(step).map(|(a, b)| (a - b).abs()).filter(|v| v.is_finite()).collect();
    if d.is_empty() {
        return 0.0;
    }
    let mid = d.len() / 2;
    let (_, median, _) = d.select_nth_unstable_by(mid, f32::total_cmp);
    1.4826 * *median
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat grey picture with coloured noise in wide blotches: the blotches go, the luminance stays.
    #[test]
    fn wide_colour_noise_is_smoothed_and_the_luminance_is_kept() {
        let (w, h) = (128, 96);
        let mut img = Rgb32f::filled(w, h, [0.2, 0.2, 0.2]);
        for y in 0..h {
            for x in 0..w {
                // blotches about 24 pixels wide, in red and blue only
                let wave = (((x / 24) + (y / 24)) % 2) as f32 * 0.04 - 0.02;
                if let Some(p) = img.data.get_mut(y * w + x) {
                    p[0] += wave;
                    p[2] -= wave;
                }
            }
        }
        let before: Vec<f32> = img.data.iter().map(|p| (p[0] + p[1] + p[2]) / 3.0).collect();
        let spread = |im: &Rgb32f| {
            let d: Vec<f32> = im.data.iter().map(|p| p[0] - p[2]).collect();
            let mean = d.iter().sum::<f32>() / d.len() as f32;
            (d.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / d.len() as f32).sqrt()
        };
        let noisy = spread(&img);
        smooth(&mut img);
        assert!(spread(&img) < noisy * 0.7, "the colour blotches shrink: {noisy} -> {}", spread(&img));
        let after: Vec<f32> = img.data.iter().map(|p| (p[0] + p[1] + p[2]) / 3.0).collect();
        assert!(before.iter().zip(&after).all(|(a, b)| (a - b).abs() < 1e-4), "the luminance is not touched");
    }

    #[test]
    fn a_picture_too_small_or_empty_is_left_alone() {
        let mut tiny = Rgb32f::filled(4, 4, [0.1, 0.2, 0.3]);
        smooth(&mut tiny);
        assert_eq!(tiny.data[0], [0.1, 0.2, 0.3]);
        let mut empty = Rgb32f::new(0, 0);
        smooth(&mut empty);
    }
}
