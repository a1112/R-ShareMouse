use anyhow::{anyhow, Context, Result};
use rshare_core::{validate_asset_relative_path, HardwareAssetKind, HardwareAssetManifest};
use serde::Serialize;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

const MAX_PACKAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy)]
struct ArchiveLimits {
    entries: usize,
    file_bytes: u64,
    total_bytes: u64,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            entries: 1024,
            file_bytes: 64 * 1024 * 1024,
            total_bytes: 256 * 1024 * 1024,
        }
    }
}

fn valid_storage_id(id: &str) -> bool {
    validate_asset_relative_path(id)
        && id == id.trim()
        && !id.starts_with('.')
        && !id.ends_with('.')
        && !id.contains(['/', '\\'])
        && !id.chars().any(char::is_control)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledHardwareAsset {
    pub id: String,
    pub name: String,
    pub kind: HardwareAssetKind,
    pub manifest_path: String,
    pub folder_path: String,
    pub manifest: HardwareAssetManifest,
}

pub fn hardware_asset_root(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("hardware")
}

pub fn import_hardware_asset_package(
    app_data_dir: &Path,
    package_bytes: &[u8],
) -> Result<InstalledHardwareAsset> {
    if package_bytes.len() > MAX_PACKAGE_BYTES {
        return Err(anyhow!("hardware asset package exceeds 64 MiB"));
    }
    let root = hardware_asset_root(app_data_dir);
    fs::create_dir_all(&root)?;
    let staging = root.join(format!(".staging-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&staging)?;

    let result = extract_and_validate(&staging, package_bytes).and_then(|manifest| {
        if !valid_storage_id(&manifest.id) {
            return Err(anyhow!("hardware asset id is not safe for storage"));
        }
        let target = root.join(&manifest.id);
        if target.exists() {
            fs::remove_dir_all(&target)?;
        }
        fs::rename(&staging, &target)?;
        Ok(installed_asset_from_manifest(target, manifest))
    });

    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }

    result
}

pub fn list_installed_hardware_assets(app_data_dir: &Path) -> Result<Vec<InstalledHardwareAsset>> {
    let root = hardware_asset_root(app_data_dir);
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut assets = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let folder = entry.path();
        if !folder.is_dir() {
            continue;
        }
        let manifest_path = folder.join("manifest.json");
        if !manifest_path.is_file() {
            continue;
        }
        let manifest: HardwareAssetManifest = serde_json::from_slice(&fs::read(&manifest_path)?)?;
        manifest.validate()?;
        assets.push(installed_asset_from_manifest(folder, manifest));
    }
    assets.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(assets)
}

pub fn export_hardware_asset_package(app_data_dir: &Path, asset_id: &str) -> Result<Vec<u8>> {
    if !valid_storage_id(asset_id) {
        return Err(anyhow!("hardware asset id is not safe for storage"));
    }

    let asset_dir = hardware_asset_root(app_data_dir).join(asset_id);
    let manifest_path = asset_dir.join("manifest.json");
    let manifest: HardwareAssetManifest = serde_json::from_slice(
        &fs::read(&manifest_path).context("hardware asset folder missing manifest.json")?,
    )?;
    manifest.validate()?;
    for relative in manifest.referenced_paths() {
        if !asset_dir.join(relative).is_file() {
            return Err(anyhow!(
                "hardware asset references missing file: {relative}"
            ));
        }
    }

    let cursor = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    let options = zip::write::SimpleFileOptions::default();
    for path in files_under(&asset_dir)? {
        let relative = path.strip_prefix(&asset_dir)?;
        let name = relative.to_string_lossy().replace('\\', "/");
        writer.start_file(name, options)?;
        writer.write_all(&fs::read(path)?)?;
    }
    Ok(writer.finish()?.into_inner())
}

fn extract_and_validate(target: &Path, package_bytes: &[u8]) -> Result<HardwareAssetManifest> {
    extract_with_limits(target, package_bytes, ArchiveLimits::default())
}

fn extract_with_limits(
    target: &Path,
    package_bytes: &[u8],
    limits: ArchiveLimits,
) -> Result<HardwareAssetManifest> {
    let mut archive = zip::ZipArchive::new(Cursor::new(package_bytes))?;
    if archive.len() > limits.entries {
        return Err(anyhow!("hardware asset archive contains too many entries"));
    }
    let mut total_bytes = 0_u64;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index)?;
        if file.is_symlink() {
            return Err(anyhow!("hardware asset archive contains a symbolic link"));
        }
        let enclosed = file
            .enclosed_name()
            .ok_or_else(|| anyhow!("archive contains unsafe path"))?
            .to_path_buf();
        let output = target.join(enclosed);
        if file.is_dir() {
            fs::create_dir_all(&output)?;
            continue;
        }
        let mut limit = limits.file_bytes.min(limits.total_bytes - total_bytes);
        if output == target.join("manifest.json") {
            limit = limit.min(MAX_MANIFEST_BYTES);
        }
        if file.size() > limit {
            return Err(anyhow!(
                "hardware asset archive exceeds extraction size limits"
            ));
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        // Bound actual decompressed bytes too: ZIP metadata is untrusted.
        // Stream to disk so a compressed asset cannot allocate its full size.
        let mut output = fs::File::create(output)?;
        let written = std::io::copy(&mut (&mut file).take(limit + 1), &mut output)?;
        if written > limit {
            return Err(anyhow!(
                "hardware asset archive exceeds extraction size limits"
            ));
        }
        total_bytes += written;
    }

    let manifest_path = target.join("manifest.json");
    let manifest: HardwareAssetManifest = serde_json::from_slice(
        &fs::read(&manifest_path).context("hardware asset package missing manifest.json")?,
    )?;
    manifest.validate()?;
    for relative in manifest.referenced_paths() {
        if !target.join(relative).is_file() {
            return Err(anyhow!(
                "hardware asset references missing file: {relative}"
            ));
        }
    }
    Ok(manifest)
}

fn installed_asset_from_manifest(
    folder: PathBuf,
    manifest: HardwareAssetManifest,
) -> InstalledHardwareAsset {
    InstalledHardwareAsset {
        id: manifest.id.clone(),
        name: manifest.name.clone(),
        kind: manifest.kind.clone(),
        manifest_path: folder.join("manifest.json").to_string_lossy().into_owned(),
        folder_path: folder.to_string_lossy().into_owned(),
        manifest,
    }
}

fn files_under(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_files(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_files(path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, files)?;
        } else if path.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn sample_package_bytes() -> Vec<u8> {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("manifest.json", options).unwrap();
        writer
            .write_all(
                br#"{
            "schema_version": 1,
            "id": "user.keyboard.sample",
            "name": "Sample",
            "kind": "keyboard",
            "base_size": { "width": 100, "height": 50 },
            "layers": [{ "id": "base", "role": "base", "src": "base.png" }],
            "regions": []
        }"#,
            )
            .unwrap();
        writer.start_file("base.png", options).unwrap();
        writer.write_all(b"png-bytes").unwrap();
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn extraction_rejects_file_total_and_entry_limits() {
        let bytes = sample_package_bytes();
        let archive = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let largest = archive.decompressed_size().unwrap() as u64 - b"png-bytes".len() as u64;
        for limits in [
            ArchiveLimits {
                entries: 1,
                file_bytes: u64::MAX - 1,
                total_bytes: u64::MAX - 1,
            },
            ArchiveLimits {
                entries: 10,
                file_bytes: largest - 1,
                total_bytes: 4096,
            },
            ArchiveLimits {
                entries: 10,
                file_bytes: 4096,
                total_bytes: largest,
            },
        ] {
            let temp = tempfile::tempdir().unwrap();
            assert!(extract_with_limits(temp.path(), &bytes, limits).is_err());
        }
        let temp = tempfile::tempdir().unwrap();
        assert!(extract_with_limits(
            temp.path(),
            &bytes,
            ArchiveLimits {
                entries: 2,
                file_bytes: largest,
                total_bytes: largest + b"png-bytes".len() as u64,
            }
        )
        .is_ok());
    }

    #[test]
    fn storage_ids_cannot_target_nested_or_staging_directories() {
        for id in [
            "parent/child",
            "parent\\child",
            ".staging-private",
            "..",
            "asset.",
            " asset",
            "asset\n",
        ] {
            assert!(!valid_storage_id(id), "unsafe storage id: {id:?}");
        }
        assert!(valid_storage_id("user.keyboard.sample"));
    }

    #[test]
    fn rejected_import_preserves_existing_asset_and_removes_staging() {
        let temp = tempfile::tempdir().unwrap();
        let installed =
            import_hardware_asset_package(temp.path(), &sample_package_bytes()).unwrap();
        let original = fs::read(&installed.manifest_path).unwrap();
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("manifest.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&original).unwrap();
        let invalid = writer.finish().unwrap().into_inner();
        assert!(import_hardware_asset_package(temp.path(), &invalid).is_err());
        assert_eq!(fs::read(&installed.manifest_path).unwrap(), original);
        assert_eq!(
            fs::read_dir(hardware_asset_root(temp.path()))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn import_zip_installs_unpacked_asset_folder() {
        let temp = tempfile::tempdir().unwrap();
        let installed =
            import_hardware_asset_package(temp.path(), &sample_package_bytes()).unwrap();

        assert_eq!(installed.id, "user.keyboard.sample");
        assert!(temp
            .path()
            .join("hardware")
            .join("user.keyboard.sample")
            .join("manifest.json")
            .exists());
        assert!(temp
            .path()
            .join("hardware")
            .join("user.keyboard.sample")
            .join("base.png")
            .exists());
    }

    #[test]
    fn import_zip_rejects_path_traversal() {
        let temp = tempfile::tempdir().unwrap();
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("../escape.txt", options).unwrap();
        writer.write_all(b"bad").unwrap();
        let bytes = writer.finish().unwrap().into_inner();

        assert!(import_hardware_asset_package(temp.path(), &bytes).is_err());
    }

    #[test]
    fn list_installed_assets_reads_unpacked_manifest_folders() {
        let temp = tempfile::tempdir().unwrap();
        import_hardware_asset_package(temp.path(), &sample_package_bytes()).unwrap();

        let installed = list_installed_hardware_assets(temp.path()).unwrap();

        assert_eq!(installed.len(), 1);
        assert_eq!(installed[0].id, "user.keyboard.sample");
        assert_eq!(installed[0].name, "Sample");
    }

    #[test]
    fn listed_assets_include_manifest_payload_for_frontend_rendering() {
        let temp = tempfile::tempdir().unwrap();
        import_hardware_asset_package(temp.path(), &sample_package_bytes()).unwrap();

        let installed = list_installed_hardware_assets(temp.path()).unwrap();

        assert_eq!(installed[0].manifest.id, "user.keyboard.sample");
        assert_eq!(
            installed[0].manifest.layers[0].src.as_deref(),
            Some("base.png")
        );
    }

    #[test]
    fn export_asset_package_includes_manifest_and_referenced_files() {
        let temp = tempfile::tempdir().unwrap();
        import_hardware_asset_package(temp.path(), &sample_package_bytes()).unwrap();

        let bytes = export_hardware_asset_package(temp.path(), "user.keyboard.sample").unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();

        let mut manifest = String::new();
        archive
            .by_name("manifest.json")
            .unwrap()
            .read_to_string(&mut manifest)
            .unwrap();
        assert!(manifest.contains("user.keyboard.sample"));
        assert!(archive.by_name("base.png").is_ok());
    }
}
