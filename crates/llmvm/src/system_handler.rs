//! Real implementations of `fs.read`, `fs.write`, `time.now` and `random`, used under
//! `--live`. Every other capability goes to the wrapped fallback handler.
//!
//! The gateway checks the policy rule and budget before a call reaches this handler. This
//! handler adds the path check from [`FsPolicy`]. Each result lands in the VM event log,
//! which is what `boruna replay` serves back, so a live clock read or random draw replays
//! to the same value.

use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use boruna_bytecode::{Capability, Value};
use rand_core::{OsRng, RngCore};

use crate::capability_gateway::{CapabilityHandler, FsPolicy};

pub struct SystemHandler {
    fs: Option<FsPolicy>,
    fallback: Box<dyn CapabilityHandler>,
}

impl SystemHandler {
    pub fn new(fs: Option<FsPolicy>, fallback: Box<dyn CapabilityHandler>) -> Self {
        SystemHandler { fs, fallback }
    }

    /// The policy and its roots, resolved. Errors when file access is not configured.
    fn roots(&self, cap: &str) -> Result<(&FsPolicy, Vec<PathBuf>), String> {
        let policy = self
            .fs
            .as_ref()
            .filter(|p| !p.allowed_roots.is_empty())
            .ok_or_else(|| {
                format!(
                    "{cap} denied: the policy has no fs_policy.allowed_roots; \
                 add the folders this program may use"
                )
            })?;
        let mut roots = Vec::new();
        for r in &policy.allowed_roots {
            // A root that does not exist cannot contain anything; skip it.
            if let Ok(c) = std::fs::canonicalize(r) {
                roots.push(c);
            }
        }
        Ok((policy, roots))
    }

    fn check_inside(
        cap: &str,
        resolved: &Path,
        roots: &[PathBuf],
        shown: &str,
    ) -> Result<(), String> {
        if roots.iter().any(|r| resolved.starts_with(r)) {
            Ok(())
        } else {
            Err(format!(
                "{cap} denied: '{shown}' is outside fs_policy.allowed_roots"
            ))
        }
    }

    fn fs_read(&self, args: &[Value]) -> Result<Value, String> {
        let path = string_arg(args, 0, "fs_read", "path")?;
        let (policy, roots) = self.roots("fs.read")?;
        let resolved = std::fs::canonicalize(path)
            .map_err(|e| format!("fs_read: cannot open '{path}': {e}"))?;
        Self::check_inside("fs.read", &resolved, &roots, path)?;
        let meta = std::fs::metadata(&resolved)
            .map_err(|e| format!("fs_read: cannot open '{path}': {e}"))?;
        if !meta.is_file() {
            return Err(format!("fs_read: '{path}' is not a file"));
        }
        if meta.len() > policy.max_read_bytes as u64 {
            return Err(format!(
                "fs_read: '{path}' is {} bytes, over fs_policy.max_read_bytes ({})",
                meta.len(),
                policy.max_read_bytes
            ));
        }
        let bytes =
            std::fs::read(&resolved).map_err(|e| format!("fs_read: cannot read '{path}': {e}"))?;
        String::from_utf8(bytes)
            .map(Value::String)
            .map_err(|_| format!("fs_read: '{path}' is not UTF-8 text"))
    }

    fn fs_write(&self, args: &[Value]) -> Result<Value, String> {
        let path = string_arg(args, 0, "fs_write", "path")?;
        let content = string_arg(args, 1, "fs_write", "content")?;
        let (_, roots) = self.roots("fs.write")?;
        let p = Path::new(path);
        let name = match p.components().next_back() {
            Some(Component::Normal(n)) => n.to_owned(),
            _ => return Err(format!("fs_write: '{path}' does not end in a file name")),
        };
        let parent = match p.parent() {
            Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let parent = std::fs::canonicalize(&parent)
            .map_err(|e| format!("fs_write: folder of '{path}' does not exist: {e}"))?;
        let mut target = parent.join(name);
        // An existing symlink is followed; check where it really points.
        if target.exists() {
            target = std::fs::canonicalize(&target)
                .map_err(|e| format!("fs_write: cannot resolve '{path}': {e}"))?;
        }
        Self::check_inside("fs.write", &target, &roots, path)?;
        std::fs::write(&target, content)
            .map_err(|e| format!("fs_write: cannot write '{path}': {e}"))?;
        Ok(Value::Bool(true))
    }
}

fn string_arg<'a>(args: &'a [Value], i: usize, func: &str, what: &str) -> Result<&'a str, String> {
    match args.get(i) {
        Some(Value::String(s)) => Ok(s),
        Some(other) => Err(format!("{func}: {what} must be a String, got {other}")),
        None => Err(format!("{func}: missing {what}")),
    }
}

fn now_ms() -> Result<Value, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| Value::Int(d.as_millis() as i64))
        .map_err(|e| format!("time_now: system clock is before 1970: {e}"))
}

fn os_u64() -> Result<u64, String> {
    let mut buf = [0u8; 8];
    OsRng
        .try_fill_bytes(&mut buf)
        .map_err(|e| format!("random: OS entropy unavailable: {e}"))?;
    Ok(u64::from_le_bytes(buf))
}

/// Uniform Int in `[lo, hi]` without modulo bias (rejection sampling).
fn random_int(lo: i64, hi: i64) -> Result<Value, String> {
    if lo > hi {
        return Err(format!("random_int: lo ({lo}) is greater than hi ({hi})"));
    }
    let span = (hi as i128 - lo as i128 + 1) as u128;
    if span > u64::MAX as u128 {
        // The whole i64 range: every u64 maps to exactly one value.
        return Ok(Value::Int(os_u64()? as i64));
    }
    let span = span as u64;
    let zone = u64::MAX - (u64::MAX % span);
    loop {
        let x = os_u64()?;
        if x < zone {
            return Ok(Value::Int((lo as i128 + (x % span) as i128) as i64));
        }
    }
}

impl CapabilityHandler for SystemHandler {
    fn handle(&mut self, cap: &Capability, args: &[Value]) -> Result<Value, String> {
        match cap {
            Capability::FsRead => self.fs_read(args),
            Capability::FsWrite => self.fs_write(args),
            Capability::TimeNow => now_ms(),
            Capability::Random => match args {
                [Value::Int(lo), Value::Int(hi)] => random_int(*lo, *hi),
                [] => Ok(Value::Float((os_u64()? >> 11) as f64 / (1u64 << 53) as f64)),
                _ => Err("random_int: expects two Int arguments (lo, hi)".to_string()),
            },
            other => self.fallback.handle(other, args),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability_gateway::MockHandler;

    fn handler(roots: &[&Path]) -> SystemHandler {
        SystemHandler::new(
            Some(FsPolicy {
                allowed_roots: roots.iter().map(|p| p.display().to_string()).collect(),
                max_read_bytes: 64,
            }),
            Box::new(MockHandler),
        )
    }

    fn s(v: &str) -> Value {
        Value::String(v.to_string())
    }

    fn p(path: &Path) -> Value {
        s(path.to_str().unwrap())
    }

    #[test]
    fn write_then_read_inside_root() {
        let root = tempfile::tempdir().unwrap();
        let mut h = handler(&[root.path()]);
        let f = root.path().join("a.txt");
        assert_eq!(
            h.handle(&Capability::FsWrite, &[p(&f), s("hello")])
                .unwrap(),
            Value::Bool(true)
        );
        assert_eq!(h.handle(&Capability::FsRead, &[p(&f)]).unwrap(), s("hello"));
    }

    #[test]
    fn read_outside_root_is_denied() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let f = other.path().join("secret.txt");
        std::fs::write(&f, "x").unwrap();
        let mut h = handler(&[root.path()]);
        let err = h.handle(&Capability::FsRead, &[p(&f)]).unwrap_err();
        assert!(err.contains("outside fs_policy.allowed_roots"), "{err}");
    }

    #[test]
    fn dotdot_escape_is_denied() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(parent.path().join("secret.txt"), "x").unwrap();
        let mut h = handler(&[&root]);
        let escape = root.join("..").join("secret.txt");
        let err = h.handle(&Capability::FsRead, &[p(&escape)]).unwrap_err();
        assert!(err.contains("outside"), "{err}");
        let err = h
            .handle(&Capability::FsWrite, &[p(&escape), s("y")])
            .unwrap_err();
        assert!(err.contains("outside"), "{err}");
        assert_eq!(
            std::fs::read_to_string(parent.path().join("secret.txt")).unwrap(),
            "x"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_out_of_root_is_denied() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let secret = other.path().join("secret.txt");
        std::fs::write(&secret, "x").unwrap();
        let link = root.path().join("link.txt");
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        let mut h = handler(&[root.path()]);
        assert!(h.handle(&Capability::FsRead, &[p(&link)]).is_err());
        assert!(h.handle(&Capability::FsWrite, &[p(&link), s("y")]).is_err());
        assert_eq!(std::fs::read_to_string(&secret).unwrap(), "x");
    }

    #[test]
    fn no_fs_policy_denies_everything() {
        let root = tempfile::tempdir().unwrap();
        let f = root.path().join("a.txt");
        std::fs::write(&f, "x").unwrap();
        let mut h = SystemHandler::new(None, Box::new(MockHandler));
        let err = h.handle(&Capability::FsRead, &[p(&f)]).unwrap_err();
        assert!(err.contains("fs_policy.allowed_roots"), "{err}");
        assert!(h.handle(&Capability::FsWrite, &[p(&f), s("y")]).is_err());
    }

    #[test]
    fn oversized_read_is_denied() {
        let root = tempfile::tempdir().unwrap();
        let f = root.path().join("big.txt");
        std::fs::write(&f, "x".repeat(65)).unwrap();
        let mut h = handler(&[root.path()]);
        let err = h.handle(&Capability::FsRead, &[p(&f)]).unwrap_err();
        assert!(err.contains("max_read_bytes"), "{err}");
    }

    #[test]
    fn write_to_missing_folder_fails() {
        let root = tempfile::tempdir().unwrap();
        let mut h = handler(&[root.path()]);
        let f = root.path().join("nope").join("a.txt");
        assert!(h.handle(&Capability::FsWrite, &[p(&f), s("y")]).is_err());
    }

    #[test]
    fn time_now_is_current_ms() {
        let mut h = SystemHandler::new(None, Box::new(MockHandler));
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let Value::Int(t) = h.handle(&Capability::TimeNow, &[]).unwrap() else {
            panic!("not an Int")
        };
        assert!((t - before).abs() < 1000, "{t} vs {before}");
    }

    #[test]
    fn random_int_stays_in_range() {
        let mut h = SystemHandler::new(None, Box::new(MockHandler));
        assert_eq!(
            h.handle(&Capability::Random, &[Value::Int(5), Value::Int(5)])
                .unwrap(),
            Value::Int(5)
        );
        for _ in 0..1000 {
            let Value::Int(x) = h
                .handle(&Capability::Random, &[Value::Int(1), Value::Int(6)])
                .unwrap()
            else {
                panic!("not an Int")
            };
            assert!((1..=6).contains(&x));
        }
        assert!(h
            .handle(
                &Capability::Random,
                &[Value::Int(i64::MIN), Value::Int(i64::MAX)]
            )
            .is_ok());
        assert!(h
            .handle(&Capability::Random, &[Value::Int(2), Value::Int(1)])
            .is_err());
        let Value::Float(f) = h.handle(&Capability::Random, &[]).unwrap() else {
            panic!("not a Float")
        };
        assert!((0.0..1.0).contains(&f));
    }

    #[test]
    fn other_capabilities_reach_the_fallback() {
        let mut h = SystemHandler::new(None, Box::new(MockHandler));
        let out = h
            .handle(&Capability::NetFetch, &[s("https://example.com")])
            .unwrap();
        assert!(matches!(out, Value::String(ref v) if v.contains("\"mock\": true")));
    }
}
