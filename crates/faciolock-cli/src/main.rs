//! `faciolockctl` — enrol, inspect, measure.
//!
//! ## What it is allowed to be
//!
//! The fourth thing that touches the socket, and the least trusted of them. It runs as root
//! because the socket is 0600 root-owned and the daemon's `SO_PEERCRED` gate refuses anything
//! else (wiki/03 R2) — but it *grants* nothing. Every op it can send is one the PAM module
//! can already send, and the one write it performs (`remove`) is an unlink that root could do
//! with `rm`.
//!
//! It does not acquire root for you. `pkexec` and `sudo` exist, `data/polkit/` carries the
//! policy for the first, and re-execing yourself through a setuid helper with argv you were
//! handed is a surface this program has no reason to have. If you are not root it prints the
//! command to run.
//!
//! ## The two things it must not hide
//!
//! - **A bright room is not a broken camera** ([B18]). An honest daylight capture answers
//!   `Unavailable`, and the message says infrared and blinds rather than "error".
//! - **A printed face passes liveness** ([B24]). Enrolment uses verification's gate, which is
//!   the strongest thing available, and that is not the same as saying it is strong.
//!
//! [B18]: ../../../wiki/11-bug-log.md
//! [B24]: ../../../wiki/11-bug-log.md

mod client;
mod passwd;
mod probe;
mod store_dir;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use faciolock_ipc::{ENROL_POSES, Op, Verdict};

use client::{Answer, DEADLINE};

const DEFAULT_CONFIG: &str = "/etc/faciolock/faciolockd.toml";

const USAGE: &str = "\
faciolockctl — manage Faciolock enrolments

  enroll [USER]        guided enrolment, three poses
  list                 who is enrolled
  remove USER [--yes]  delete an enrolment
  test [USER]          one authentication, exactly as PAM would ask for it
  bench [-n N] [USER]  N authentications; p50/p95/p99
  diagnose [--write]   what this machine has, and the config for it

  --config PATH        the daemon's config (default /etc/faciolock/faciolockd.toml)

USER defaults to $SUDO_USER, so `sudo faciolockctl enroll` does the obvious thing.
Everything but `diagnose` needs root: the socket is 0600 and the store is 0700.";

/// The subset of the daemon's config this program needs.
///
/// Deliberately not `deny_unknown_fields`: the daemon owns that file and validates it
/// strictly at startup, and a CLI that refused to list enrolments because the daemon grew a
/// key would be reporting somebody else's problem as its own.
///
/// **The defaults must match `faciolock-daemon`'s `Config::default`.** They are spelled twice
/// because the daemon is a binary and its `Config` is not importable; if that ever becomes
/// worth fixing, the fix is a shared crate, not a guess.
#[derive(serde::Deserialize)]
#[serde(default)]
struct Settings {
    socket: PathBuf,
    templates: PathBuf,
    /// `diagnose` opens the camera at this geometry, so hardware that is not 640×360 gets
    /// probed at whatever the operator configured rather than at the reference machine's
    /// shape. The daemon re-verifies geometry on open, so a wrong value here is a clear
    /// `WrongGeometry` rather than a silent mis-probe.
    width: u32,
    height: u32,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            socket: PathBuf::from("/run/faciolock.sock"),
            templates: PathBuf::from("/var/lib/faciolock/templates"),
            width: 640,
            height: 360,
        }
    }
}

impl Settings {
    /// A missing file is a fresh install and the defaults are right. A file that exists and
    /// does not parse is an operator error — the same reading the daemon takes.
    fn load(path: &Path) -> Result<Settings, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("faciolockctl: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let mut config = PathBuf::from(DEFAULT_CONFIG);
    let mut rest: Vec<&str> = Vec::new();

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => match it.next() {
                Some(v) => config = PathBuf::from(v),
                None => return Err("--config needs a path".into()),
            },
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => rest.push(other),
        }
    }

    let settings = Settings::load(&config)?;
    let (command, rest) = rest.split_first().map_or(("", &[][..]), |(c, r)| (*c, r));

    match command {
        "" => {
            println!("{USAGE}");
            Ok(())
        }
        "diagnose" => diagnose(&config, rest.contains(&"--write")),
        "enroll" | "enrol" => needs_root(command).and_then(|()| enroll(&settings, subject(rest)?)),
        "list" => needs_root(command).and_then(|()| list(&settings)),
        "remove" => needs_root(command)
            .and_then(|()| remove(&settings, subject(rest)?, rest.contains(&"--yes"))),
        "test" => needs_root(command).and_then(|()| test(&settings, subject(rest)?)),
        "bench" => needs_root(command).and_then(|()| bench(&settings, rest)),
        other => Err(format!("unknown command {other:?}\n\n{USAGE}")),
    }
}

/// The user a command is about: the argument if given, otherwise whoever invoked `sudo`.
///
/// Falling back to `$SUDO_USER` rather than to the *effective* user matters — the effective
/// user under `sudo` is root, and silently enrolling root's face because someone typed
/// `sudo faciolockctl enroll` is not a mistake this should make quietly.
fn subject(rest: &[&str]) -> Result<String, String> {
    if let Some(name) = rest.iter().find(|a| !a.starts_with('-')) {
        return Ok((*name).to_string());
    }
    match std::env::var("SUDO_USER") {
        Ok(name) if !name.is_empty() => Ok(name),
        _ => Err("no username given, and $SUDO_USER is not set — pass one explicitly".into()),
    }
}

fn needs_root(command: &str) -> Result<(), String> {
    match unsafe { libc::geteuid() } {
        0 => Ok(()),
        // Named rather than acquired: see the module header.
        _ => Err(format!(
            "`{command}` needs root — the socket is 0600 and the store is 0700.\n\
             Try:  sudo faciolockctl {command} …\n\
             or:   pkexec faciolockctl {command} …"
        )),
    }
}

/// Ask, and turn "the daemon is not there" into one message rather than six.
fn ask(settings: &Settings, op: Op, user: &str) -> Result<Answer, String> {
    client::ask(&settings.socket, op, user, DEADLINE)
}

// ---------------------------------------------------------------------------- enrol

fn enroll(settings: &Settings, user: String) -> Result<(), String> {
    // Fail before the camera is lit rather than after three poses.
    if passwd::uid_of(&user).is_none() {
        return Err(format!("no such user: {user}"));
    }

    println!("Enrolling {user}. {ENROL_POSES} poses, a second or so each.");
    println!("Sit where you normally sit. Faciolock will match what it sees now.\n");

    let mut pose = 0;
    let mut denials = 0;
    let mut unjudged = 0;
    while pose < ENROL_POSES {
        prompt(POSES[pose]);
        let answer = ask(settings, Op::Enroll, &user)?;
        match answer.verdict {
            Verdict::Ack => {
                pose += 1;
                denials = 0;
                unjudged = 0;
                println!("  captured  ({pose}/{ENROL_POSES}, {} ms)\n", answer.daemon_ms);
            }
            // The pipeline looked and refused. **Retried indefinitely, on purpose**: this is
            // a positioning problem, the person is standing right there, and pressing Enter
            // again after moving is exactly the fix. The loop blocks on stdin between tries,
            // so it waits rather than spins.
            Verdict::Deny => {
                unjudged = 0;
                denials += 1;
                println!("  {}\n", enrol_advice(Verdict::Deny));
                if denials % GIVE_UP == 0 {
                    println!("  {denials} in a row. {}\n", stuck_advice(Verdict::Deny));
                }
            }
            // The pipeline could not judge it — a room with too much infrared in it, or a
            // camera somebody else is holding. **Not retried indefinitely**, because unlike a
            // pose neither of those is fixed by pressing Enter again, and a CLI that loops
            // forever on them leaves the person with nothing but Ctrl-C.
            other => {
                denials = 0;
                unjudged += 1;
                println!("  {}\n", enrol_advice(other));
                if unjudged >= GIVE_UP {
                    return Err(format!(
                        "gave up after {unjudged} captures the daemon could not judge.\n\n{}",
                        stuck_advice(other)
                    ));
                }
            }
        }
    }

    // **Ask the daemon rather than trusting the count.** Three `Ack`s mean three poses were
    // captured; they do not by themselves mean a template was written. A store write that
    // failed on the last pose, or a daemon that restarted mid-enrolment, both end here with
    // the CLI having counted to three — and "Enrolled" would be a lie the user discovers at
    // a lock screen. `Status` costs nothing, touches no camera, and settles it.
    match ask(settings, Op::Status, &user)?.verdict {
        Verdict::NoEnrolment => Err(format!(
            "captured {ENROL_POSES} poses but the daemon has no template for {user}.\n\
             Either a pose took more than a minute to arrive, which starts the enrolment\n\
             over, or the store could not be written. The daemon's log says which."
        )),
        _ => {
            println!("Enrolled {user}. Check it with:  sudo faciolockctl test {user}");
            Ok(())
        }
    }
}

/// Consecutive failures before the tone changes — advice for a pose the pipeline refused,
/// and the end of the road for one it could not judge.
const GIVE_UP: usize = 3;

const POSES: [&str; ENROL_POSES] = [
    "Look straight at the camera.",
    "Turn your head slightly to the left.",
    "If you wear glasses, take them off. Otherwise just look straight again.",
];

/// Wait for the person to be ready. A pose taken while they are still reaching for the key is
/// a pose of a blurred shoulder, and it would enrol as one.
fn prompt(instruction: &str) {
    print!("{instruction}  [Enter] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    // EOF is fine: piped input means a script, and a script does not need to be waited for.
    let _ = std::io::stdin().read_line(&mut line);
}

/// One line the person can act on, per verdict.
fn enrol_advice(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Deny => "no usable face — sit square to the camera, about arm's length away",
        // B18. The daemon reached the ambient gate, which is a room problem and not a camera
        // one, and saying "error" here would send someone to check their hardware.
        Verdict::Unavailable => {
            "couldn't judge that one — too much infrared in the room, or the camera is busy"
        }
        Verdict::RateLimited => "the daemon is refusing requests for this user right now",
        Verdict::NoEnrolment | Verdict::Allow | Verdict::Ack => "unexpected answer from the daemon",
    }
}

fn stuck_advice(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Unavailable => {
            "This is usually daylight: the infrared floor is above what the sensor can work \
             against. Close the blinds or try after dark. If the room is already dark, check \
             nothing else is using the camera."
        }
        _ => "Check the camera sees you at all:  sudo faciolockctl diagnose",
    }
}

// ---------------------------------------------------------------------------- list / remove

fn list(settings: &Settings) -> Result<(), String> {
    let found = store_dir::list(&settings.templates)?;
    if found.is_empty() {
        println!("Nobody is enrolled.  ({})", settings.templates.display());
        return Ok(());
    }

    println!("{:>8}  {:<20}  {:>7}  enrolled", "uid", "user", "bytes");
    for e in &found {
        println!(
            "{:>8}  {:<20}  {:>7}  {}",
            e.uid,
            // A template whose user no longer exists is the thing worth spotting here.
            e.name.as_deref().unwrap_or("(no such user)"),
            e.bytes,
            e.modified.map_or("unknown".into(), age),
        );
    }
    Ok(())
}

/// "3 days ago", from a mtime. No date formatting crate for one column.
fn age(t: std::time::SystemTime) -> String {
    let Ok(elapsed) = t.elapsed() else { return "just now".into() };
    let secs = elapsed.as_secs();
    match secs {
        0..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} days ago", secs / 86_400),
    }
}

fn remove(settings: &Settings, user: String, yes: bool) -> Result<(), String> {
    let Some(uid) = passwd::uid_of(&user) else {
        return Err(format!("no such user: {user}"));
    };

    if !yes {
        print!("Delete {user}'s enrolment (uid {uid})? [y/N] ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        if !matches!(line.trim(), "y" | "Y" | "yes") {
            println!("Left alone.");
            return Ok(());
        }
    }

    if !store_dir::remove(&settings.templates, uid)? {
        println!("{user} was not enrolled.");
        return Ok(());
    }

    // The template is gone; a lockout earned by it must not outlive it. Best-effort: an
    // absent daemon is not a reason to report the removal as failed, because it succeeded.
    match ask(settings, Op::Clear, &user) {
        Ok(_) => println!("Removed {user}'s enrolment and cleared their failure budget."),
        Err(e) => println!("Removed {user}'s enrolment. Could not reach the daemon to clear their failure budget ({e})."),
    }
    Ok(())
}

// ---------------------------------------------------------------------------- test / bench

fn test(settings: &Settings, user: String) -> Result<(), String> {
    println!("Authenticating {user} — look at the camera.");
    let answer = ask(settings, Op::Auth, &user)?;
    println!(
        "\n  {}\n  {} ms in the daemon, {} ms round trip",
        describe(answer.verdict),
        answer.daemon_ms,
        answer.round_trip.as_millis()
    );

    // This went through the real path, budget included, so a failure just cost the user part
    // of theirs. Clearing is something this program can already do as root; the alternative
    // was a no-count path inside the daemon, and after B24 the budget is the defence rather
    // than a backstop behind liveness.
    clear_after(settings, &user);
    Ok(())
}

fn bench(settings: &Settings, rest: &[&str]) -> Result<(), String> {
    let (runs, positional) = bench_args(rest)?;
    let user = subject(&positional)?;

    println!("{runs} authentications for {user}. Hold still and look at the camera.");
    println!("The failure budget is cleared between runs — otherwise the lockout fires at 5");
    println!("failures and every later sample measures a refusal rather than the pipeline.\n");

    let mut samples: Vec<u128> = Vec::with_capacity(runs);
    let mut verdicts: Vec<Verdict> = Vec::with_capacity(runs);
    for i in 1..=runs {
        let answer = ask(settings, Op::Auth, &user)?;
        samples.push(answer.round_trip.as_millis());
        verdicts.push(answer.verdict);
        clear_after(settings, &user);
        print!("\r  {i}/{runs}");
        let _ = std::io::stdout().flush();
    }
    println!("\r              \r");

    let allowed = verdicts.iter().filter(|v| v.is_allow()).count();
    println!("  {allowed}/{runs} allowed");
    // Not a mean, per wiki/06: an authentication that occasionally takes 900 ms is a bad lock
    // screen no matter what the average says.
    samples.sort_unstable();
    for (label, p) in [("p50", 0.50), ("p95", 0.95), ("p99", 0.99)] {
        println!("  {label}  {} ms", percentile(&samples, p));
    }
    println!("  min  {} ms\n  max  {} ms", samples[0], samples[samples.len() - 1]);
    if runs < 100 {
        println!("\n  n={runs}: p99 here is the slowest sample, not an estimate of one.");
    }
    Ok(())
}

/// Split `-n N` from the positional arguments.
///
/// `-n`'s value has to be *consumed*, not filtered out by shape: `bench csr -n 20` and
/// `bench -n 20 csr` must mean the same thing, and a filter that only drops words starting
/// with `-` leaves `20` looking like a username in the first form.
fn bench_args<'a>(rest: &[&'a str]) -> Result<(usize, Vec<&'a str>), String> {
    let mut runs = 20usize;
    let mut positional = Vec::new();
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match *arg {
            "-n" => match it.next().and_then(|v| v.parse().ok()) {
                Some(n) if n > 0 => runs = n,
                _ => return Err("-n needs a positive count".into()),
            },
            other if other.starts_with('-') => return Err(format!("unknown option {other:?}")),
            other => positional.push(other),
        }
    }
    Ok((runs, positional))
}

/// Nearest-rank, which needs no interpolation and cannot invent a number that was never
/// measured. `samples` must be sorted and non-empty.
fn percentile(samples: &[u128], p: f64) -> u128 {
    let rank = (p * samples.len() as f64).ceil() as usize;
    samples[rank.saturating_sub(1).min(samples.len() - 1)]
}

/// Give back what the run just spent. Best-effort and quiet: the measurement already
/// happened, and a failure to clear is a thing to say once rather than to abort over.
fn clear_after(settings: &Settings, user: &str) {
    if let Err(e) = ask(settings, Op::Clear, user) {
        eprintln!("faciolockctl: could not clear the failure budget: {e}");
    }
}

fn describe(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Allow => "Allow — face unlock would have succeeded",
        Verdict::Deny => "Deny — a face was looked at and refused",
        Verdict::Unavailable => "Unavailable — the question could not be asked (bright room, or camera busy)",
        Verdict::NoEnrolment => "NoEnrolment — nothing enrolled for this user",
        Verdict::RateLimited => "RateLimited — too many recent failures",
        Verdict::Ack => "Ack",
    }
}

// ---------------------------------------------------------------------------- diagnose

fn diagnose(config: &Path, write: bool) -> Result<(), String> {
    let settings = Settings::load(config)?;

    println!("config      {}", config.display());
    println!("socket      {}", settings.socket.display());
    match client::ask(&settings.socket, Op::Status, "root", Duration::from_secs(2)) {
        Ok(_) => println!("daemon      answering"),
        Err(e) => println!("daemon      {e}"),
    }

    println!("\nvideo nodes");
    for node in probe::survey() {
        match node {
            faciolock_camera::enumerate::Node::Capture(c) => println!(
                "  {}  {}  grey={} max={}px",
                c.path.display(),
                c.card,
                c.greyscale,
                c.max_pixels
            ),
            faciolock_camera::enumerate::Node::NotCapture { path } => {
                println!("  {}  (not a capture device — metadata node)", path.display())
            }
            faciolock_camera::enumerate::Node::Inaccessible { path, reason } => println!(
                "  {}  could not open: {reason} — a permissions problem, not a missing camera",
                path.display()
            ),
        }
    }

    let selected = match probe::discover() {
        Ok(s) => s,
        Err(e) => {
            println!("\nno infrared camera: {e}");
            return Ok(());
        }
    };
    println!("\nselected    {}  ({})", selected.node.display(), selected.card);
    match &selected.by_path {
        Some(p) => println!("stable name {}", p.display()),
        None => println!("stable name none — this machine has no by-path entry for that node"),
    }

    println!("\nillumination ({PROBE_NOTE})");
    let geometry = faciolock_camera::Geometry::new(settings.width, settings.height);
    let strobe = match probe::observe(&selected.node, geometry) {
        Ok(s) => s,
        Err(e) => {
            println!("  could not probe: {e}");
            println!("  If faciolockd is running it holds the camera — stop it and re-run.");
            return Ok(());
        }
    };
    println!("  frame means  {}", sample_of(&strobe.means));
    println!("  alternation  {:.2}  (1.00 is a clean ON/OFF pattern)", strobe.alternation);
    println!("  bright/dark  {:.1}:1", strobe.ratio);

    if !strobe.is_strobing() {
        println!("\n  No strobe observed, and no tier will be written.");
        println!("  Most often this is daylight rather than hardware: ambient infrared lifts");
        println!("  the dark frames until the pattern flattens. Re-run after dark. Writing a");
        println!("  weaker tier because a room was bright is exactly what P1-e forbids.");
        return Ok(());
    }

    println!("  tier         ir-strobe");
    let text = probe::config_text(&probe::stable_name(&selected), "ir-strobe");
    if !write {
        println!("\nconfig for this machine — `diagnose --write` writes it:\n");
        println!("{text}");
        return Ok(());
    }
    needs_root("diagnose --write")?;
    write_and_report(config, &text)
}

const PROBE_NOTE: &str = "30 frames, read-only — no control is set and no emitter is touched";

fn write_and_report(config: &Path, text: &str) -> Result<(), String> {
    match probe::write_config(config, text) {
        Ok(()) => {
            println!("\nwrote {}", config.display());
            Ok(())
        }
        Err(e) => {
            println!("\n{e}");
            println!("Add these lines yourself if they are not already there:\n\n{text}");
            Ok(())
        }
    }
}

/// First eight means, so the reader sees the pattern without forty numbers of it.
fn sample_of(means: &[f64]) -> String {
    let head: Vec<String> = means.iter().take(8).map(|m| format!("{m:.3}")).collect();
    match means.len() > 8 {
        true => format!("{}  … ({} frames)", head.join("  "), means.len()),
        false => head.join("  "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `sudo faciolockctl enroll` with no name must enrol the person who typed it, not root.
    /// Getting this backwards writes a template for uid 0 and looks like it worked.
    #[test]
    fn the_subject_defaults_to_the_invoking_user_not_root() {
        unsafe { std::env::set_var("SUDO_USER", "csr") };
        assert_eq!(subject(&[]).expect("falls back"), "csr");
        assert_eq!(subject(&["--yes"]).expect("flags are not names"), "csr");
        assert_eq!(subject(&["someone"]).expect("explicit wins"), "someone");

        unsafe { std::env::remove_var("SUDO_USER") };
        assert!(subject(&[]).is_err(), "no name and no SUDO_USER must not silently pick one");
    }

    /// wiki/06 asks for p50/p95/p99 and specifically not a mean. Nearest-rank so every number
    /// printed is a sample that was actually measured.
    #[test]
    fn percentiles_are_measured_samples_not_interpolations() {
        let s: Vec<u128> = (1..=100).collect();
        assert_eq!(percentile(&s, 0.50), 50);
        assert_eq!(percentile(&s, 0.95), 95);
        assert_eq!(percentile(&s, 0.99), 99);

        // Small n, which is what `bench` defaults to. p99 is the max and must not index past
        // the end getting there.
        let small: Vec<u128> = vec![10, 20, 30];
        assert_eq!(percentile(&small, 0.99), 30);
        assert_eq!(percentile(&small, 0.50), 20);
        assert_eq!(percentile(&[7], 0.99), 7);
    }

    /// The CLI's defaults are the daemon's defaults, spelled twice. If they drift, `list`
    /// reports on a directory nobody writes to and says nobody is enrolled.
    #[test]
    fn the_defaults_are_the_paths_wiki_04_specifies() {
        let s = Settings::default();
        assert_eq!(s.socket, Path::new("/run/faciolock.sock"));
        assert_eq!(s.templates, Path::new("/var/lib/faciolock/templates"));

        // And a config that sets them wins.
        let dir = std::env::temp_dir().join(format!("faciolockctl-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tmpdir");
        let path = dir.join("faciolockd.toml");
        std::fs::write(&path, "socket = \"/tmp/x.sock\"\nburst_frames = 6\n").expect("write");
        let loaded = Settings::load(&path).expect("a key this crate does not know must not fail");
        assert_eq!(loaded.socket, Path::new("/tmp/x.sock"));
        assert_eq!(loaded.templates, Path::new("/var/lib/faciolock/templates"));
        assert_eq!((loaded.width, loaded.height), (640, 360));

        assert_eq!(
            Settings::load(Path::new("/nonexistent/faciolockd.toml")).expect("defaults").socket,
            Path::new("/run/faciolock.sock")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every verdict has advice, and the daylight one names the room rather than the hardware
    /// — B18's whole lesson is that this failure is not a fault.
    #[test]
    fn a_bright_room_is_described_as_a_room() {
        let advice = enrol_advice(Verdict::Unavailable);
        assert!(advice.contains("infrared"), "{advice}");
        assert!(stuck_advice(Verdict::Unavailable).contains("daylight"));

        for v in [Verdict::Deny, Verdict::Unavailable, Verdict::RateLimited, Verdict::NoEnrolment] {
            assert!(!enrol_advice(v).is_empty());
            assert!(!describe(v).is_empty());
        }
    }

    /// One prompt per pose, or the loop indexes past the end of the array.
    #[test]
    fn there_is_a_prompt_for_every_pose() {
        assert_eq!(POSES.len(), ENROL_POSES);
    }

    /// `-n`'s value must be consumed rather than filtered by shape, or `bench csr -n 20`
    /// benchmarks a user called `20`.
    #[test]
    fn the_count_never_gets_mistaken_for_a_username() {
        assert_eq!(bench_args(&["-n", "50", "csr"]).expect("parses"), (50, vec!["csr"]));
        assert_eq!(bench_args(&["csr", "-n", "50"]).expect("parses"), (50, vec!["csr"]));
        assert_eq!(bench_args(&[]).expect("parses"), (20, Vec::new()));

        for bad in [vec!["-n"], vec!["-n", "0"], vec!["-n", "lots"], vec!["--wat"]] {
            assert!(bench_args(&bad).is_err(), "{bad:?} must not parse");
        }
    }
}
