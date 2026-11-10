//! Fetching, pinning and verifying the server's contract bundle (server D-186, D-192).

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct Lock {
    pub bundle_format: u64,
    pub contract_version: u64,
    pub manifest_sha256: String,
    pub server_version: String,
}

impl Lock {
    pub fn read(root: &Path) -> Result<Lock> {
        let v = crate::read_json(&root.join("Contract/contract.lock"))?;
        Ok(Lock {
            bundle_format: v["bundleFormat"].as_u64().context("bundleFormat")?,
            contract_version: v["contractVersion"].as_u64().context("contractVersion")?,
            manifest_sha256: v["manifestSha256"].as_str().context("manifestSha256")?.to_string(),
            server_version: v["serverVersion"].as_str().context("serverVersion")?.to_string(),
        })
    }

    fn write(&self, root: &Path) -> Result<()> {
        let body = format!(
            "{{\n  \"bundleFormat\": {},\n  \"contractVersion\": {},\n  \"manifestSha256\": \"{}\",\n  \"serverVersion\": \"{}\"\n}}\n",
            self.bundle_format, self.contract_version, self.manifest_sha256, self.server_version
        );
        std::fs::write(root.join("Contract/contract.lock"), body)?;
        Ok(())
    }
}

pub fn bundle_dir(root: &Path, server_version: &str) -> PathBuf {
    root.join("Contract/bundle").join(format!("purlay-contract-v{server_version}"))
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

/// Checks every file against `MANIFEST.json`, refuses extra files, and checks the
/// manifest's own digest against `Contract/contract.lock`. Returns the bundle directory.
pub fn verify(root: &Path) -> Result<PathBuf> {
    let lock = Lock::read(root)?;
    let dir = bundle_dir(root, &lock.server_version);
    if !dir.exists() {
        bail!(
            "bundle {} is missing; run `cargo xtask contract fetch v{}`",
            dir.display(),
            lock.server_version
        );
    }
    let manifest_path = dir.join("MANIFEST.json");
    let digest = sha256_file(&manifest_path)?;
    if digest != lock.manifest_sha256 {
        bail!("MANIFEST.json digest {digest} does not match the lock {}", lock.manifest_sha256);
    }
    let manifest = crate::read_json(&manifest_path)?;
    if manifest["format"].as_u64() != Some(lock.bundle_format) {
        bail!("bundle format differs from the lock");
    }
    if manifest["contractVersion"].as_u64() != Some(lock.contract_version) {
        bail!("contract version differs from the lock");
    }
    let mut listed = std::collections::BTreeSet::new();
    for f in manifest["files"].as_array().context("files")? {
        let rel = f["path"].as_str().context("path")?;
        let want = f["sha256"].as_str().context("sha256")?;
        let got = sha256_file(&dir.join(rel))?;
        if got != want {
            bail!("{rel}: digest {got} differs from the manifest's {want}");
        }
        listed.insert(rel.replace('\\', "/"));
    }
    listed.insert("MANIFEST.json".into());
    for entry in walk(&dir)? {
        let rel = entry.strip_prefix(&dir)?.to_string_lossy().replace('\\', "/");
        if !listed.contains(&rel) {
            bail!("{rel} is in the bundle but not in its manifest");
        }
    }
    println!(
        "contract bundle verified: server {} contract {} ({} files)",
        lock.server_version,
        lock.contract_version,
        listed.len()
    );
    Ok(dir)
}

fn walk(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_dir() {
            out.extend(walk(&p)?);
        } else {
            out.push(p);
        }
    }
    Ok(out)
}

/// Downloads `purlay-contract-<tag>.tar.gz` from the server's GitHub release, checks
/// its `.sha256`, unpacks it and pins it.
pub fn fetch(root: &Path, tag: Option<&str>) -> Result<()> {
    let tag = match tag {
        Some(t) => t.to_string(),
        None => format!("v{}", Lock::read(root)?.server_version),
    };
    let download = root.join("Contract/download");
    std::fs::create_dir_all(&download)?;
    let pattern = format!("purlay-contract-{tag}.tar.gz*");
    let status = Command::new("gh")
        .args(["release", "download", &tag, "--repo", "doancan/purlay", "--clobber", "-p", &pattern, "-D"])
        .arg(&download)
        .status()
        .context("run gh (GitHub CLI)")?;
    if !status.success() {
        bail!("gh release download failed");
    }
    let archive = download.join(format!("purlay-contract-{tag}.tar.gz"));
    let sums = std::fs::read_to_string(download.join(format!("purlay-contract-{tag}.tar.gz.sha256")))?;
    let want = sums.split_whitespace().next().context("empty .sha256")?;
    let got = sha256_file(&archive)?;
    if got != want {
        bail!("archive digest {got} differs from the release's {want}");
    }
    let bundle = root.join("Contract/bundle");
    std::fs::create_dir_all(&bundle)?;
    let status = Command::new("tar").arg("-xzf").arg(&archive).arg("-C").arg(&bundle).status()?;
    if !status.success() {
        bail!("tar failed");
    }
    pin(root, &bundle_dir(root, tag.trim_start_matches('v')))
}

pub fn use_dir(root: &Path, dir: &Path) -> Result<()> {
    let manifest = crate::read_json(&dir.join("MANIFEST.json"))?;
    let version = manifest["serverVersion"].as_str().context("serverVersion")?;
    let target = bundle_dir(root, version);
    if target.exists() {
        std::fs::remove_dir_all(&target)?;
    }
    copy_dir(dir, &target)?;
    pin(root, &target)
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()))?;
        } else {
            std::fs::copy(&p, to.join(e.file_name()))?;
        }
    }
    Ok(())
}

fn pin(root: &Path, dir: &Path) -> Result<()> {
    let manifest = crate::read_json(&dir.join("MANIFEST.json"))?;
    let lock = Lock {
        bundle_format: manifest["format"].as_u64().context("format")?,
        contract_version: manifest["contractVersion"].as_u64().context("contractVersion")?,
        manifest_sha256: sha256_file(&dir.join("MANIFEST.json"))?,
        server_version: manifest["serverVersion"].as_str().context("serverVersion")?.to_string(),
    };
    lock.write(root)?;
    verify(root)?;
    println!("pinned server {} (manifest {})", lock.server_version, lock.manifest_sha256);
    Ok(())
}
