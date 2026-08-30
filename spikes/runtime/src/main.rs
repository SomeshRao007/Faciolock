use std::time::{Duration, Instant};

const ITERS: usize = 50;
const WARMUP: usize = 5;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: {} <model.onnx> [width] [height]", args[0]);
        std::process::exit(2);
    }
    let path = args[1].clone();
    let w: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(640);
    let h: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(360);

    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!("model : {path}");
    println!("size  : {:.1} KB", size as f64 / 1024.0);
    println!("input : 1x3x{h}x{w} f32 (NCHW)\n");

    let input: Vec<f32> = (0..3 * h * w).map(|i| ((i % 255) as f32) / 255.0).collect();

    report("tract", bench_tract(&path, &input, w, h));
}

struct Bench {
    load: Duration,
    times: Vec<Duration>,
    note: String,
}

fn report(name: &str, r: Result<Bench, String>) {
    println!("-- {name} --------------------------------");
    match r {
        Err(e) => {
            println!("  LOAD FAILED");
            for line in e.lines().take(14) {
                println!("    {line}");
            }
        }
        Ok(b) => {
            let mut t: Vec<f64> = b.times.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
            t.sort_by(|a, x| a.partial_cmp(x).unwrap());
            let p = |q: f64| t[((t.len() - 1) as f64 * q).round() as usize];
            println!("  load    {:>8.1} ms", b.load.as_secs_f64() * 1000.0);
            println!("  p50     {:>8.2} ms", p(0.50));
            println!("  p95     {:>8.2} ms", p(0.95));
            println!("  p99     {:>8.2} ms", p(0.99));
            println!("  min/max {:>8.2} / {:.2} ms", t[0], t[t.len() - 1]);
            if !b.note.is_empty() {
                println!("  {}", b.note);
            }
        }
    }
    println!();
}

fn bench_tract(path: &str, input: &[f32], w: usize, h: usize) -> Result<Bench, String> {
    use tract_onnx::prelude::*;

    let t0 = Instant::now();
    let model = tract_onnx::onnx()
        .model_for_path(path)
        .map_err(|e| format!("{e:?}"))?
        .with_input_fact(0, f32::fact([1, 3, h, w]).into())
        .map_err(|e| format!("{e:?}"))?
        .into_optimized()
        .map_err(|e| format!("{e:?}"))?
        .into_runnable()
        .map_err(|e| format!("{e:?}"))?;
    let load = t0.elapsed();

    let tensor: Tensor = tract_ndarray::Array4::from_shape_vec((1, 3, h, w), input.to_vec())
        .map_err(|e| format!("{e:?}"))?
        .into();

    let outputs = model
        .run(tvec!(tensor.clone().into()))
        .map_err(|e| format!("{e:?}"))?;
    let note = format!("{} output(s)", outputs.len());

    for _ in 0..WARMUP {
        let _ = model.run(tvec!(tensor.clone().into()));
    }
    let mut times = Vec::with_capacity(ITERS);
    for _ in 0..ITERS {
        let t = Instant::now();
        model
            .run(tvec!(tensor.clone().into()))
            .map_err(|e| format!("{e:?}"))?;
        times.push(t.elapsed());
    }

    Ok(Bench { load, times, note })
}
