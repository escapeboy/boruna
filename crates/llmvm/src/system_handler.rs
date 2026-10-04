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
        // Read at most one byte past the limit, so a file that grew after the size check is
        // refused instead of read whole.
        let mut bytes = Vec::new();
        std::fs::File::open(&resolved)
            .and_then(|f| {
                use std::io::Read;
                f.take(policy.max_read_bytes as u64 + 1)
                    .read_to_end(&mut bytes)
            })
            .map_err(|e| format!("fs_read: cannot read '{path}': {e}"))?;
        if bytes.len() > policy.max_read_bytes {
            return Err(format!(
                "fs_read: '{path}' is over fs_policy.max_read_bytes ({})",
                policy.max_read_bytes
            ));
        }
        String::from_utf8(bytes)
            .map(Value::String)
            .map_err(|_| format!("fs_read: '{path}' is not UTF-8 text"))
    }

    /// The folder of `path`, resolved, plus its final file name. The folder must exist; the file
    /// need not.
    fn split_target(func: &str, path: &str) -> Result<(PathBuf, std::ffi::OsString), String> {
        let p = Path::new(path);
        let name = match p.components().next_back() {
            Some(Component::Normal(n)) => n.to_owned(),
            _ => return Err(format!("{func}: '{path}' does not end in a file name")),
        };
        let parent = match p.parent() {
            Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let parent = std::fs::canonicalize(&parent)
            .map_err(|e| format!("{func}: folder of '{path}' does not exist: {e}"))?;
        Ok((parent, name))
    }

    /// Where a write to `path` really lands, checked against the roots.
    fn write_target(&self, func: &str, path: &str) -> Result<PathBuf, String> {
        let (_, roots) = self.roots("fs.write")?;
        let (parent, name) = Self::split_target(func, path)?;
        let mut target = parent.join(name);
        // The write follows a symlink, so check where it really points. A link to a missing
        // file cannot be resolved and is refused: writing through it would create the file
        // wherever the link points, possibly outside the roots.
        let is_link = std::fs::symlink_metadata(&target)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if is_link {
            target = std::fs::canonicalize(&target)
                .map_err(|_| format!("{func} denied: '{path}' is a symlink to a missing file"))?;
        }
        Self::check_inside("fs.write", &target, &roots, path)?;
        Ok(target)
    }

    fn fs_write(&self, args: &[Value]) -> Result<Value, String> {
        let path = string_arg(args, 0, "fs_write", "path")?;
        let content = string_arg(args, 1, "fs_write", "content")?;
        let target = self.write_target("fs_write", path)?;
        std::fs::write(&target, content)
            .map_err(|e| format!("fs_write: cannot write '{path}': {e}"))?;
        Ok(Value::Bool(true))
    }

    fn fs_append(&self, args: &[Value]) -> Result<Value, String> {
        let path = string_arg(args, 0, "fs_append", "path")?;
        let content = string_arg(args, 1, "fs_append", "content")?;
        let target = self.write_target("fs_append", path)?;
        use std::io::Write;
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&target)
            .and_then(|mut f| f.write_all(content.as_bytes()))
            .map_err(|e| format!("fs_append: cannot write '{path}': {e}"))?;
        Ok(Value::Bool(true))
    }

    /// Delete a file. A symlink is removed itself, never its target, so only the folder
    /// holding it has to be inside the roots.
    fn fs_delete(&self, args: &[Value]) -> Result<Value, String> {
        let path = string_arg(args, 0, "fs_delete", "path")?;
        let (policy, roots) = self.roots("fs.write")?;
        if !policy.allow_delete {
            return Err(format!(
                "fs_delete denied: the policy does not set fs_policy.allow_delete (deleting '{path}')"
            ));
        }
        let (parent, name) = Self::split_target("fs_delete", path)?;
        let target = parent.join(name);
        Self::check_inside("fs.write", &target, &roots, path)?;
        let meta = std::fs::symlink_metadata(&target)
            .map_err(|e| format!("fs_delete: cannot find '{path}': {e}"))?;
        if meta.is_dir() {
            return Err(format!(
                "fs_delete: '{path}' is a folder; only files are deleted"
            ));
        }
        std::fs::remove_file(&target)
            .map_err(|e| format!("fs_delete: cannot delete '{path}': {e}"))?;
        Ok(Value::Bool(true))
    }

    /// Names of the entries in a folder, sorted, not recursive.
    fn fs_list(&self, args: &[Value]) -> Result<Value, String> {
        let path = string_arg(args, 0, "fs_list", "path")?;
        let (policy, roots) = self.roots("fs.read")?;
        let resolved = std::fs::canonicalize(path)
            .map_err(|e| format!("fs_list: cannot open '{path}': {e}"))?;
        Self::check_inside("fs.read", &resolved, &roots, path)?;
        let mut names = Vec::new();
        for entry in std::fs::read_dir(&resolved)
            .map_err(|e| format!("fs_list: cannot list '{path}': {e}"))?
        {
            let entry = entry.map_err(|e| format!("fs_list: cannot list '{path}': {e}"))?;
            if names.len() == policy.max_list_entries {
                return Err(format!(
                    "fs_list: '{path}' has more than fs_policy.max_list_entries ({}) entries",
                    policy.max_list_entries
                ));
            }
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        Ok(Value::List(names.into_iter().map(Value::String).collect()))
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
            // The compiler appends an operation name for the calls that share a capability:
            // `fs_list(dir)` is `[dir, "list"]`, `fs_append(p, s)` is `[p, s, "append"]` and
            // `fs_delete(p)` is `[p, "", "delete"]`. The plain forms have fewer arguments.
            Capability::FsRead => match args {
                [_] => self.fs_read(args),
                [_, Value::String(op)] if op == "list" => self.fs_list(args),
                _ => Err("fs.read: unknown operation".to_string()),
            },
            Capability::FsWrite => match args {
                [_, _] => self.fs_write(args),
                [_, _, Value::String(op)] if op == "append" => self.fs_append(args),
                [_, _, Value::String(op)] if op == "delete" => self.fs_delete(args),
                _ => Err("fs.write: unknown operation".to_string()),
            },
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
                allow_delete: true,
                max_list_entries: 3,
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

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_cannot_create_a_file_outside_the_root() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let missing = other.path().join("pwned.txt");
        let link = root.path().join("link.txt");
        std::os::unix::fs::symlink(&missing, &link).unwrap();
        let mut h = handler(&[root.path()]);
        let err = h
            .handle(&Capability::FsWrite, &[p(&link), s("escaped")])
            .unwrap_err();
        assert!(err.contains("symlink to a missing file"), "{err}");
        assert!(!missing.exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_a_file_inside_the_root_is_written() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real.txt");
        std::fs::write(&real, "old").unwrap();
        let link = root.path().join("link.txt");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let mut h = handler(&[root.path()]);
        h.handle(&Capability::FsWrite, &[p(&link), s("new")])
            .unwrap();
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "new");
    }

    #[test]
    fn list_append_and_delete_inside_the_root() {
        let root = tempfile::tempdir().unwrap();
        let mut h = handler(&[root.path()]);
        let f = root.path().join("log.txt");
        std::fs::create_dir(root.path().join("sub")).unwrap();
        h.handle(&Capability::FsWrite, &[p(&f), s("a"), s("append")])
            .unwrap();
        h.handle(&Capability::FsWrite, &[p(&f), s("b"), s("append")])
            .unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "ab");
        assert_eq!(
            h.handle(&Capability::FsRead, &[p(root.path()), s("list")])
                .unwrap(),
            Value::List(vec![s("log.txt"), s("sub")])
        );
        h.handle(&Capability::FsWrite, &[p(&f), s(""), s("delete")])
            .unwrap();
        assert!(!f.exists());
        // Folders are not deleted.
        let err = h
            .handle(
                &Capability::FsWrite,
                &[p(&root.path().join("sub")), s(""), s("delete")],
            )
            .unwrap_err();
        assert!(err.contains("is a folder"), "{err}");
    }

    #[test]
    fn list_append_and_delete_outside_the_root_are_denied() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let victim = other.path().join("keep.txt");
        std::fs::write(&victim, "x").unwrap();
        let mut h = handler(&[root.path()]);
        assert!(h
            .handle(&Capability::FsRead, &[p(other.path()), s("list")])
            .is_err());
        assert!(h
            .handle(&Capability::FsWrite, &[p(&victim), s("y"), s("append")])
            .is_err());
        let escape = root
            .path()
            .join("..")
            .join(other.path().file_name().unwrap());
        assert!(h
            .handle(
                &Capability::FsWrite,
                &[p(&escape.join("keep.txt")), s(""), s("delete")]
            )
            .is_err());
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "x");
    }

    #[cfg(unix)]
    #[test]
    fn deleting_a_symlink_removes_the_link_not_its_target() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let target = other.path().join("keep.txt");
        std::fs::write(&target, "x").unwrap();
        let link = root.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let mut h = handler(&[root.path()]);
        h.handle(&Capability::FsWrite, &[p(&link), s(""), s("delete")])
            .unwrap();
        assert!(std::fs::symlink_metadata(&link).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "x");
    }

    #[test]
    fn delete_needs_allow_delete_in_the_policy() {
        let root = tempfile::tempdir().unwrap();
        let f = root.path().join("keep.txt");
        std::fs::write(&f, "x").unwrap();
        let mut h = SystemHandler::new(
            Some(FsPolicy {
                allowed_roots: vec![root.path().display().to_string()],
                max_read_bytes: 64,
                allow_delete: false,
                max_list_entries: 3,
            }),
            Box::new(MockHandler),
        );
        let err = h
            .handle(&Capability::FsWrite, &[p(&f), s(""), s("delete")])
            .unwrap_err();
        assert!(err.contains("fs_policy.allow_delete"), "{err}");
        assert!(f.exists());
        // Writing is still allowed.
        h.handle(&Capability::FsWrite, &[p(&f), s("y")]).unwrap();
    }

    #[test]
    fn list_over_max_list_entries_is_refused() {
        let root = tempfile::tempdir().unwrap();
        for n in ["a", "b", "c", "d"] {
            std::fs::write(root.path().join(n), "x").unwrap();
        }
        let mut h = handler(&[root.path()]);
        let err = h
            .handle(&Capability::FsRead, &[p(root.path()), s("list")])
            .unwrap_err();
        assert!(err.contains("max_list_entries (3)"), "{err}");
    }

    #[test]
    fn unknown_operation_tags_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let mut h = handler(&[root.path()]);
        let f = root.path().join("a.txt");
        assert!(h
            .handle(&Capability::FsWrite, &[p(&f), s("x"), s("truncate")])
            .is_err());
        assert!(h.handle(&Capability::FsRead, &[p(&f), s("stat")]).is_err());
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
