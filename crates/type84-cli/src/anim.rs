//! `type84 anim flash`: show a built-in effect for a few seconds, then put
//! the previous effect back.
//!
//! The board has no volatile (RAM-only) lighting channel (PROTOCOL.md §3.5a),
//! so every lighting change is a persistent SET_LED_EFFECT write. To keep
//! flash wear minimal one flash is exactly two single-frame 0x23 writes
//! (effect on, effect back), each skipped when it would change nothing.
//!
//! Robustness:
//! - One animation at a time: an exclusive, non-blocking `flock` on
//!   `<state dir>/anim.lock`. A second invocation while one is running is
//!   skipped (no writes) rather than queued.
//! - The effect to restore is saved to `<state dir>/anim-state.json` before
//!   the first write and removed after the restore. If a previous run was
//!   interrupted (power loss, kill -9), the next run reuses that saved
//!   effect as the one to restore, so a stale animation effect is never
//!   mistaken for the user's lighting. `type84 anim restore` writes it back.
//! - SIGINT, SIGTERM and SIGHUP cut the wait short and still restore.
//!
//! The state directory is `$XDG_STATE_HOME/type84` (`~/.local/state/type84`),
//! not `/run`, so the saved effect survives a reboot.

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use type84_proto::led;

pub const STATE_FORMAT: &str = "type84-anim-state";
pub const STATE_FORMAT_VERSION: u32 = 1;

/// The effect to restore after an animation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnimState {
    pub format: String,
    pub format_version: u32,
    pub created_at: String,
    /// Raw 16-byte GET_LED_EFFECT block, hex.
    pub effect: String,
    /// Human-readable description of `effect`.
    pub description: String,
}

impl AnimState {
    pub fn new(effect: &[u8]) -> Self {
        AnimState {
            format: STATE_FORMAT.into(),
            format_version: STATE_FORMAT_VERSION,
            created_at: chrono::Local::now().to_rfc3339(),
            effect: hex::encode(effect),
            description: led::describe_effect(effect),
        }
    }

    pub fn effect_bytes(&self) -> Result<Vec<u8>> {
        let b = hex::decode(&self.effect).context("anim state: effect is not hex")?;
        led::normalise_effect(&b).context("anim state: invalid effect block")?;
        Ok(b)
    }

    pub fn from_json(text: &str) -> Result<Self> {
        let s: AnimState = serde_json::from_str(text)?;
        if s.format != STATE_FORMAT || s.format_version != STATE_FORMAT_VERSION {
            bail!("not a type84 anim state file");
        }
        s.effect_bytes()?;
        Ok(s)
    }

    /// Load `path`; `Ok(None)` if it does not exist.
    pub fn load(path: &Path) -> Result<Option<Self>> {
        match std::fs::read_to_string(path) {
            Ok(t) => Ok(Some(
                Self::from_json(&t).with_context(|| format!("loading {}", path.display()))?,
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Write atomically (temp file + rename) and re-read.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        {
            let mut f =
                File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
            f.write_all(serde_json::to_string_pretty(self)?.as_bytes())?;
            f.write_all(b"\n")?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, path).with_context(|| format!("renaming to {}", path.display()))?;
        if Self::load(path)?.as_ref() != Some(self) {
            bail!("{} did not read back", path.display());
        }
        Ok(())
    }
}

/// `$XDG_STATE_HOME/type84`, falling back to the local data directory.
pub fn state_dir() -> Result<PathBuf> {
    let base = dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .context("cannot determine a state directory")?;
    Ok(base.join("type84"))
}

pub fn state_path(dir: &Path) -> PathBuf {
    dir.join("anim-state.json")
}

/// Exclusive lock held for the duration of an animation or a write.
#[derive(Debug)]
pub struct AnimLock {
    _file: File,
}

/// Take the animation lock without waiting. `Ok(None)` if another
/// animation holds it.
pub fn try_lock(dir: &Path) -> Result<Option<AnimLock>> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join("anim.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: flock on a file descriptor we own; no memory is shared.
        let r = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if r != 0 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
                return Ok(None);
            }
            return Err(e).with_context(|| format!("locking {}", path.display()));
        }
    }
    Ok(Some(AnimLock { _file: file }))
}

static STOP: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

/// Make SIGINT, SIGTERM and SIGHUP set a flag instead of killing the
/// process, so the restore write still happens.
pub fn install_stop_handler() {
    #[cfg(unix)]
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        // SAFETY: the handler only stores to an atomic, which is
        // async-signal-safe.
        unsafe {
            libc::signal(sig, on_signal as *const () as libc::sighandler_t);
        }
    }
}

pub fn stop_requested() -> bool {
    STOP.load(Ordering::SeqCst)
}

/// Sleep for `d`, returning early when a stop signal arrives.
pub fn sleep_interruptible(d: Duration) {
    let end = Instant::now() + d;
    while !stop_requested() {
        let now = Instant::now();
        if now >= end {
            break;
        }
        std::thread::sleep((end - now).min(Duration::from_millis(50)));
    }
}

/// Parse a duration in seconds (`2`, `1.5`), 0.1..=60 s.
pub fn parse_seconds(s: &str) -> Result<Duration, String> {
    let v: f64 = s
        .trim_end_matches('s')
        .parse()
        .map_err(|_| format!("{s:?} is not a number of seconds"))?;
    if !(0.1..=60.0).contains(&v) {
        return Err("duration must be 0.1..=60 seconds".into());
    }
    Ok(Duration::from_secs_f64(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    // flock is a no-op on Windows (no cross-process lock there), so lock semantics are unix-only.
    #[cfg(unix)]
    #[test]
    fn state_round_trip_and_lock() {
        let dir = std::env::temp_dir().join(format!("type84-anim-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = state_path(&dir);
        assert_eq!(AnimState::load(&path).unwrap(), None);
        let eff = hex::decode("0fffffff000000000105040000000000").unwrap();
        let s = AnimState::new(&eff);
        s.save(&path).unwrap();
        let back = AnimState::load(&path).unwrap().unwrap();
        assert_eq!(back.effect_bytes().unwrap(), eff);
        assert!(back.description.starts_with("circles"));
        let l = try_lock(&dir).unwrap();
        assert!(l.is_some());
        assert!(try_lock(&dir).unwrap().is_none(), "second lock must fail");
        drop(l);
        assert!(try_lock(&dir).unwrap().is_some());
        assert!(AnimState::from_json(r#"{"format":"x"}"#).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn seconds() {
        assert_eq!(parse_seconds("2").unwrap(), Duration::from_secs(2));
        assert_eq!(parse_seconds("1.5s").unwrap(), Duration::from_millis(1500));
        assert!(parse_seconds("0").is_err());
        assert!(parse_seconds("x").is_err());
        assert!(parse_seconds("61").is_err());
    }
}
