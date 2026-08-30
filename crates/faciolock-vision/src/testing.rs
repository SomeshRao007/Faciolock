pub const W: usize = 64;
pub const H: usize = 64;

pub fn blank(w: usize, h: usize, v: u8) -> Vec<u8> {
    vec![v; w * h]
}

pub fn flat(w: usize, h: usize, v: u8) -> Vec<u8> {
    blank(w, h, v)
}

pub fn domed(w: usize, h: usize, peak: u8, floor: u8) -> Vec<u8> {
    let mut f = vec![0u8; w * h];
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    let max_r = (cx * cx + cy * cy).sqrt();
    for y in 0..h {
        for x in 0..w {
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            let t = (dx * dx + dy * dy).sqrt() / max_r;
            let v = peak as f64 - t * (peak as f64 - floor as f64);
            f[y * w + x] = v.clamp(0.0, 255.0) as u8;
        }
    }
    f
}

pub fn planar(frames: &[Vec<u8>]) -> Vec<u8> {
    frames.concat()
}

pub fn strobe(on: &[u8], off: &[u8], pairs: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity((on.len() + off.len()) * pairs);
    for _ in 0..pairs {
        out.extend_from_slice(on);
        out.extend_from_slice(off);
    }
    out
}

pub fn strobe_one_contaminated(
    on: &[u8],
    off: &[u8],
    contaminated_off: &[u8],
    pairs: usize,
    bad_index: usize,
) -> Vec<u8> {
    let mut out = Vec::with_capacity((on.len() + off.len()) * pairs);
    for i in 0..pairs {
        out.extend_from_slice(on);
        out.extend_from_slice(if i == bad_index { contaminated_off } else { off });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domed_is_brighter_at_the_centre_than_at_the_corner() {
        let f = domed(W, H, 60, 10);
        let centre = f[(H / 2) * W + W / 2];
        assert!(centre > f[0], "centre {centre} vs corner {}", f[0]);
        assert!(centre > f[W - 1]);
    }

    #[test]
    fn strobe_alternates_and_has_the_right_length() {
        let on = blank(W, H, 60);
        let off = blank(W, H, 0);
        let burst = strobe(&on, &off, 5);
        assert_eq!(burst.len(), W * H * 10);
        assert_eq!(burst[0], 60);
        assert_eq!(burst[W * H], 0);
    }

    #[test]
    fn exactly_one_pair_is_contaminated() {
        let on = blank(W, H, 60);
        let off = blank(W, H, 0);
        let bad = blank(W, H, 30);
        let burst = strobe_one_contaminated(&on, &off, &bad, 4, 2);
        let offs: Vec<u8> = (0..4).map(|i| burst[(2 * i + 1) * W * H]).collect();
        assert_eq!(offs, vec![0, 0, 30, 0]);
    }
}
