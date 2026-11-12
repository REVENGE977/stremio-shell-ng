use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    time::Duration,
};

use anyhow::{anyhow, Context};
use semver::{Version, VersionReq};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use url::Url;

#[derive(Debug, Clone)]
pub struct Update {
    /// The new version that we update to
    pub version: Version,
    pub file: PathBuf,
}

#[derive(Debug)]
pub struct Updater {
    pub current_version: Version,
    pub next_version: VersionReq,
    pub endpoint: Url,
    pub force_update: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateResponse {
    version_desc: Url,
    version: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileItem {
    pub url: Url,
    pub checksum: String,
    os: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Descriptor {
    version: String,
    files: Vec<FileItem>,
}

fn validated_installer_filename(url: &Url) -> Result<String, anyhow::Error> {
    if url.scheme() != "https" {
        return Err(anyhow!("Update installer URL must use HTTPS"));
    }
    let file_name = Path::new(url.path())
        .file_name()
        .context("Invalid file name")?
        .to_str()
        .context("The path is not valid UTF-8")?
        .to_string();
    let mut components = Path::new(&file_name).components();
    if !matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(_)), None)
    ) {
        return Err(anyhow!("Invalid installer file name"));
    }
    let ext = Path::new(&file_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    if !matches!(ext.to_ascii_lowercase().as_str(), "exe" | "msi") {
        return Err(anyhow!("Unsupported installer file extension: {ext}"));
    }
    Ok(file_name)
}

fn updates_dir() -> Result<PathBuf, anyhow::Error> {
    let dir = crate::stremio_app::enhanced::base_dir().join("updates");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

impl Updater {
    pub fn new(current_version: Version, updater_endpoint: &Url, force_update: bool) -> Self {
        Self {
            next_version: VersionReq::parse(&format!(">{current_version}"))
                .expect("Version is type-safe"),
            current_version,
            endpoint: updater_endpoint.clone(),
            force_update,
        }
    }

    /// Fetches the latest update from the update server.
    pub fn autoupdate(&self) -> Result<Option<Update>, anyhow::Error> {
        println!("Fetching updates for v{}", self.current_version);
        println!("Using updater endpoint {}", self.endpoint);

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent("StremioEnhanced/5.0")
            .build()?;

        let update_response = client
            .get(self.endpoint.clone())
            .send()?
            .error_for_status()?
            .json::<UpdateResponse>()?;
        let update_descriptor = client
            .get(update_response.version_desc)
            .send()?
            .error_for_status()?
            .json::<Descriptor>()?;

        if update_response.version != update_descriptor.version {
            return Err(anyhow!("Mismatched update versions"));
        }
        let installer = update_descriptor
            .files
            .iter()
            .find(|file_item| file_item.os == std::env::consts::OS)
            .context("No update for this OS")?;
        let version = Version::parse(update_descriptor.version.as_str())?;
        if !self.force_update && !self.next_version.matches(&version) {
            return Err(anyhow!(
                "No new releases found that match the requirement of `{}`",
                self.next_version
            ));
        }
        println!("Found update v{version}");

        let file_name = validated_installer_filename(&installer.url)?;
        let dest = updates_dir()?.join(file_name);

        let mut installer_response = client
            .get(installer.url.clone())
            .send()?
            .error_for_status()?;
        let size = installer_response.content_length();
        let mut downloaded: u64 = 0;
        let mut sha256 = Sha256::new();

        println!("Downloading {} to {}", installer.url, dest.display());

        let mut chunk = [0u8; 8192];
        let mut file = fs::File::create(&dest)?;
        loop {
            let chunk_size = installer_response.read(&mut chunk)?;
            if chunk_size == 0 {
                break;
            }
            sha256.update(&chunk[..chunk_size]);
            file.write_all(&chunk[..chunk_size])?;
            if let Some(size) = size {
                downloaded += chunk_size as u64;
                print!("\rProgress: {}%", downloaded * 100 / size);
            } else {
                print!(".");
            }
            std::io::stdout().flush().ok();
        }
        println!();
        let actual_sha256 = format!("{:x}", sha256.finalize());
        if !actual_sha256.eq_ignore_ascii_case(&installer.checksum) {
            fs::remove_file(&dest)?;
            return Err(anyhow!("Checksum verification failed"));
        }
        println!("Checksum verified.");

        Ok(Some(Update {
            version,
            file: dest,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_installer_filename_and_scheme() {
        let valid = Url::parse("https://example.com/releases/StremioEnhancedSetup.exe").unwrap();
        assert_eq!(
            validated_installer_filename(&valid).unwrap(),
            "StremioEnhancedSetup.exe"
        );

        let insecure = Url::parse("http://example.com/releases/StremioEnhancedSetup.exe").unwrap();
        assert!(validated_installer_filename(&insecure).is_err());

        let wrong_ext = Url::parse("https://example.com/releases/script.bat").unwrap();
        assert!(validated_installer_filename(&wrong_ext).is_err());
    }
}
