//! Durable, per-application storage with read-only Mystral save compatibility.
//!
//! Text values use one ordered JSON object, matching Mystral localStorage.
//! Existing Mystral `<key>.bin` and `.bin.bak` files remain readable.
//! New binary writes atomically publish a versioned value-or-deletion record.
//! The old storage directory is never modified during migration.

use std::collections::HashMap;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use deno_core::{JsBuffer, OpState, op2};
use deno_error::JsErrorBox;
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

const MAX_BINARY_BYTES: u64 = 512 * 1024 * 1024;
const BINARY_RECORD_MAGIC: &[u8; 8] = b"PGRSBIN1";
const BINARY_RECORD_HEADER_BYTES: u64 = 9;
const MAX_TEXT_VALUE_BYTES: usize = 768 * 1024 * 1024;
const MAX_TEXT_TOTAL_BYTES: usize = 1024 * 1024 * 1024;
const MAX_TEXT_KEY_BYTES: usize = 4096;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

enum BinaryRecord {
    Deleted,
    Value(Vec<u8>),
}

#[derive(Default)]
struct OrderedText {
    keys: Vec<String>,
    values: HashMap<String, String>,
    total_bytes: usize,
}

impl OrderedText {
    fn insert_loaded(&mut self, key: String, value: String) -> Result<()> {
        validate_text_entry(&key, &value)?;
        if !self.values.contains_key(&key) {
            self.keys.push(key.clone());
        }
        let previous = self.values.insert(key.clone(), value);
        if let Some(previous) = previous {
            self.total_bytes -= key.len() + previous.len();
        }
        self.total_bytes += key.len() + self.values[&key].len();
        if self.total_bytes > MAX_TEXT_TOTAL_BYTES {
            bail!("localStorage exceeds the 1 GiB total limit");
        }
        Ok(())
    }

    fn set(&mut self, path: &Path, key: String, value: String) -> Result<()> {
        validate_text_entry(&key, &value)?;
        let old_size = self.values.get(&key).map_or(0, |old| key.len() + old.len());
        let new_size = key.len() + value.len();
        let next_size = self.total_bytes - old_size + new_size;
        if next_size > MAX_TEXT_TOTAL_BYTES {
            bail!("localStorage exceeds the 1 GiB total limit");
        }
        let was_new = !self.values.contains_key(&key);
        if was_new {
            self.keys.push(key.clone());
        }
        let old = self.values.insert(key.clone(), value);
        let previous_size = self.total_bytes;
        self.total_bytes = next_size;
        if let Err(error) = self.persist(path) {
            self.total_bytes = previous_size;
            self.values.remove(&key);
            if let Some(old) = old {
                self.values.insert(key, old);
            } else {
                self.keys.pop();
            }
            return Err(error);
        }
        Ok(())
    }

    fn remove(&mut self, path: &Path, key: &str) -> Result<()> {
        let Some(value) = self.values.remove(key) else {
            return Ok(());
        };
        let index = self
            .keys
            .iter()
            .position(|stored| stored == key)
            .expect("key order");
        let owned_key = self.keys.remove(index);
        self.total_bytes -= owned_key.len() + value.len();
        if let Err(error) = self.persist(path) {
            self.total_bytes += owned_key.len() + value.len();
            self.keys.insert(index, owned_key.clone());
            self.values.insert(owned_key, value);
            return Err(error);
        }
        Ok(())
    }

    fn clear(&mut self, path: &Path) -> Result<()> {
        let prior = std::mem::take(self);
        if let Err(error) = self.persist(path) {
            *self = prior;
            return Err(error);
        }
        Ok(())
    }

    fn persist(&self, path: &Path) -> Result<()> {
        let bytes = serde_json::to_vec(self).context("serializing localStorage")?;
        if bytes.len() > MAX_TEXT_TOTAL_BYTES {
            bail!("serialized localStorage exceeds the 1 GiB limit");
        }
        // A corrupt primary must never replace its valid backup during repair.
        let backup_existing = if path.exists() {
            match load_text(path) {
                Ok(_) => true,
                Err(_error) if backup_path(path).exists() => false,
                Err(error) => return Err(error),
            }
        } else {
            false
        };
        atomic_write(path, &bytes, backup_existing)
    }
}

fn validate_text_entry(key: &str, value: &str) -> Result<()> {
    if key.len() > MAX_TEXT_KEY_BYTES {
        bail!("localStorage key exceeds 4096 UTF-8 bytes");
    }
    if value.len() > MAX_TEXT_VALUE_BYTES {
        bail!("localStorage value exceeds 768 MiB");
    }
    Ok(())
}

impl Serialize for OrderedText {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut object = serializer.serialize_map(Some(self.keys.len()))?;
        for key in &self.keys {
            object.serialize_entry(key, &self.values[key])?;
        }
        object.end()
    }
}

impl<'de> Deserialize<'de> for OrderedText {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct OrderedVisitor;
        impl<'de> Visitor<'de> for OrderedVisitor {
            type Value = OrderedText;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a flat JSON object of string keys and values")
            }

            fn visit_map<M: MapAccess<'de>>(
                self,
                mut access: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                let mut result = OrderedText::default();
                while let Some((key, value)) = access.next_entry::<String, String>()? {
                    result
                        .insert_loaded(key, value)
                        .map_err(serde::de::Error::custom)?;
                }
                Ok(result)
            }
        }
        deserializer.deserialize_map(OrderedVisitor)
    }
}

pub struct StorageBackend {
    namespace: String,
    directory: PathBuf,
    text_path: PathBuf,
    binary_directory: PathBuf,
    legacy_text_path: Option<PathBuf>,
    legacy_binary_directory: Option<PathBuf>,
    text: OrderedText,
}

/// Native storage access independent of V8 and its thread. Cloning this handle
/// copies paths only; cached localStorage values and binary payloads are not copied.
pub struct NativeStorage(StorageBackend);

impl Clone for NativeStorage {
    fn clone(&self) -> Self {
        self.0.native_handle()
    }
}

impl NativeStorage {
    pub fn binary_get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.0.binary_get(key)
    }
    pub fn binary_backup(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.0.binary_backup(key)
    }
    pub fn binary_has(&self, key: &str, backup: bool) -> Result<bool> {
        self.0.binary_has(key, backup)
    }
    pub fn binary_set(&self, key: &str, bytes: &[u8]) -> Result<()> {
        self.0.binary_set(key, bytes)
    }
    /// Validate both the new value and any existing recovered value while holding
    /// the namespace lock, so another process cannot change it before publication.
    pub fn binary_set_validated(
        &self,
        key: &str,
        bytes: &[u8],
        validate: impl Fn(&[u8]) -> Result<()>,
    ) -> Result<()> {
        let _lock = self.0.lock()?;
        validate(bytes)?;
        if let Some(previous) = self.0.binary_get_unlocked(key)? {
            validate(&previous).context("refusing to overwrite an invalid existing save")?;
        }
        self.0.binary_set_unlocked(key, bytes)
    }
    pub fn binary_remove(&self, key: &str) -> Result<()> {
        self.0.binary_remove(key)
    }
    pub fn text_get(&self, key: &str) -> Result<Option<String>> {
        let mut backend = self.0.native_handle().0;
        backend.text_get(key).map(|value| value.map(str::to_owned))
    }
    pub fn text_remove(&self, key: &str) -> Result<()> {
        self.0.native_handle().0.text_remove(key)
    }
}

impl StorageBackend {
    pub fn native_handle(&self) -> NativeStorage {
        NativeStorage(Self {
            namespace: self.namespace.clone(),
            directory: self.directory.clone(),
            text_path: self.text_path.clone(),
            binary_directory: self.binary_directory.clone(),
            legacy_text_path: self.legacy_text_path.clone(),
            legacy_binary_directory: self.legacy_binary_directory.clone(),
            text: OrderedText::default(),
        })
    }
    /// `storage_dir_override` is a storage root; the namespace remains appended.
    /// `legacy_namespace` is the old Mystral cwd stem ("wuxia" for Wuxia).
    pub fn open(
        project_root: &Path,
        storage_dir_override: Option<&Path>,
        namespace_override: Option<&str>,
        legacy_namespace: Option<&str>,
    ) -> Result<Self> {
        let legacy_root = if legacy_namespace.is_some() {
            Some(legacy_storage_root()?)
        } else {
            None
        };
        Self::open_with_legacy_root(
            project_root,
            storage_dir_override,
            namespace_override,
            legacy_namespace,
            legacy_root.as_deref(),
        )
    }

    fn open_with_legacy_root(
        project_root: &Path,
        storage_dir_override: Option<&Path>,
        namespace_override: Option<&str>,
        legacy_namespace: Option<&str>,
        legacy_root: Option<&Path>,
    ) -> Result<Self> {
        let namespace = match namespace_override {
            Some(value) => validate_namespace(value)?,
            None => project_namespace(project_root)?,
        };
        let legacy_namespace = legacy_namespace.map(validate_namespace).transpose()?;
        let storage_root = match storage_dir_override {
            Some(path) => path.to_path_buf(),
            None => default_storage_root()?,
        };
        let directory = storage_root.join(&namespace);
        fs::create_dir_all(&directory)
            .with_context(|| format!("creating storage directory {}", directory.display()))?;
        let text_path = directory.join("local-storage.json");
        let binary_directory = directory.join("binary");
        fs::create_dir_all(&binary_directory).context("creating binary storage directory")?;

        let legacy_text = legacy_root
            .zip(legacy_namespace.as_ref())
            .map(|(root, namespace)| root.join(format!("{namespace}.json")));
        let legacy_binary_directory = legacy_text.as_ref().map(|path| appended_path(path, ".d"));
        let text = if text_path.exists() || backup_path(&text_path).exists() {
            load_text_with_backup(&text_path)?
        } else if let Some(path) = legacy_text.as_ref().filter(|path| path.exists()) {
            load_text(path)
                .with_context(|| format!("reading legacy Mystral storage {}", path.display()))?
        } else {
            OrderedText::default()
        };
        Ok(Self {
            namespace,
            directory,
            text_path,
            binary_directory,
            legacy_text_path: legacy_text,
            legacy_binary_directory,
            text,
        })
    }

    pub fn text_get(&mut self, key: &str) -> Result<Option<&str>> {
        let _lock = self.lock()?;
        self.reload_text()?;
        Ok(self.text.values.get(key).map(String::as_str))
    }
    pub fn text_length(&mut self) -> Result<usize> {
        let _lock = self.lock()?;
        self.reload_text()?;
        Ok(self.text.keys.len())
    }
    pub fn text_key(&self, index: usize) -> Option<&str> {
        self.text.keys.get(index).map(String::as_str)
    }
    pub fn text_set(&mut self, key: String, value: String) -> Result<()> {
        let _lock = self.lock()?;
        self.reload_text()?;
        self.text.set(&self.text_path, key, value)
    }
    pub fn text_remove(&mut self, key: &str) -> Result<()> {
        let _lock = self.lock()?;
        self.reload_text()?;
        self.text.remove(&self.text_path, key)
    }
    pub fn text_clear(&mut self) -> Result<()> {
        let _lock = self.lock()?;
        self.reload_text()?;
        self.text.clear(&self.text_path)
    }

    fn reload_text(&mut self) -> Result<()> {
        self.text = if self.text_path.exists() || backup_path(&self.text_path).exists() {
            load_text_with_backup(&self.text_path)?
        } else if let Some(path) = self.legacy_text_path.as_ref().filter(|path| path.exists()) {
            load_text(path)
                .with_context(|| format!("reading legacy Mystral storage {}", path.display()))?
        } else {
            OrderedText::default()
        };
        Ok(())
    }

    fn lock(&self) -> Result<File> {
        let path = self.directory.join(".storage.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("opening storage lock {}", path.display()))?;
        file.lock()
            .with_context(|| format!("locking storage namespace {}", self.namespace))?;
        Ok(file)
    }

    pub fn binary_get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let _lock = self.lock()?;
        self.binary_get_unlocked(key)
    }

    fn binary_get_unlocked(&self, key: &str) -> Result<Option<Vec<u8>>> {
        if let Some(record) = read_current_record(&self.record_path(key)?)? {
            return Ok(match record {
                BinaryRecord::Deleted => None,
                BinaryRecord::Value(bytes) => Some(bytes),
            });
        }
        let live = self.binary_path(key)?;
        if self.tombstone_path(key)?.exists() {
            return Ok(None);
        }
        let backup = backup_path(&live);
        if live.exists() {
            return match read_binary(&live) {
                Ok(value) => Ok(Some(value)),
                Err(primary_error) if backup.exists() => {
                    read_binary(&backup).map(Some).with_context(|| {
                        format!(
                            "primary binary storage failed: {primary_error:#}; backup also failed"
                        )
                    })
                }
                Err(error) => Err(error),
            };
        }
        if backup.exists() {
            return read_binary(&backup).map(Some);
        }
        let Some(legacy) = self.legacy_path(key)? else {
            return Ok(None);
        };
        if legacy.exists() {
            return match read_binary(&legacy) {
                Ok(value) => Ok(Some(value)),
                Err(primary_error) if backup_path(&legacy).exists() => read_binary(&backup_path(
                    &legacy,
                ))
                .map(Some)
                .with_context(|| {
                    format!("legacy binary storage failed: {primary_error:#}; backup also failed")
                }),
                Err(error) => Err(error),
            };
        }
        let backup = backup_path(&legacy);
        if backup.exists() {
            return read_binary(&backup).map(Some);
        }
        Ok(None)
    }

    pub fn binary_backup(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let _lock = self.lock()?;
        let record_path = self.record_path(key)?;
        if let Some(has_value) = read_current_record_state(&record_path)? {
            if !has_value {
                return Ok(None);
            }
            if backup_path(&record_path).exists() {
                return Ok(match read_binary_record(&backup_path(&record_path))? {
                    BinaryRecord::Deleted => None,
                    BinaryRecord::Value(bytes) => Some(bytes),
                });
            }
            if self.tombstone_path(key)?.exists() {
                return Ok(None);
            }
            let previous = self.binary_path(key)?;
            if previous.exists() {
                return read_binary(&previous).map(Some);
            }
            if backup_path(&previous).exists() {
                return read_binary(&backup_path(&previous)).map(Some);
            }
            if let Some(legacy) = self.legacy_path(key)? {
                if legacy.exists() {
                    return read_binary(&legacy).map(Some);
                }
                if backup_path(&legacy).exists() {
                    return read_binary(&backup_path(&legacy)).map(Some);
                }
            }
            return Ok(None);
        }
        if self.tombstone_path(key)?.exists() {
            return Ok(None);
        }
        let live = self.binary_path(key)?;
        let backup = backup_path(&live);
        if backup.exists() {
            return read_binary(&backup).map(Some);
        }
        let Some(legacy) = self.legacy_path(key)? else {
            return Ok(None);
        };
        let backup = backup_path(&legacy);
        if backup.exists() {
            return read_binary(&backup).map(Some);
        }
        // A migrated save may still have a valid old live copy as recovery.
        if live.exists() && legacy.exists() {
            return read_binary(&legacy).map(Some);
        }
        Ok(None)
    }

    pub fn binary_has(&self, key: &str, backup: bool) -> Result<bool> {
        let _lock = self.lock()?;
        let record_path = self.record_path(key)?;
        if let Some(has_value) = read_current_record_state(&record_path)? {
            if !has_value {
                return Ok(false);
            }
            if !backup {
                return Ok(true);
            }
            if backup_path(&record_path).exists() {
                return read_binary_record_state(&backup_path(&record_path));
            }
            if self.tombstone_path(key)?.exists() {
                return Ok(false);
            }
            let previous = self.binary_path(key)?;
            return Ok(previous.exists()
                || backup_path(&previous).exists()
                || self
                    .legacy_path(key)?
                    .as_ref()
                    .is_some_and(|legacy| legacy.exists() || backup_path(legacy).exists()));
        }
        if self.tombstone_path(key)?.exists() {
            return Ok(false);
        }
        let live = self.binary_path(key)?;
        let old = self.legacy_path(key)?;
        if backup {
            Ok(backup_path(&live).exists()
                || old
                    .as_ref()
                    .is_some_and(|old| backup_path(old).exists() || live.exists() && old.exists()))
        } else {
            Ok(live.exists()
                || backup_path(&live).exists()
                || old
                    .as_ref()
                    .is_some_and(|old| old.exists() || backup_path(old).exists()))
        }
    }

    pub fn binary_set(&self, key: &str, bytes: &[u8]) -> Result<()> {
        let _lock = self.lock()?;
        self.binary_set_unlocked(key, bytes)
    }

    fn binary_set_unlocked(&self, key: &str, bytes: &[u8]) -> Result<()> {
        if bytes.len() as u64 > MAX_BINARY_BYTES {
            bail!("binary storage value exceeds 512 MiB");
        }
        let path = self.record_path(key)?;
        read_current_record_state(&path)?;
        let backup_primary = path.exists() && read_binary_record_state(&path).is_ok();
        atomic_write_parts(&path, &[b"PGRSBIN1V", bytes], backup_primary)
    }

    pub fn binary_remove(&self, key: &str) -> Result<()> {
        let _lock = self.lock()?;
        let mut record = Vec::with_capacity(BINARY_RECORD_HEADER_BYTES as usize);
        record.extend_from_slice(BINARY_RECORD_MAGIC);
        record.push(b'D');
        let path = self.record_path(key)?;
        read_current_record_state(&path)?;
        let backup_primary = path.exists() && read_binary_record_state(&path).is_ok();
        atomic_write(&path, &record, backup_primary)
    }

    fn record_path(&self, key: &str) -> Result<PathBuf> {
        validate_binary_key(key)?;
        Ok(self.binary_directory.join(format!("{key}.record")))
    }

    fn binary_path(&self, key: &str) -> Result<PathBuf> {
        validate_binary_key(key)?;
        Ok(self.binary_directory.join(format!("{key}.bin")))
    }
    fn tombstone_path(&self, key: &str) -> Result<PathBuf> {
        validate_binary_key(key)?;
        Ok(self.binary_directory.join(format!("{key}.deleted")))
    }
    fn legacy_path(&self, key: &str) -> Result<Option<PathBuf>> {
        validate_binary_key(key)?;
        Ok(self
            .legacy_binary_directory
            .as_ref()
            .map(|directory| directory.join(format!("{key}.bin"))))
    }
}

fn project_namespace(root: &Path) -> Result<String> {
    let manifest = root.join("package.json");
    if manifest.exists() {
        let bytes = fs::read(&manifest).context("reading package.json for storage namespace")?;
        let package: serde_json::Value =
            serde_json::from_slice(&bytes).context("parsing package.json for storage namespace")?;
        if let Some(name) = package.get("name").and_then(serde_json::Value::as_str) {
            return validate_namespace(name);
        }
    }
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("project root has no storage namespace"))?;
    validate_namespace(name)
}

fn validate_namespace(value: &str) -> Result<String> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        bail!("storage namespace must be 1-64 ASCII letters, digits, hyphens, or underscores");
    }
    Ok(value.to_ascii_lowercase())
}

fn validate_binary_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.len() > 96
        || !key.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' || byte == b'.'
        })
    {
        bail!("invalid binary storage key");
    }
    Ok(())
}

fn default_storage_root() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        Ok(user_data_home()?.join("Peregrust").join("storage"))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(user_data_home()?.join("Peregrust").join("storage"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Ok(user_data_home()?.join("peregrust").join("storage"))
    }
}

fn legacy_storage_root() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        Ok(user_data_home()?.join("Mystral").join("storage"))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(user_data_home()?.join("Mystral").join("storage"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Ok(user_data_home()?.join("mystral").join("storage"))
    }
}

fn user_data_home() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        env::var_os("APPDATA")
            .or_else(|| {
                env::var_os("USERPROFILE").map(|home| {
                    PathBuf::from(home)
                        .join("AppData")
                        .join("Roaming")
                        .into_os_string()
                })
            })
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("APPDATA is unavailable; pass --storage-dir"))
    }
    #[cfg(target_os = "macos")]
    {
        env::var_os("HOME")
            .map(|home| {
                PathBuf::from(home)
                    .join("Library")
                    .join("Application Support")
            })
            .ok_or_else(|| anyhow!("HOME is unavailable; pass --storage-dir"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("share"))
            })
            .ok_or_else(|| anyhow!("user data directory unavailable; pass --storage-dir"))
    }
}

fn load_text(path: &Path) -> Result<OrderedText> {
    let bytes = read_limited(path, MAX_TEXT_TOTAL_BYTES as u64)?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing localStorage {}", path.display()))
}

fn load_text_with_backup(path: &Path) -> Result<OrderedText> {
    let backup = backup_path(path);
    if path.exists() {
        return match load_text(path) {
            Ok(value) => Ok(value),
            Err(primary_error) if backup.exists() => load_text(&backup).with_context(|| {
                format!("primary localStorage failed: {primary_error:#}; backup also failed")
            }),
            Err(error) => Err(error),
        };
    }
    if backup.exists() {
        return load_text(&backup);
    }
    Err(anyhow!("localStorage file is missing"))
}

fn read_binary(path: &Path) -> Result<Vec<u8>> {
    read_limited(path, MAX_BINARY_BYTES)
}

fn read_binary_record(path: &Path) -> Result<BinaryRecord> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    if !(BINARY_RECORD_HEADER_BYTES..=MAX_BINARY_BYTES + BINARY_RECORD_HEADER_BYTES)
        .contains(&length)
    {
        bail!("invalid binary storage record size {}", path.display());
    }
    let mut header = [0u8; BINARY_RECORD_HEADER_BYTES as usize];
    file.read_exact(&mut header)?;
    if &header[..BINARY_RECORD_MAGIC.len()] != BINARY_RECORD_MAGIC {
        bail!("invalid binary storage record {}", path.display());
    }
    match header[BINARY_RECORD_MAGIC.len()] {
        b'D' if length == BINARY_RECORD_HEADER_BYTES => Ok(BinaryRecord::Deleted),
        b'V' => {
            let mut bytes = Vec::with_capacity((length - BINARY_RECORD_HEADER_BYTES) as usize);
            file.take(MAX_BINARY_BYTES + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 != length - BINARY_RECORD_HEADER_BYTES {
                bail!(
                    "binary storage record changed while reading {}",
                    path.display()
                );
            }
            Ok(BinaryRecord::Value(bytes))
        }
        _ => bail!("invalid binary storage record state {}", path.display()),
    }
}

fn read_binary_record_state(path: &Path) -> Result<bool> {
    let metadata = fs::metadata(path).with_context(|| format!("inspecting {}", path.display()))?;
    if metadata.len() < BINARY_RECORD_HEADER_BYTES
        || metadata.len() > MAX_BINARY_BYTES + BINARY_RECORD_HEADER_BYTES
    {
        bail!("invalid binary storage record size {}", path.display());
    }
    let mut header = [0u8; BINARY_RECORD_HEADER_BYTES as usize];
    File::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .read_exact(&mut header)
        .with_context(|| format!("reading binary record header {}", path.display()))?;
    if &header[..BINARY_RECORD_MAGIC.len()] != BINARY_RECORD_MAGIC {
        bail!("invalid binary storage record {}", path.display());
    }
    match header[BINARY_RECORD_MAGIC.len()] {
        b'D' if metadata.len() == BINARY_RECORD_HEADER_BYTES => Ok(false),
        b'V' => Ok(true),
        _ => bail!("invalid binary storage record state {}", path.display()),
    }
}

fn read_current_record_state(path: &Path) -> Result<Option<bool>> {
    let backup = backup_path(path);
    if path.exists() {
        return match read_binary_record_state(path) {
            Ok(state) => Ok(Some(state)),
            Err(primary_error) if backup.exists() => read_binary_record_state(&backup)
                .map(Some)
                .with_context(|| {
                    format!("primary binary record failed: {primary_error:#}; backup also failed")
                }),
            Err(error) => Err(error),
        };
    }
    if backup.exists() {
        return read_binary_record_state(&backup).map(Some);
    }
    Ok(None)
}

fn read_current_record(path: &Path) -> Result<Option<BinaryRecord>> {
    let backup = backup_path(path);
    if path.exists() {
        return match read_binary_record(path) {
            Ok(record) => Ok(Some(record)),
            Err(primary_error) if backup.exists() => {
                read_binary_record(&backup).map(Some).with_context(|| {
                    format!("primary binary record failed: {primary_error:#}; backup also failed")
                })
            }
            Err(error) => Err(error),
        };
    }
    if backup.exists() {
        return read_binary_record(&backup).map(Some);
    }
    Ok(None)
}

fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let metadata = fs::metadata(path).with_context(|| format!("inspecting {}", path.display()))?;
    if metadata.len() > limit {
        bail!(
            "storage file {} exceeds configured size limit",
            path.display()
        );
    }
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {}", path.display()))?;
    if bytes.len() as u64 > limit {
        bail!(
            "storage file {} exceeds configured size limit",
            path.display()
        );
    }
    Ok(bytes)
}

fn appended_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn backup_path(path: &Path) -> PathBuf {
    appended_path(path, ".bak")
}

fn temporary_path(path: &Path) -> PathBuf {
    let number = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    appended_path(path, &format!(".{}.{}.tmp", std::process::id(), number))
}

fn atomic_write(path: &Path, bytes: &[u8], backup: bool) -> Result<()> {
    atomic_write_parts(path, &[bytes], backup)
}

fn atomic_write_parts(path: &Path, parts: &[&[u8]], backup: bool) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("storage path has no parent"))?;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let staged = temporary_path(path);
    let outcome = (|| {
        write_synced_parts(&staged, parts)?;
        if backup && path.exists() {
            let previous = backup_path(path);
            let backup_staged = temporary_path(&previous);
            let backup_outcome: Result<()> = (|| {
                fs::copy(path, &backup_staged)
                    .with_context(|| format!("staging backup {}", previous.display()))?;
                OpenOptions::new()
                    .write(true)
                    .open(&backup_staged)?
                    .sync_all()?;
                replace_path(&backup_staged, &previous)?;
                Ok(())
            })();
            if backup_outcome.is_err() {
                let _ = fs::remove_file(&backup_staged);
            }
            backup_outcome?;
        }
        replace_path(&staged, path)?;
        sync_parent(parent)?;
        Ok(())
    })();
    if outcome.is_err() {
        let _ = fs::remove_file(&staged);
    }
    outcome
}

#[cfg(test)]
fn write_synced(path: &Path, bytes: &[u8]) -> Result<()> {
    write_synced_parts(path, &[bytes])
}

fn write_synced_parts(path: &Path, parts: &[&[u8]]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("creating staged storage file {}", path.display()))?;
    for bytes in parts {
        file.write_all(bytes)
            .with_context(|| format!("writing {}", path.display()))?;
    }
    file.sync_all()
        .with_context(|| format!("syncing {}", path.display()))
}

#[cfg(windows)]
fn replace_path(staged: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new_name: *const u16, flags: u32) -> i32;
    }
    const REPLACE_EXISTING: u32 = 0x1;
    const WRITE_THROUGH: u32 = 0x8;
    let from: Vec<u16> = staged.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let success =
        unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), REPLACE_EXISTING | WRITE_THROUGH) };
    if success == 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("atomically replacing {}", destination.display()));
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_path(staged: &Path, destination: &Path) -> Result<()> {
    fs::rename(staged, destination)
        .with_context(|| format!("atomically replacing {}", destination.display()))
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> Result<()> {
    File::open(parent)?
        .sync_all()
        .context("syncing storage directory")
}
#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> Result<()> {
    Ok(())
}

fn js_error(error: anyhow::Error) -> JsErrorBox {
    JsErrorBox::generic(format!("{error:#}"))
}

#[op2]
#[string]
fn op_peregrust_storage_text_get(
    state: &mut OpState,
    #[string] key: String,
) -> Result<Option<String>, JsErrorBox> {
    state
        .borrow_mut::<StorageBackend>()
        .text_get(&key)
        .map(|value| value.map(str::to_owned))
        .map_err(js_error)
}

#[op2]
#[string]
fn op_peregrust_storage_text_key(state: &OpState, index: u32) -> Option<String> {
    state
        .borrow::<StorageBackend>()
        .text_key(index as usize)
        .map(str::to_owned)
}

#[op2(fast)]
fn op_peregrust_storage_text_length(state: &mut OpState) -> Result<u32, JsErrorBox> {
    state
        .borrow_mut::<StorageBackend>()
        .text_length()
        .map(|length| length as u32)
        .map_err(js_error)
}

#[op2(fast)]
fn op_peregrust_storage_text_set(
    state: &mut OpState,
    #[string] key: String,
    #[string] value: String,
) -> Result<(), JsErrorBox> {
    state
        .borrow_mut::<StorageBackend>()
        .text_set(key, value)
        .map_err(js_error)
}

#[op2(fast)]
fn op_peregrust_storage_text_remove(
    state: &mut OpState,
    #[string] key: String,
) -> Result<(), JsErrorBox> {
    state
        .borrow_mut::<StorageBackend>()
        .text_remove(&key)
        .map_err(js_error)
}

#[op2(fast)]
fn op_peregrust_storage_text_clear(state: &mut OpState) -> Result<(), JsErrorBox> {
    state
        .borrow_mut::<StorageBackend>()
        .text_clear()
        .map_err(js_error)
}

#[op2(fast)]
fn op_peregrust_storage_binary_has(
    state: &OpState,
    #[string] key: String,
    backup: bool,
) -> Result<bool, JsErrorBox> {
    state
        .borrow::<StorageBackend>()
        .binary_has(&key, backup)
        .map_err(js_error)
}

#[op2]
#[buffer]
fn op_peregrust_storage_binary_get(
    state: &OpState,
    #[string] key: String,
    backup: bool,
) -> Result<Vec<u8>, JsErrorBox> {
    let storage = state.borrow::<StorageBackend>();
    let value = if backup {
        storage.binary_backup(&key)
    } else {
        storage.binary_get(&key)
    }
    .map_err(js_error)?;
    value.ok_or_else(|| JsErrorBox::generic("binary storage value is missing"))
}

#[op2]
fn op_peregrust_storage_binary_set(
    state: &OpState,
    #[string] key: String,
    #[buffer] value: JsBuffer,
) -> Result<(), JsErrorBox> {
    state
        .borrow::<StorageBackend>()
        .binary_set(&key, &value)
        .map_err(js_error)
}

#[op2(fast)]
fn op_peregrust_storage_binary_remove(
    state: &OpState,
    #[string] key: String,
) -> Result<(), JsErrorBox> {
    state
        .borrow::<StorageBackend>()
        .binary_remove(&key)
        .map_err(js_error)
}

deno_core::extension!(
    peregrust_storage,
    ops = [
        op_peregrust_storage_text_get,
        op_peregrust_storage_text_key,
        op_peregrust_storage_text_length,
        op_peregrust_storage_text_set,
        op_peregrust_storage_text_remove,
        op_peregrust_storage_text_clear,
        op_peregrust_storage_binary_has,
        op_peregrust_storage_binary_get,
        op_peregrust_storage_binary_set,
        op_peregrust_storage_binary_remove,
    ],
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_handle_is_thread_safe_and_rejects_invalid_overwrites() {
        let temp = tempfile::tempdir().unwrap();
        let storage =
            StorageBackend::open(temp.path(), Some(temp.path()), Some("native"), None).unwrap();
        let native = storage.native_handle();
        let worker = native.clone();
        std::thread::spawn(move || worker.binary_set("save", b"valid:first").unwrap())
            .join()
            .unwrap();
        let validate = |bytes: &[u8]| -> Result<()> {
            anyhow::ensure!(bytes.starts_with(b"valid:"), "invalid checkpoint");
            Ok(())
        };
        native
            .binary_set_validated("save", b"valid:second", validate)
            .unwrap();
        assert_eq!(
            native.binary_backup("save").unwrap().unwrap(),
            b"valid:first"
        );
        assert!(
            native
                .binary_set_validated("save", b"broken", validate)
                .is_err()
        );
        assert_eq!(native.binary_get("save").unwrap().unwrap(), b"valid:second");
        native.binary_set("save", b"broken").unwrap();
        assert!(
            native
                .binary_set_validated("save", b"valid:third", validate)
                .is_err()
        );
        assert_eq!(native.binary_get("save").unwrap().unwrap(), b"broken");
        assert_eq!(
            native.binary_backup("save").unwrap().unwrap(),
            b"valid:second"
        );
    }

    #[test]
    fn segmented_records_preserve_empty_and_large_values() {
        let temp = tempfile::tempdir().unwrap();
        let storage =
            StorageBackend::open(temp.path(), Some(temp.path()), Some("native"), None).unwrap();
        storage.binary_set("save", b"").unwrap();
        assert_eq!(storage.binary_get("save").unwrap().unwrap(), b"");
        let bytes: Vec<u8> = (0..1_048_576).map(|i| (i % 251) as u8).collect();
        storage.binary_set("save", &bytes).unwrap();
        assert_eq!(storage.binary_get("save").unwrap().unwrap(), bytes);
        assert_eq!(storage.binary_backup("save").unwrap().unwrap(), b"");
        fs::write(storage.record_path("save").unwrap(), b"PGRSBIN1Dtrailing").unwrap();
        assert!(read_binary_record(&storage.record_path("save").unwrap()).is_err());
    }

    #[test]
    fn text_order_and_rollback_survive_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("game");
        fs::create_dir_all(&root).unwrap();
        let mut storage =
            StorageBackend::open(&root, Some(temp.path()), Some("game"), None).unwrap();
        storage.text_set("b".into(), "two".into()).unwrap();
        storage.text_set("a".into(), "one".into()).unwrap();
        storage.text_set("b".into(), "updated".into()).unwrap();
        assert_eq!(storage.text_key(0), Some("b"));
        assert_eq!(storage.text_key(1), Some("a"));
        let mut storage =
            StorageBackend::open(&root, Some(temp.path()), Some("game"), None).unwrap();
        assert_eq!(storage.text_get("b").unwrap(), Some("updated"));
        assert_eq!(storage.text_key(0), Some("b"));
    }

    #[test]
    fn binary_backup_and_tombstone() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("game");
        fs::create_dir_all(&root).unwrap();
        let storage = StorageBackend::open(&root, Some(temp.path()), Some("game"), None).unwrap();
        storage.binary_set("save.1", b"first").unwrap();
        storage.binary_set("save.1", b"second").unwrap();
        assert_eq!(storage.binary_get("save.1").unwrap().unwrap(), b"second");
        assert_eq!(storage.binary_backup("save.1").unwrap().unwrap(), b"first");
        storage.binary_remove("save.1").unwrap();
        assert!(storage.binary_get("save.1").unwrap().is_none());
        assert!(!storage.binary_has("save.1", false).unwrap());
        storage.binary_set("save.1", b"third").unwrap();
        assert_eq!(storage.binary_get("save.1").unwrap().unwrap(), b"third");
    }

    #[test]
    fn binary_record_commit_never_exposes_deleted_legacy_save() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("game");
        fs::create_dir_all(&root).unwrap();
        let storage = StorageBackend::open(&root, Some(temp.path()), Some("game"), None).unwrap();
        let old = storage.binary_path("save.1").unwrap();
        fs::write(&old, b"old save").unwrap();
        fs::write(storage.tombstone_path("save.1").unwrap(), b"deleted\n").unwrap();
        assert!(storage.binary_get("save.1").unwrap().is_none());

        let path = storage.record_path("save.1").unwrap();
        let staged = temporary_path(&path);
        write_synced(&staged, b"PGRSBIN1Vnew save").unwrap();
        assert!(storage.binary_get("save.1").unwrap().is_none());
        replace_path(&staged, &path).unwrap();
        assert_eq!(storage.binary_get("save.1").unwrap().unwrap(), b"new save");
        assert!(storage.binary_backup("save.1").unwrap().is_none());

        let staged = temporary_path(&path);
        write_synced(&staged, b"PGRSBIN1D").unwrap();
        assert_eq!(storage.binary_get("save.1").unwrap().unwrap(), b"new save");
        replace_path(&staged, &path).unwrap();
        assert!(storage.binary_get("save.1").unwrap().is_none());
        assert_eq!(fs::read(&old).unwrap(), b"old save");
        storage.binary_set("save.1", b"replacement").unwrap();
        assert_eq!(
            storage.binary_get("save.1").unwrap().unwrap(),
            b"replacement"
        );
        assert!(storage.binary_backup("save.1").unwrap().is_none());
    }

    #[test]
    fn corrupt_binary_record_does_not_replace_valid_backup() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("game");
        fs::create_dir_all(&root).unwrap();
        let storage = StorageBackend::open(&root, Some(temp.path()), Some("game"), None).unwrap();
        storage.binary_set("save.1", b"first").unwrap();
        storage.binary_set("save.1", b"second").unwrap();
        let path = storage.record_path("save.1").unwrap();
        fs::write(&path, b"invalid").unwrap();
        assert_eq!(storage.binary_get("save.1").unwrap().unwrap(), b"first");
        storage.binary_set("save.1", b"third").unwrap();
        assert_eq!(storage.binary_get("save.1").unwrap().unwrap(), b"third");
        assert_eq!(storage.binary_backup("save.1").unwrap().unwrap(), b"first");
    }

    #[test]
    fn invalid_binary_key_cannot_escape_storage() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("game");
        fs::create_dir_all(&root).unwrap();
        let storage = StorageBackend::open(&root, Some(temp.path()), Some("game"), None).unwrap();
        assert!(storage.binary_set("../escape", b"bad").is_err());
        assert!(storage.binary_get("C:/escape").is_err());
    }

    #[test]
    fn legacy_mystral_data_is_read_only_and_first_write_migrates_safely() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let legacy_root = temp.path().join("mystral");
        let new_root = temp.path().join("peregrust");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&legacy_root).unwrap();
        let legacy_text = legacy_root.join("wuxia.json");
        fs::write(
            &legacy_text,
            b"{\"settings\":\"old\",\"catalog\":\"saved\"}",
        )
        .unwrap();
        let legacy_binary = legacy_root.join("wuxia.json.d");
        fs::create_dir_all(&legacy_binary).unwrap();
        let old_save = legacy_binary.join("jianghu.simulation-ui.manual.checkpoint.v1.bin");
        fs::write(&old_save, b"old checkpoint").unwrap();

        let mut store = StorageBackend::open_with_legacy_root(
            &project,
            Some(&new_root),
            Some("wuxiaworld"),
            Some("wuxia"),
            Some(&legacy_root),
        )
        .unwrap();
        assert_eq!(store.text_get("settings").unwrap(), Some("old"));
        assert_eq!(
            store
                .binary_get("jianghu.simulation-ui.manual.checkpoint.v1")
                .unwrap()
                .unwrap(),
            b"old checkpoint"
        );
        store.text_set("settings".into(), "new".into()).unwrap();
        store
            .binary_set(
                "jianghu.simulation-ui.manual.checkpoint.v1",
                b"new checkpoint",
            )
            .unwrap();
        assert_eq!(
            fs::read(&legacy_text).unwrap(),
            b"{\"settings\":\"old\",\"catalog\":\"saved\"}"
        );
        assert_eq!(fs::read(&old_save).unwrap(), b"old checkpoint");

        let mut reopened = StorageBackend::open_with_legacy_root(
            &project,
            Some(&new_root),
            Some("wuxiaworld"),
            Some("wuxia"),
            Some(&legacy_root),
        )
        .unwrap();
        assert_eq!(reopened.text_get("settings").unwrap(), Some("new"));
        assert_eq!(reopened.text_get("catalog").unwrap(), Some("saved"));
        assert_eq!(
            reopened
                .binary_get("jianghu.simulation-ui.manual.checkpoint.v1")
                .unwrap()
                .unwrap(),
            b"new checkpoint"
        );
        assert_eq!(
            reopened
                .binary_backup("jianghu.simulation-ui.manual.checkpoint.v1")
                .unwrap()
                .unwrap(),
            b"old checkpoint"
        );
        reopened
            .binary_remove("jianghu.simulation-ui.manual.checkpoint.v1")
            .unwrap();
        assert!(
            reopened
                .binary_get("jianghu.simulation-ui.manual.checkpoint.v1")
                .unwrap()
                .is_none()
        );
        assert_eq!(fs::read(&old_save).unwrap(), b"old checkpoint");
    }

    #[test]
    fn malformed_new_text_does_not_silently_reset_saves() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("game");
        fs::create_dir_all(&root).unwrap();
        let directory = temp.path().join("game");
        fs::write(directory.join("local-storage.json"), b"not json").unwrap();
        assert!(StorageBackend::open(&root, Some(temp.path()), Some("game"), None).is_err());
    }

    #[test]
    fn corrupt_primary_recovers_without_overwriting_valid_backup() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("game");
        fs::create_dir_all(&root).unwrap();
        let mut storage =
            StorageBackend::open(&root, Some(temp.path()), Some("game"), None).unwrap();
        storage.text_set("save".into(), "first".into()).unwrap();
        storage.text_set("save".into(), "second".into()).unwrap();
        let primary = storage.text_path.clone();
        let backup = backup_path(&primary);
        fs::write(&primary, b"corrupt").unwrap();
        let mut reopened =
            StorageBackend::open(&root, Some(temp.path()), Some("game"), None).unwrap();
        assert_eq!(reopened.text_get("save").unwrap(), Some("first"));
        reopened.text_set("other".into(), "new".into()).unwrap();
        assert_eq!(
            load_text(&backup)
                .unwrap()
                .values
                .get("save")
                .map(String::as_str),
            Some("first")
        );
        assert_eq!(
            load_text(&primary)
                .unwrap()
                .values
                .get("other")
                .map(String::as_str),
            Some("new")
        );
    }

    #[test]
    fn two_open_instances_cannot_overwrite_each_others_text_keys() {
        use std::sync::{Arc, Barrier};
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("game");
        fs::create_dir_all(&root).unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for (key, value) in [("settings", "first"), ("catalog", "second")] {
            let root = root.clone();
            let storage_root = temp.path().to_path_buf();
            let barrier = Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || {
                let mut store =
                    StorageBackend::open(&root, Some(&storage_root), Some("game"), None).unwrap();
                barrier.wait();
                store.text_set(key.into(), value.into()).unwrap();
            }));
        }
        barrier.wait();
        for worker in workers {
            worker.join().unwrap();
        }
        let mut reopened =
            StorageBackend::open(&root, Some(temp.path()), Some("game"), None).unwrap();
        assert_eq!(reopened.text_get("settings").unwrap(), Some("first"));
        assert_eq!(reopened.text_get("catalog").unwrap(), Some("second"));
    }
}
