#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 4 {
        return;
    }
    let width = u16::from_le_bytes([data[0], data[1]]) as usize % 700;
    let height = u16::from_le_bytes([data[2], data[3]]) as usize % 700;
    let pixels = &data[4..];

    let cfg = faciolock_vision::Config::default();
    if let Ok(burst) = faciolock_vision::Burst::from_planar(pixels, width, height, &cfg) {
        assert_eq!(burst.width(), width);
        assert_eq!(burst.height(), height);
        for pair in burst.pairs() {
            let _ = pair.on.mean();
            let _ = pair.off.mean();
        }
        let _ = burst.observed();
    }
});
