#![allow(dead_code)]

use std::path::{Path, PathBuf};

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

pub fn models_dir() -> PathBuf {
    repo_root().join("models")
}

pub fn corpus_dir() -> PathBuf {
    repo_root().join("corpus")
}

pub fn yunet() -> PathBuf {
    models_dir().join("face_detection_yunet_2026may.onnx")
}

pub fn sface() -> PathBuf {
    models_dir().join("face_recognition_sface_2021dec.onnx")
}

fn required() -> bool {
    std::env::var("FACIOLOCK_CORPUS_REQUIRED").is_ok_and(|v| v == "1")
}

pub fn skip(what: &str, path: &Path) -> bool {
    let msg = format!(
        "\n  ####  SKIPPED — DEGRADED RUN  ####\n  \
         missing {what}: {}\n  \
         This test asserted nothing. Set FACIOLOCK_CORPUS_REQUIRED=1 to make this fatal.\n",
        path.display()
    );
    if required() {
        panic!("{msg}");
    }
    eprintln!("{msg}");
    false
}

pub struct Capture {
    pub stem: String,
    pub label: String,
    pub bytes: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

impl Capture {
    pub fn has_live_face(&self) -> bool {
        self.label.starts_with("live_")
            // These recordings contain the enrolled person with an IR remote at the sensor.
            || matches!(self.label.as_str(), "attack_floodlight_daylight" | "attack_floodlight_room")
    }

    pub fn verification_bytes(&self) -> &[u8] {
        // The subject is still leaving at the start of this nominally empty recording.
        if self.stem == "null_daylight_room_20260806T060331Z" {
            return &self.bytes[30 * self.width * self.height..];
        }
        &self.bytes
    }

    pub fn frame(&self, i: usize) -> &[u8] {
        let stride = self.width * self.height;
        &self.bytes[i * stride..(i + 1) * stride]
    }

    pub fn frame_count(&self) -> usize {
        self.bytes.len() / (self.width * self.height)
    }
}

pub fn captures() -> Vec<Capture> {
    let dir = corpus_dir();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        skip("corpus directory", &dir);
        return out;
    };

    let mut sidecars: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    sidecars.sort();

    for sidecar in sidecars {
        let gray = sidecar.with_extension("gray");
        if !gray.exists() {
            skip("capture pixels", &gray);
            continue;
        }
        let Ok(meta) = std::fs::read_to_string(&sidecar) else { continue };
        let Some(width) = json_usize(&meta, "width") else { continue };
        let Some(height) = json_usize(&meta, "height") else { continue };
        let Ok(bytes) = std::fs::read(&gray) else { continue };
        let label = json_str(&meta, "label")
            .unwrap_or_else(|| gray.file_stem().unwrap().to_string_lossy().to_string());
        let stem = gray.file_stem().unwrap().to_string_lossy().to_string();
        out.push(Capture { stem, label, bytes, width, height });
    }
    out
}

fn json_usize(s: &str, key: &str) -> Option<usize> {
    let at = s.find(&format!("\"{key}\""))?;
    let rest = &s[at + key.len() + 2..];
    let colon = rest.find(':')?;
    rest[colon + 1..]
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}

fn json_str(s: &str, key: &str) -> Option<String> {
    let at = s.find(&format!("\"{key}\""))?;
    let rest = &s[at + key.len() + 2..];
    let colon = rest.find(':')?;
    let open = rest[colon..].find('"')? + colon + 1;
    let close = rest[open..].find('"')? + open;
    Some(rest[open..close].to_string())
}
