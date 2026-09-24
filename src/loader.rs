//! Project-local ES module loading. The loader never reads outside `root`,
//! including through symlinks or `file:` URLs supplied to dynamic `import()`.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use deno_ast::{
    EmitOptions, MediaType, ParseParams, SourceMapOption, TranspileModuleOptions, TranspileOptions,
};
use deno_core::error::ModuleLoaderError;
use deno_core::{
    ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse, ModuleLoader, ModuleResolveResponse,
    ModuleSource, ModuleSourceCode, ModuleSpecifier, ModuleType, RequestedModuleType,
    ResolutionKind,
};

const PROBE_EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "js", "jsx", "mjs", "json"];
const MAX_MODULE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_MODULES: usize = 2048;
const MAX_IMPORT_MAP_BYTES: u64 = 1024 * 1024;

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, ModuleLoaderError> {
    let file = fs::File::open(path)
        .map_err(|error| ModuleLoaderError::generic(format!("{}: {error}", path.display())))?;
    let length = file
        .metadata()
        .map_err(|error| ModuleLoaderError::generic(error.to_string()))?
        .len();
    if length > limit {
        return Err(ModuleLoaderError::generic(format!(
            "file exceeds {limit} byte limit: {}",
            path.display()
        )));
    }
    let mut bytes = Vec::with_capacity(length as usize);
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| ModuleLoaderError::generic(format!("{}: {error}", path.display())))?;
    if bytes.len() as u64 > limit {
        return Err(ModuleLoaderError::generic(format!(
            "file exceeds {limit} byte limit: {}",
            path.display()
        )));
    }
    Ok(bytes)
}

pub struct ProjectModuleLoader {
    root: PathBuf,
    imports: HashMap<String, String>,
    import_map_base: PathBuf,
    source_maps: RefCell<HashMap<String, Vec<u8>>>,
    loaded_modules: RefCell<HashSet<ModuleSpecifier>>,
}

impl ProjectModuleLoader {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, ModuleLoaderError> {
        let root = root.as_ref().canonicalize().map_err(|error| {
            ModuleLoaderError::generic(format!("cannot open project root: {error}"))
        })?;
        if !root.is_dir() {
            return Err(ModuleLoaderError::generic(format!(
                "project root is not a directory: {}",
                root.display()
            )));
        }
        Ok(Self {
            import_map_base: root.clone(),
            root,
            imports: HashMap::new(),
            source_maps: RefCell::new(HashMap::new()),
            loaded_modules: RefCell::new(HashSet::new()),
        })
    }

    /// Reads an import map with the standard top-level `imports` member.
    /// Targets must resolve to files under the project root.
    pub fn read_import_map(&mut self, path: impl AsRef<Path>) -> Result<(), ModuleLoaderError> {
        let path = if path.as_ref().is_absolute() {
            path.as_ref().to_path_buf()
        } else {
            self.root.join(path.as_ref())
        };
        let path = self.secure_path(path)?;
        let bytes = read_bounded(&path, MAX_IMPORT_MAP_BYTES)?;
        let value: deno_core::serde_json::Value = deno_core::serde_json::from_slice(&bytes)
            .map_err(|error| {
                ModuleLoaderError::generic(format!(
                    "invalid import map {}: {error}",
                    path.display()
                ))
            })?;
        if value.get("scopes").is_some() {
            return Err(ModuleLoaderError::generic(
                "scoped import maps are not supported",
            ));
        }
        if value.get("integrity").is_some() {
            return Err(ModuleLoaderError::generic(
                "import map integrity metadata is not supported",
            ));
        }
        let imports = value
            .get("imports")
            .and_then(|value| value.as_object())
            .ok_or_else(|| {
                ModuleLoaderError::generic(format!(
                    "import map {} needs an imports object",
                    path.display()
                ))
            })?;
        self.imports = imports
            .iter()
            .map(|(key, value)| {
                let target = value.as_str().ok_or_else(|| {
                    ModuleLoaderError::generic(format!(
                        "import map target for {key:?} must be a string"
                    ))
                })?;
                if key.ends_with('/') && !target.ends_with('/') {
                    return Err(ModuleLoaderError::generic(format!(
                        "import map prefix {key:?} needs a target ending in /"
                    )));
                }
                Ok((key.clone(), target.to_owned()))
            })
            .collect::<Result<_, _>>()?;
        self.import_map_base = path.parent().unwrap_or(&self.root).to_path_buf();
        Ok(())
    }

    pub fn entry_specifier(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<ModuleSpecifier, ModuleLoaderError> {
        let path = if path.as_ref().is_absolute() {
            path.as_ref().to_path_buf()
        } else {
            self.root.join(path)
        };
        self.file_specifier(self.secure_path(path)?)
    }

    fn file_specifier(&self, path: PathBuf) -> Result<ModuleSpecifier, ModuleLoaderError> {
        ModuleSpecifier::from_file_path(&path).map_err(|_| {
            ModuleLoaderError::generic(format!("cannot form file URL for {}", path.display()))
        })
    }

    fn secure_path(&self, path: PathBuf) -> Result<PathBuf, ModuleLoaderError> {
        let path = self.probe(path)?.canonicalize().map_err(|error| {
            ModuleLoaderError::generic(format!("cannot resolve module: {error}"))
        })?;
        if !path.starts_with(&self.root) {
            return Err(ModuleLoaderError::generic(format!(
                "module is outside project root: {}",
                path.display()
            )));
        }
        if !path.is_file() {
            return Err(ModuleLoaderError::generic(format!(
                "module is not a file: {}",
                path.display()
            )));
        }
        Ok(path)
    }

    fn probe(&self, path: PathBuf) -> Result<PathBuf, ModuleLoaderError> {
        if path.is_file() {
            return Ok(path);
        }
        if path.extension().is_none() {
            for extension in PROBE_EXTENSIONS {
                let candidate = path.with_extension(extension);
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
            for extension in PROBE_EXTENSIONS {
                let candidate = path.join(format!("index.{extension}"));
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
        Err(ModuleLoaderError::generic(format!(
            "module not found: {}",
            path.display()
        )))
    }

    fn resolve_mapped(&self, specifier: &str) -> Option<String> {
        if let Some(target) = self.imports.get(specifier) {
            return Some(target.clone());
        }
        self.imports
            .iter()
            .filter(|(key, _)| key.ends_with('/') && specifier.starts_with(key.as_str()))
            .max_by_key(|(key, _)| key.len())
            .map(|(key, target)| format!("{target}{}", &specifier[key.len()..]))
    }

    fn resolve_three(&self, specifier: &str) -> Option<PathBuf> {
        let package = self.root.join("node_modules/three");
        match specifier {
            "three" => Some(package.join("build/three.webgpu.js")),
            "three/webgpu" => Some(package.join("build/three.webgpu.js")),
            "three/tsl" => Some(package.join("build/three.tsl.js")),
            "three/addons" => Some(package.join("examples/jsm/Addons.js")),
            _ if specifier.starts_with("three/addons/") => {
                Some(package.join("examples/jsm").join(&specifier[13..]))
            }
            _ if specifier.starts_with("three/src/") => {
                Some(package.join("src").join(&specifier[10..]))
            }
            _ if specifier.starts_with("three/examples/jsm/") => {
                Some(package.join("examples/jsm").join(&specifier[19..]))
            }
            _ if specifier.starts_with("three/examples/fonts/") => {
                Some(package.join("examples/fonts").join(&specifier[21..]))
            }
            _ => None,
        }
    }

    fn resolve_file(
        &self,
        specifier: &str,
        referrer: &str,
    ) -> Result<ModuleSpecifier, ModuleLoaderError> {
        if specifier.starts_with("ext:") {
            return ModuleSpecifier::parse(specifier)
                .map_err(|error| ModuleLoaderError::generic(error.to_string()));
        }
        let mapped = self.resolve_mapped(specifier);
        let target = mapped.as_deref().unwrap_or(specifier);
        let path = if target.starts_with("file:") {
            let url = ModuleSpecifier::parse(target)
                .map_err(|error| ModuleLoaderError::generic(error.to_string()))?;
            url.to_file_path()
                .map_err(|_| ModuleLoaderError::generic(format!("invalid file URL: {target}")))?
        } else if target.starts_with("./") || target.starts_with("../") {
            let base = if mapped.is_some() {
                self.import_map_base.clone()
            } else {
                let referrer = ModuleSpecifier::parse(referrer)
                    .map_err(|error| ModuleLoaderError::generic(error.to_string()))?;
                let path = referrer.to_file_path().map_err(|_| {
                    ModuleLoaderError::generic(format!("invalid referrer: {referrer}"))
                })?;
                path.parent()
                    .ok_or_else(|| ModuleLoaderError::generic("referrer has no parent directory"))?
                    .to_path_buf()
            };
            base.join(target)
        } else if target.starts_with('/') {
            self.root.join(target.trim_start_matches('/'))
        } else if target.contains(':') {
            return Err(ModuleLoaderError::generic(format!(
                "unsupported module scheme: {target}"
            )));
        } else if let Some(path) = self.resolve_three(target) {
            path
        } else {
            return Err(ModuleLoaderError::generic(format!(
                "unmapped bare import: {specifier}"
            )));
        };
        self.file_specifier(self.secure_path(path)?)
    }

    fn load_file(
        &self,
        specifier: &ModuleSpecifier,
        requested: &RequestedModuleType,
    ) -> Result<ModuleSource, ModuleLoaderError> {
        let path = specifier.to_file_path().map_err(|_| {
            ModuleLoaderError::generic(format!("unsupported module URL: {specifier}"))
        })?;
        let path = self.secure_path(path)?;
        if !self.loaded_modules.borrow().contains(specifier)
            && self.loaded_modules.borrow().len() >= MAX_MODULES
        {
            return Err(ModuleLoaderError::generic(format!(
                "module graph exceeds {MAX_MODULES} files"
            )));
        }
        let source = String::from_utf8(read_bounded(&path, MAX_MODULE_BYTES)?)
            .map_err(|error| ModuleLoaderError::generic(format!("{}: {error}", path.display())))?;
        if !matches!(
            requested,
            RequestedModuleType::None | RequestedModuleType::Json
        ) {
            return Err(ModuleLoaderError::generic(format!(
                "unsupported import attribute for {specifier}"
            )));
        }
        let media_type = MediaType::from_specifier(specifier);
        let is_json = media_type == MediaType::Json;
        if is_json && *requested != RequestedModuleType::Json {
            return Err(ModuleLoaderError::generic(format!(
                "JSON import needs a type=json attribute: {specifier}"
            )));
        }
        if !is_json && *requested == RequestedModuleType::Json {
            return Err(ModuleLoaderError::generic(format!(
                "expected JSON module: {specifier}"
            )));
        }
        let (code, module_type) = if is_json {
            (source, ModuleType::Json)
        } else if matches!(
            media_type,
            MediaType::TypeScript
                | MediaType::Tsx
                | MediaType::Mts
                | MediaType::Cts
                | MediaType::Jsx
        ) {
            let parsed = deno_ast::parse_module(ParseParams {
                specifier: specifier.clone(),
                text: Arc::from(source),
                media_type,
                capture_tokens: false,
                scope_analysis: false,
                maybe_syntax: None,
            })
            .map_err(|error| ModuleLoaderError::generic(format!("{specifier}: {error}")))?;
            let emitted = parsed
                .transpile(
                    &TranspileOptions::default(),
                    &TranspileModuleOptions::default(),
                    &EmitOptions {
                        source_map: SourceMapOption::Separate,
                        ..Default::default()
                    },
                )
                .map_err(|error| ModuleLoaderError::generic(format!("{specifier}: {error}")))?
                .into_source();
            if let Some(map) = emitted.source_map {
                self.source_maps
                    .borrow_mut()
                    .insert(specifier.to_string(), map.into_bytes());
            }
            (emitted.text, ModuleType::JavaScript)
        } else {
            (source, ModuleType::JavaScript)
        };
        self.loaded_modules.borrow_mut().insert(specifier.clone());
        Ok(ModuleSource::new(
            module_type,
            ModuleSourceCode::String(code.into()),
            specifier,
            None,
        ))
    }
}

impl ModuleLoader for ProjectModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> ModuleResolveResponse {
        self.resolve_file(specifier, referrer)
    }

    fn load(
        &self,
        specifier: &ModuleSpecifier,
        _referrer: Option<&ModuleLoadReferrer>,
        options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        ModuleLoadResponse::Sync(self.load_file(specifier, &options.requested_module_type))
    }

    fn get_source_map(&self, file_name: &str) -> Option<Cow<'_, [u8]>> {
        // `Cow::Owned` lets the `RefCell` borrow end before returning.
        self.source_maps
            .borrow()
            .get(file_name)
            .cloned()
            .map(Cow::Owned)
    }

    fn load_external_source_map(&self, source_map_url: &str) -> Option<Cow<'_, [u8]>> {
        let url = ModuleSpecifier::parse(source_map_url).ok()?;
        let path = self.secure_path(url.to_file_path().ok()?).ok()?;
        read_bounded(&path, MAX_MODULE_BYTES).ok().map(Cow::Owned)
    }

    fn source_map_source_exists(&self, source_url: &str) -> Option<bool> {
        let path = ModuleSpecifier::parse(source_url)
            .ok()?
            .to_file_path()
            .ok()?;
        Some(self.secure_path(path).is_ok())
    }

    fn get_source_mapped_source_line(&self, file_name: &str, line_number: usize) -> Option<String> {
        let path = ModuleSpecifier::parse(file_name)
            .ok()?
            .to_file_path()
            .ok()?;
        let path = self.secure_path(path).ok()?;
        String::from_utf8(read_bounded(&path, MAX_MODULE_BYTES).ok()?)
            .ok()?
            .lines()
            .nth(line_number)
            .map(str::to_owned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn project() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("peregrust-loader-{stamp}"));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn resolves_relative_ts_and_rejects_parent_escape() {
        let root = project();
        fs::write(root.join("main.ts"), "import './part';").unwrap();
        fs::write(root.join("part.ts"), "export const answer: number = 42;").unwrap();
        let outside = root.with_extension("outside.ts");
        fs::write(&outside, "export const escaped = true;").unwrap();
        let loader = ProjectModuleLoader::new(&root).unwrap();
        let main = loader.entry_specifier("main.ts").unwrap();
        let part = loader
            .resolve("./part", main.as_str(), ResolutionKind::Import)
            .unwrap();
        assert!(part.as_str().ends_with("part.ts"));
        let outside_url = ModuleSpecifier::from_file_path(&outside).unwrap();
        assert!(
            loader
                .resolve(outside_url.as_str(), main.as_str(), ResolutionKind::Import)
                .is_err()
        );
        let loaded = loader.load_file(&part, &RequestedModuleType::None).unwrap();
        assert_eq!(loaded.module_type, ModuleType::JavaScript);
        assert!(loader.get_source_map(part.as_str()).is_some());
        fs::remove_dir_all(root).unwrap();
        fs::remove_file(outside).unwrap();
    }

    #[test]
    fn import_map_resolves_bare_name_within_project() {
        let root = project();
        fs::create_dir(root.join("vendor")).unwrap();
        fs::write(
            root.join("vendor/math.ts"),
            "export const square = (n: number) => n * n;",
        )
        .unwrap();
        fs::write(root.join("main.ts"), "import 'math';").unwrap();
        fs::write(
            root.join("import-map.json"),
            r#"{"imports":{"math":"./vendor/math.ts"}}"#,
        )
        .unwrap();
        let mut loader = ProjectModuleLoader::new(&root).unwrap();
        loader.read_import_map("import-map.json").unwrap();
        let main = loader.entry_specifier("main.ts").unwrap();
        let resolved = loader
            .resolve("math", main.as_str(), ResolutionKind::Import)
            .unwrap();
        assert!(resolved.as_str().ends_with("vendor/math.ts"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn import_map_targets_are_relative_to_map_file() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        fs::create_dir(root.join("config")).unwrap();
        fs::create_dir(root.join("vendor")).unwrap();
        fs::write(root.join("main.ts"), "import 'math';").unwrap();
        fs::write(root.join("vendor/math.ts"), "export const value = 42;").unwrap();
        fs::write(
            root.join("config/import-map.json"),
            r#"{"imports":{"math":"../vendor/math.ts","lib/":"../vendor/"}}"#,
        )
        .unwrap();
        let mut loader = ProjectModuleLoader::new(root).unwrap();
        loader.read_import_map("config/import-map.json").unwrap();
        let main = loader.entry_specifier("main.ts").unwrap();
        let exact = loader
            .resolve("math", main.as_str(), ResolutionKind::Import)
            .unwrap();
        let prefix = loader
            .resolve("lib/math.ts", main.as_str(), ResolutionKind::Import)
            .unwrap();
        assert_eq!(exact, prefix);
        assert!(exact.as_str().ends_with("vendor/math.ts"));
    }

    #[test]
    fn bounded_read_rejects_large_file() {
        let project = tempfile::tempdir().unwrap();
        let file = project.path().join("data");
        fs::write(&file, b"123456").unwrap();
        assert!(read_bounded(&file, 5).is_err());
        assert_eq!(read_bounded(&file, 6).unwrap(), b"123456");
    }
}
