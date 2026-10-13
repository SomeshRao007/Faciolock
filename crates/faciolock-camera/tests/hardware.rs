use std::time::{Duration, Instant};

use faciolock_camera::{Capture, Geometry, discover, open, survey};
use faciolock_camera::enumerate::Node;
use faciolock_vision::frame::{Frame, Observed, classify};

const REFERENCE: Geometry = Geometry::new(640, 360);

#[test]
#[ignore = "needs the IR camera"]
fn the_survey_separates_cameras_from_metadata_nodes() {
    let nodes = survey();
    assert!(!nodes.is_empty(), "no /dev/video* nodes at all");

    let mut capture = 0;
    let mut metadata = 0;
    for node in &nodes {
        match node {
            Node::Capture(c) => {
                capture += 1;
                println!(
                    "  capture     {:<14} grey={:<5} max={:>8}px  {}",
                    c.path.display(),
                    c.greyscale,
                    c.max_pixels,
                    c.card
                );
            }
            Node::NotCapture { path } => {
                metadata += 1;
                println!("  not-capture {}", path.display());
            }
            Node::Inaccessible { path, reason } => {
                println!("  denied      {} ({reason})", path.display());
            }
        }
    }

    println!("\n{capture} capture device(s), {metadata} metadata node(s)");
    assert!(capture > 0, "no capture devices");
}

#[test]
#[ignore = "needs the IR camera"]
fn discovery_finds_the_ir_camera_and_a_stable_name_for_it() {
    let found = discover().expect("an IR camera");
    println!("node    {}", found.node.display());
    println!("card    {}", found.card);
    match &found.by_path {
        Some(p) => println!("by-path {}", p.display()),
        None => println!("by-path (none — config would have to store {})", found.node.display()),
    }

    let by_path = found.by_path.expect("a by-path name to persist");
    let name = by_path.to_string_lossy();

    assert!(!name.contains("by-id"), "by-id must never be persisted: {name}");
    assert!(name.contains("by-path"), "expected a by-path symlink, got {name}");

    let via_link = std::fs::canonicalize(&by_path).expect("by-path resolves");
    let via_node = std::fs::canonicalize(&found.node).expect("node resolves");
    assert_eq!(via_link, via_node);
}

#[test]
#[ignore = "needs the IR camera"]
fn a_burst_off_the_sensor_is_contiguous_and_still_strobes() {
    let found = discover().expect("an IR camera");
    let path = found.by_path.unwrap_or(found.node);
    let dev = open(&path, REFERENCE).expect("GREY at 640x360");

    let mut cap = Capture::start(dev, REFERENCE, 5, Instant::now() + Duration::from_secs(3))
        .expect("stream starts");
    let burst = cap.burst(30, Duration::from_secs(3)).expect("30 frames");

    assert_eq!(burst.width, 640);
    assert_eq!(burst.height, 360);
    assert_eq!(burst.planar.len(), 30 * REFERENCE.stride());
    assert_eq!(burst.meta.len(), 30);

    println!("discarded {}  dropped {}", burst.discarded, burst.dropped);
    println!("sensor span {:?} over 30 frames", burst.sensor_span());

    let mut gaps: Vec<f64> = burst
        .meta
        .windows(2)
        .map(|w| (w[1].timestamp - w[0].timestamp).as_secs_f64() * 1000.0)
        .collect();
    gaps.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    let p50 = gaps[gaps.len() / 2];
    println!(
        "frame interval  min {:.1} ms  p50 {:.1} ms  max {:.1} ms  =>  {:.1} fps delivered",
        gaps[0],
        p50,
        gaps[gaps.len() - 1],
        1000.0 / p50
    );

    let seqs: Vec<u32> = burst.meta.iter().map(|m| m.sequence).collect();
    for w in seqs.windows(2) {
        assert!(w[1] > w[0], "sequence went backwards or repeated: {:?}", w);
    }
    assert_eq!(burst.dropped, 0, "sensor produced frames we never saw: {seqs:?}");

    for w in burst.meta.windows(2) {
        assert!(
            w[1].timestamp > w[0].timestamp,
            "timestamps did not advance: {:?} then {:?}",
            w[0].timestamp,
            w[1].timestamp
        );
    }

    let frames: Vec<Frame<'_>> = burst
        .planar
        .chunks_exact(REFERENCE.stride())
        .filter_map(|c| Frame::new(c, burst.width, burst.height))
        .collect();
    let means: Vec<f64> = frames.iter().map(|f| f.mean()).collect();
    let (_, observed) = classify(&means);

    for (i, m) in means.iter().enumerate() {
        print!("{:>3} {:>10.5}  ", i, m);
        if i % 4 == 3 {
            println!();
        }
    }
    println!();

    assert_eq!(observed, Observed::Strobing, "means: {means:?}");
}

#[test]
#[ignore = "needs the IR camera"]
fn a_burst_contains_no_frame_older_than_the_request() {
    let found = discover().expect("an IR camera");
    let path = found.by_path.unwrap_or(found.node);
    let dev = open(&path, REFERENCE).expect("GREY at 640x360");
    let mut cap = Capture::start(dev, REFERENCE, 5, Instant::now() + Duration::from_secs(3))
        .expect("stream starts");

    std::thread::sleep(Duration::from_millis(500));

    let asked_at = monotonic_now();
    let burst = cap.burst(6, Duration::from_secs(2)).expect("6 frames");
    let oldest = burst.meta[0].timestamp;

    println!("requested at {asked_at:?}, oldest frame stamped {oldest:?}");
    assert!(
        oldest >= asked_at,
        "burst contained a frame exposed {:?} before the request",
        asked_at.saturating_sub(oldest)
    );
}

#[test]
#[ignore = "needs the IR camera"]
fn burst_cost_from_a_warm_session() {
    const TRIALS: usize = 50;
    const FRAMES: usize = 6;

    let found = discover().expect("an IR camera");
    let path = found.by_path.unwrap_or(found.node);
    let dev = open(&path, REFERENCE).expect("GREY at 640x360");
    let mut cap = Capture::start(dev, REFERENCE, 5, Instant::now() + Duration::from_secs(3))
        .expect("stream starts");

    for _ in 0..10 {
        let _ = cap.burst(FRAMES, Duration::from_secs(2));
    }

    let mut wall = Vec::with_capacity(TRIALS);
    let mut sensor = Vec::with_capacity(TRIALS);
    let mut discarded = 0usize;
    let mut dropped = 0u32;

    for _ in 0..TRIALS {
        let t0 = std::time::Instant::now();
        let burst = cap.burst(FRAMES, Duration::from_secs(2)).expect("burst");
        wall.push(t0.elapsed().as_secs_f64() * 1000.0);
        sensor.push(burst.sensor_span().as_secs_f64() * 1000.0);
        discarded += burst.discarded;
        dropped += burst.dropped;
    }

    wall.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    sensor.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    let pct = |v: &[f64], p: f64| v[((v.len() - 1) as f64 * p).round() as usize];

    println!("\n=== burst cost, {FRAMES} frames, {TRIALS} trials, warm session ===");
    println!(
        "wall clock (request -> burst ready)   p50 {:.1} ms   p95 {:.1} ms   p99 {:.1} ms",
        pct(&wall, 0.50),
        pct(&wall, 0.95),
        pct(&wall, 0.99)
    );
    println!(
        "sensor span (first -> last exposure)  p50 {:.1} ms   p95 {:.1} ms",
        pct(&sensor, 0.50),
        pct(&sensor, 0.95)
    );
    println!("discarded frames {discarded}, dropped {dropped}");
    println!("wiki/06 budgets 130 ms for this stage.\n");

    assert_eq!(dropped, 0, "sensor produced frames the burst never saw");
}

fn monotonic_now() -> Duration {
    let raw = std::fs::read_to_string("/proc/uptime").expect("/proc/uptime");
    let secs: f64 = raw.split_whitespace().next().expect("uptime field").parse().expect("a number");
    Duration::from_secs_f64(secs)
}
