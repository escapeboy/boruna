use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

/// Capabilities that bytecode can request.
/// All side effects go through this interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Capability {
    NetFetch,
    FsRead,
    FsWrite,
    DbQuery,
    UiRender,
    TimeNow,
    Random,
    LlmCall,
    ActorSpawn,
    ActorSend,
    /// Read a workflow step's resolved input value (sprint `0.3-S14`).
    /// Dispatched from the compiler-recognized built-in
    /// `step_input(name: String) -> String`. The handler returns the
    /// JSON-encoded upstream output for the named input. Steps that
    /// need typed access parse the JSON; the platform stays
    /// String-to-String at the language layer.
    StepInput,
}

impl Capability {
    pub fn from_id(id: u32) -> Option<Self> {
        match id {
            0 => Some(Capability::NetFetch),
            1 => Some(Capability::FsRead),
            2 => Some(Capability::FsWrite),
            3 => Some(Capability::DbQuery),
            4 => Some(Capability::UiRender),
            5 => Some(Capability::TimeNow),
            6 => Some(Capability::Random),
            7 => Some(Capability::LlmCall),
            8 => Some(Capability::ActorSpawn),
            9 => Some(Capability::ActorSend),
            10 => Some(Capability::StepInput),
            _ => None,
        }
    }

    pub fn id(&self) -> u32 {
        match self {
            Capability::NetFetch => 0,
            Capability::FsRead => 1,
            Capability::FsWrite => 2,
            Capability::DbQuery => 3,
            Capability::UiRender => 4,
            Capability::TimeNow => 5,
            Capability::Random => 6,
            Capability::LlmCall => 7,
            Capability::ActorSpawn => 8,
            Capability::ActorSend => 9,
            Capability::StepInput => 10,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Capability::NetFetch => "net.fetch",
            Capability::FsRead => "fs.read",
            Capability::FsWrite => "fs.write",
            Capability::DbQuery => "db.query",
            Capability::UiRender => "ui.render",
            Capability::TimeNow => "time.now",
            Capability::Random => "random",
            Capability::LlmCall => "llm.call",
            Capability::ActorSpawn => "actor.spawn",
            Capability::ActorSend => "actor.send",
            Capability::StepInput => "step.input",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "net.fetch" | "net" => Some(Capability::NetFetch),
            "fs.read" => Some(Capability::FsRead),
            "fs.write" => Some(Capability::FsWrite),
            "db.query" | "db" => Some(Capability::DbQuery),
            "ui.render" | "ui" => Some(Capability::UiRender),
            "time.now" | "time" => Some(Capability::TimeNow),
            "random" => Some(Capability::Random),
            "llm.call" | "llm" => Some(Capability::LlmCall),
            "actor.spawn" | "actor_spawn" => Some(Capability::ActorSpawn),
            "actor.send" | "actor_send" => Some(Capability::ActorSend),
            "step.input" | "step_input" => Some(Capability::StepInput),
            _ => None,
        }
    }

    /// Contract version for this capability.
    ///
    /// Bump only when the capability's *contract* changes — that is, when the
    /// argument shape, return shape, or observable side-effect semantics change
    /// in a way that downstream cached results would no longer be valid.
    /// Do NOT bump on every binary release.
    ///
    /// All capabilities ship at `"1"` in 0.2.0. Future bumps must:
    ///   1. update the match arm here,
    ///   2. update `tests::test_capability_set_hash_known_value` golden hash,
    ///   3. add a CHANGELOG entry under `### Changed`,
    ///   4. mention the new version in `docs/reference/capability-identity.md`.
    pub fn version(&self) -> &'static str {
        match self {
            Capability::NetFetch
            | Capability::FsRead
            | Capability::FsWrite
            | Capability::DbQuery
            | Capability::UiRender
            | Capability::TimeNow
            | Capability::Random
            | Capability::LlmCall
            | Capability::ActorSpawn
            | Capability::ActorSend
            | Capability::StepInput => "1",
        }
    }

    /// Canonical iteration order for hashing — sorted ascending by `name()`.
    /// Locked by `tests::test_capability_all_is_sorted_by_name`.
    /// **Note:** adding a capability bumps `capability_set_hash` (additive
    /// change in surface area); FleetQ-blessed and integrators are
    /// expected to invalidate cache keys on the new hash.
    pub const ALL: [Capability; 11] = [
        Capability::ActorSend,
        Capability::ActorSpawn,
        Capability::DbQuery,
        Capability::FsRead,
        Capability::FsWrite,
        Capability::LlmCall,
        Capability::NetFetch,
        Capability::Random,
        Capability::StepInput,
        Capability::TimeNow,
        Capability::UiRender,
    ];
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// One capability's stable identity: name + contract version.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilityIdentity {
    pub name: String,
    pub version: String,
}

/// Wire-format protocol version for the capability surface report.
///
/// Bumped on **breaking shape changes** (field rename, removal, type change).
/// Additive changes (new optional field) keep the same protocol_version.
/// Documented in `docs/reference/capability-identity.md` under "Stability".
pub const CAPABILITY_REPORT_PROTOCOL_VERSION: u32 = 1;

/// The full capability surface this binary exposes.
///
/// `capability_set_hash` is a stable identity over `(name, version)` pairs;
/// integrators use it as part of their cache key to safely memoize
/// deterministic results across binary upgrades. See
/// `docs/reference/capability-identity.md`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilitySetReport {
    /// Wire-format version of this report — bumped on breaking shape changes.
    pub protocol_version: u32,
    /// Binary identity. Defaults to `"boruna"`; downstream forks that rebrand
    /// can pass their own name. Does NOT participate in `capability_set_hash`.
    pub name: String,
    /// Binary version (typically `CARGO_PKG_VERSION` of the calling crate).
    /// Does NOT participate in `capability_set_hash`.
    pub version: String,
    pub capabilities: Vec<CapabilityIdentity>,
    pub capability_set_hash: String,
}

/// Hash algorithm:
///
/// 1. For each capability in `Capability::ALL` (already sorted by name):
///    encode `"{name}\t{version}\n"` as UTF-8.
/// 2. Concatenate all encodings into a single byte string.
/// 3. SHA-256 of that byte string.
/// 4. Lower-case hex, prefixed with `"sha256:"`.
///
/// This is documented byte-for-byte in `docs/reference/capability-identity.md`
/// so external implementations can reproduce it.
pub fn compute_capability_set_hash<I, S1, S2>(entries: I) -> String
where
    I: IntoIterator<Item = (S1, S2)>,
    S1: AsRef<str>,
    S2: AsRef<str>,
{
    let mut hasher = Sha256::new();
    for (name, version) in entries {
        hasher.update(name.as_ref().as_bytes());
        hasher.update(b"\t");
        hasher.update(version.as_ref().as_bytes());
        hasher.update(b"\n");
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(7 + 64);
    out.push_str("sha256:");
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Build a `CapabilitySetReport` for the running binary.
///
/// `binary_name` and `binary_version` are parameters (rather than read from
/// `env!`) so callers control which crate's identity represents "the binary"
/// — the CLI passes its own package metadata, the MCP server passes its own.
/// Forks that rebrand can pass a different name without patching this crate.
/// Neither field participates in `capability_set_hash` (only the per-capability
/// `(name, version)` pairs do), so cached results survive binary upgrades and
/// rebrands as long as the capability contract surface is unchanged.
pub fn capability_set_report(binary_name: &str, binary_version: &str) -> CapabilitySetReport {
    let identities: Vec<CapabilityIdentity> = Capability::ALL
        .iter()
        .map(|cap| CapabilityIdentity {
            name: cap.name().to_string(),
            version: cap.version().to_string(),
        })
        .collect();

    let hash = compute_capability_set_hash(
        identities
            .iter()
            .map(|c| (c.name.as_str(), c.version.as_str())),
    );

    CapabilitySetReport {
        protocol_version: CAPABILITY_REPORT_PROTOCOL_VERSION,
        name: binary_name.to_string(),
        version: binary_version.to_string(),
        capabilities: identities,
        capability_set_hash: hash,
    }
}
