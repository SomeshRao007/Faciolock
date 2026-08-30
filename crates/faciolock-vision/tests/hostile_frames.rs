use faciolock_vision::{Burst, Config};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 16
    }

    fn upto(&mut self, n: usize) -> usize {
        (self.next() as usize) % n
    }
}

#[test]
fn no_buffer_and_no_geometry_panics_the_frame_path() {
    let cfg = Config::default();
    let mut rng = Rng(0xB105_5EED_0007_2026);
    let mut built = 0usize;

    let shapes = [0usize, 1, 2, 3, 7, 8, 16, 640, usize::MAX];
    let pixels: Vec<u8> = (0..4096).map(|_| rng.next() as u8).collect();

    for &w in &shapes {
        for &h in &shapes {
            for take in [0usize, 1, 2, 3, 63, 64, 65, 128, 4096] {
                let slice = &pixels[..take.min(pixels.len())];
                if let Ok(burst) = Burst::from_planar(slice, w, h, &cfg) {
                    built += 1;
                    assert_eq!(burst.width(), w);
                    assert_eq!(burst.height(), h);
                    for pair in burst.pairs() {
                        assert!(pair.on.mean().is_finite());
                        assert!(pair.off.mean().is_finite());
                    }
                    let _ = burst.observed();
                }
            }
        }
    }
    assert_eq!(built, 0, "noise is not expected to pair; if it now does, the loop above got weaker");

    let on = faciolock_vision::testing::domed(8, 8, 200, 40);
    let off = faciolock_vision::testing::blank(8, 8, 0);
    let clean = faciolock_vision::testing::strobe(&on, &off, 8);

    let intact = Burst::from_planar(&clean, 8, 8, &cfg).expect("a clean strobe must pair");
    assert!(!intact.pairs().is_empty(), "the generator must produce pairs to corrupt");

    let mut paired = 0usize;
    for _ in 0..5_000 {
        let mut bytes = clean.clone();
        for _ in 0..1 + rng.upto(4) {
            let at = rng.upto(bytes.len());
            bytes[at] ^= 1 << rng.upto(8);
        }
        let stride = 8 * 8;
        let cut = match rng.upto(4) {
            0 => bytes.len() - rng.upto(bytes.len()),
            _ => bytes.len() - stride * rng.upto(4),
        };
        if let Ok(burst) = Burst::from_planar(&bytes[..cut], 8, 8, &cfg) {
            paired += 1;
            for pair in burst.pairs() {
                assert!(pair.on.mean().is_finite());
                assert!(pair.off.mean().is_finite());
            }
            let _ = burst.observed();
        }
    }
    assert!(paired > 100, "only {paired} corrupted bursts paired; the statistics barely ran");
}
