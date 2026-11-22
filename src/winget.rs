use quick_xml::{Reader, events::Event};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{self, BufReader, Read, Seek, Write};
use std::path::{Path, PathBuf};
use zip::ZipArchive;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

const DOWNLOAD_ROOT: &str = "https://github.com/microsoft/winget-cli/releases/download";
const BUNDLE: &str = "Microsoft.DesktopAppInstaller_8wekyb3d8bbwe.msixbundle";
const DEPENDENCIES: &str = "DesktopAppInstaller_Dependencies.zip";

#[derive(Deserialize)]
struct ReleaseMetadata {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<AssetMetadata>,
}

#[derive(Deserialize)]
struct AssetMetadata {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
    size: u64,
}

pub struct Release {
    pub tag: String,
    pub bundle: Asset,
    pub dependencies: Asset,
    pub license: Asset,
}

pub struct Asset {
    pub name: String,
    pub url: String,
    size: u64,
    sha256: String,
}

pub fn parse_release(json: &[u8]) -> Result<Release> {
    let release: ReleaseMetadata = serde_json::from_slice(json)?;
    if release.draft || release.prerelease || !safe_filename(&release.tag_name) {
        return Err("GitHub did not return a stable WinGet release.".into());
    }

    let select = |matches: fn(&str) -> bool, label: &str| -> Result<Asset> {
        let mut candidates = release.assets.iter().filter(|asset| matches(&asset.name));
        let Some(asset) = candidates.next() else {
            return Err(format!("The release is missing its {label}.").into());
        };
        if candidates.next().is_some() || !safe_filename(&asset.name) || asset.size == 0 {
            return Err(format!("The release has invalid or ambiguous {label} metadata.").into());
        }
        let url = format!("{DOWNLOAD_ROOT}/{}/{}", release.tag_name, asset.name);
        if asset.browser_download_url != url {
            return Err(format!("The {label} download does not belong to this release.").into());
        }
        let sha256 = asset
            .digest
            .as_deref()
            .and_then(|digest| digest.strip_prefix("sha256:"))
            .filter(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or_else(|| format!("The release does not publish a valid SHA-256 for {label}."))?
            .to_ascii_lowercase();
        Ok(Asset {
            name: asset.name.clone(),
            url,
            size: asset.size,
            sha256,
        })
    };

    Ok(Release {
        bundle: select(|name| name == BUNDLE, "App Installer bundle")?,
        dependencies: select(|name| name == DEPENDENCIES, "dependency archive")?,
        license: select(|name| name.ends_with("_License1.xml"), "license")?,
        tag: release.tag_name,
    })
}

fn safe_filename(name: &str) -> bool {
    !name.is_empty()
        && !name.ends_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

pub fn copy_verified(asset: &Asset, mut input: impl Read, mut output: impl Write) -> Result<()> {
    let mut hasher = Sha256::new();
    let mut remaining = asset.size;
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        remaining = remaining
            .checked_sub(count as u64)
            .ok_or_else(|| format!("{} exceeds its published size.", asset.name))?;
        hasher.update(&buffer[..count]);
        output.write_all(&buffer[..count])?;
    }
    if remaining != 0 || format!("{:x}", hasher.finalize()) != asset.sha256 {
        return Err(format!(
            "{} did not match its published SHA-256 or size.",
            asset.name
        )
        .into());
    }
    output.flush()?;
    Ok(())
}

pub fn bundle_version(input: impl Read + Seek) -> Result<String> {
    let mut bundle = ZipArchive::new(input)?;
    let manifest = bundle.by_name("AppxMetadata/AppxBundleManifest.xml")?;
    let mut xml = Reader::from_reader(BufReader::new(manifest.take(1024 * 1024)));
    let mut buffer = Vec::new();
    loop {
        match xml.read_event_into(&mut buffer)? {
            Event::Start(element) | Event::Empty(element)
                if element.local_name().as_ref() == "Identity" =>
            {
                for attribute in element.attributes() {
                    let attribute = attribute?;
                    if attribute.key.as_ref() == "Version" {
                        let version = attribute.value;
                        if version.split('.').count() == 4
                            && version
                                .split('.')
                                .all(|number| number.parse::<u16>().is_ok())
                        {
                            return Ok(version.into_owned());
                        }
                    }
                }
                break;
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    Err("The App Installer bundle does not contain a valid package version.".into())
}

pub fn extract_dependencies(input: impl Read + Seek, destination: &Path) -> Result<Vec<PathBuf>> {
    let mut archive = ZipArchive::new(input)?;
    let mut packages = Vec::new();
    let mut architectures = [false; 2];

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.enclosed_name().is_none() || entry.is_symlink() {
            return Err("The dependency archive contains an unsafe entry.".into());
        }
        if entry.is_dir() {
            continue;
        }

        let name = entry.name().replace('\\', "/");
        let Some((architecture, filename)) = name.split_once('/') else {
            continue;
        };
        let architecture = architecture.to_ascii_lowercase();
        let architecture_index = match architecture.as_str() {
            "x86" => 0,
            "x64" => 1,
            _ => continue,
        };
        let extension = filename
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase());
        if !matches!(extension.as_deref(), Some("appx" | "msix")) {
            continue;
        }
        if !safe_filename(filename) || entry.size() == 0 {
            return Err("The dependency archive contains an invalid package entry.".into());
        }

        let folder = destination.join(architecture);
        fs::create_dir_all(&folder)?;
        let path = folder.join(filename);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        io::copy(&mut entry, &mut file)?;
        file.flush()?;
        packages.push(path);
        architectures[architecture_index] = true;
    }

    if architectures != [true, true] {
        return Err("The dependency archive must contain both x86 and x64 packages.".into());
    }
    packages.sort();
    Ok(packages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;
    use zip::{ZipWriter, write::SimpleFileOptions};

    fn metadata() -> serde_json::Value {
        let tag = "v1.99.1";
        let assets: Vec<_> = [
            BUNDLE,
            DEPENDENCIES,
            "release-id_License1.xml",
            "readme.txt",
        ]
        .map(|name| {
            json!({
                "name": name,
                "browser_download_url": format!("{DOWNLOAD_ROOT}/{tag}/{name}"),
                "digest": format!("sha256:{:x}", Sha256::digest(b"test")),
                "size": 4,
            })
        })
        .into();
        json!({"tag_name": tag, "draft": false, "prerelease": false, "assets": assets})
    }

    #[test]
    fn selects_assets_from_the_same_stable_release() {
        let mut metadata = metadata();
        let release = parse_release(&serde_json::to_vec(&metadata).unwrap()).unwrap();
        assert_eq!(release.tag, "v1.99.1");
        assert_eq!(release.bundle.name, BUNDLE);
        assert_eq!(release.dependencies.name, DEPENDENCIES);
        assert!(
            release
                .license
                .url
                .ends_with("v1.99.1/release-id_License1.xml")
        );

        metadata["assets"][2]["browser_download_url"] = json!("https://example.com/license.xml");
        assert!(parse_release(&serde_json::to_vec(&metadata).unwrap()).is_err());
        metadata["assets"][2]["browser_download_url"] = json!(release.license.url);
        metadata["prerelease"] = json!(true);
        assert!(parse_release(&serde_json::to_vec(&metadata).unwrap()).is_err());
    }

    #[test]
    fn verifies_download_before_using_it() {
        let release = parse_release(&serde_json::to_vec(&metadata()).unwrap()).unwrap();
        let mut file = Vec::new();
        copy_verified(&release.bundle, &b"test"[..], &mut file).unwrap();
        assert_eq!(file, b"test");
        assert!(copy_verified(&release.bundle, &b"fail"[..], io::sink()).is_err());
        assert!(copy_verified(&release.bundle, &b"tes"[..], io::sink()).is_err());
    }

    fn archive(names: &[&str]) -> Cursor<Vec<u8>> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for name in names {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(b"package").unwrap();
        }
        zip.finish().unwrap()
    }

    #[test]
    fn reads_the_app_installer_version_from_the_bundle_manifest() {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(
            "AppxMetadata/AppxBundleManifest.xml",
            SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(br#"<Bundle xmlns="http://schemas.microsoft.com/appx/2013/bundle"><Identity Version="2026.917.151.0"/><Packages><Package Version="1.29.380.0"/></Packages></Bundle>"#)
            .unwrap();
        assert_eq!(
            bundle_version(zip.finish().unwrap()).unwrap(),
            "2026.917.151.0"
        );
    }

    #[test]
    fn extracts_both_architectures_and_ignores_arm_packages() {
        let directory = tempfile::tempdir().unwrap();
        let names = [
            "x64/Framework_3.0.msix",
            "x86/Framework_3.0.appx",
            "arm64/Other.appx",
        ];
        let extracted = extract_dependencies(archive(&names), directory.path()).unwrap();
        assert_eq!(extracted.len(), 2);
        assert_eq!(fs::read(&extracted[0]).unwrap(), b"package");
        assert!(!directory.path().join("arm64").exists());
    }

    #[test]
    fn rejects_traversal_and_archives_missing_an_architecture() {
        let directory = tempfile::tempdir().unwrap();
        assert!(extract_dependencies(archive(&["../outside.appx"]), directory.path()).is_err());
        assert!(extract_dependencies(archive(&["x64/Framework.appx"]), directory.path()).is_err());
    }
}
