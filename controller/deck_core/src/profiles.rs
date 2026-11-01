//! Robot profiles from `pair` bundles (the file pairing method). docs/controller.md § Configuration.
//!
//! ~/.config/steamdeck-remote-joy/          (0700)
//! ├── config.toml                          active_robot = "<robot id hex>"
//! └── robots/<robot id hex>/               (0700)
//!     ├── profile.toml                     name, trust, endpoints, deck name/CN
//!     ├── ca.pem, client.pem
//!     └── client.key                       (0600)

use std::fs;
use std::io::Read;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Profile {
    pub name: String,
    /// "robot": trusts this robot's own CA ("fleet:<name>" post-MVP).
    pub trust: String,
    pub endpoints: Vec<String>,
    pub deck_name: String,
    pub deck_cn: String,
    /// SHA-256 of ca.pem (DER), hex. Also the directory name. Checked against Info.robot_id.
    pub id: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Config {
    active_robot: Option<String>,
}

/// A profile plus where its certificates are.
#[derive(Clone, Debug)]
pub struct Installed {
    pub profile: Profile,
    pub dir: PathBuf,
}

impl Installed {
    pub fn ca(&self) -> PathBuf {
        self.dir.join("ca.pem")
    }
    pub fn cert(&self) -> PathBuf {
        self.dir.join("client.pem")
    }
    pub fn key(&self) -> PathBuf {
        self.dir.join("client.key")
    }
}

pub struct Store {
    root: PathBuf,
}

/// The bundle's robot.toml, as written by `pair`.
#[derive(Deserialize)]
struct BundleToml {
    robot: BundleRobot,
    deck: BundleDeck,
}
#[derive(Deserialize)]
struct BundleRobot {
    name: String,
    id: String,
    endpoints: Vec<String>,
}
#[derive(Deserialize)]
struct BundleDeck {
    name: String,
    cn: String,
}

pub fn default_root() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    base.join("steamdeck-remote-joy")
}

/// SHA-256 of the first certificate in a PEM file, hex.
pub fn pem_cert_sha256(pem: &str) -> Result<String> {
    let begin = "-----BEGIN CERTIFICATE-----";
    let end = "-----END CERTIFICATE-----";
    let start = pem.find(begin).ok_or_else(|| anyhow!("no certificate in PEM"))? + begin.len();
    let stop = pem[start..].find(end).ok_or_else(|| anyhow!("unterminated certificate"))? + start;
    let b64: String = pem[start..stop].chars().filter(|c| !c.is_whitespace()).collect();
    let der = base64::engine::general_purpose::STANDARD.decode(b64).context("certificate base64")?;
    Ok(hex(&Sha256::digest(der)))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn mkdir_private(p: &Path) -> Result<()> {
    fs::create_dir_all(p).with_context(|| format!("create {}", p.display()))?;
    fs::set_permissions(p, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn write_file(p: &Path, data: &[u8], mode: u32) -> Result<()> {
    let tmp = p.with_extension("tmp");
    {
        use std::io::Write;
        let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(&tmp)?;
        f.write_all(data)?;
    }
    fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
    fs::rename(&tmp, p)?;
    Ok(())
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Store { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn robots(&self) -> PathBuf {
        self.root.join("robots")
    }

    fn read_config(&self) -> Config {
        fs::read_to_string(self.root.join("config.toml")).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default()
    }

    fn write_config(&self, c: &Config) -> Result<()> {
        mkdir_private(&self.root)?;
        write_file(&self.root.join("config.toml"), toml::to_string(c)?.as_bytes(), 0o600)
    }

    /// Installs a `pair` bundle (tar of ca.pem, client.pem, client.key, robot.toml) and makes it
    /// the active robot. Checks that robot.toml's id really is the bundled CA's fingerprint.
    pub fn add_bundle(&self, bundle: &Path) -> Result<Installed> {
        let mut files = std::collections::HashMap::new();
        let mut ar = tar::Archive::new(fs::File::open(bundle).with_context(|| format!("open {}", bundle.display()))?);
        for entry in ar.entries()? {
            let mut e = entry?;
            let name = e.path()?.to_string_lossy().into_owned();
            if !["ca.pem", "client.pem", "client.key", "robot.toml"].contains(&name.as_str()) {
                bail!("unexpected file {name:?} in bundle");
            }
            let mut data = Vec::new();
            e.read_to_end(&mut data)?;
            files.insert(name, data);
        }
        let get = |n: &str| files.get(n).ok_or_else(|| anyhow!("bundle is missing {n}"));
        let meta: BundleToml = toml::from_str(std::str::from_utf8(get("robot.toml")?)?).context("robot.toml")?;
        let ca = std::str::from_utf8(get("ca.pem")?)?;
        let fp = pem_cert_sha256(ca)?;
        if fp != meta.robot.id.to_lowercase() {
            bail!("bundle robot.toml says id {} but ca.pem is {fp}: not a bundle from `pair`, or tampered", meta.robot.id);
        }
        if meta.robot.endpoints.is_empty() {
            bail!("bundle has no endpoints");
        }
        let profile = Profile {
            name: meta.robot.name,
            trust: "robot".into(),
            endpoints: meta.robot.endpoints,
            deck_name: meta.deck.name,
            deck_cn: meta.deck.cn,
            id: fp.clone(),
        };
        let dir = self.robots().join(&fp);
        mkdir_private(&self.root)?;
        mkdir_private(&self.robots())?;
        mkdir_private(&dir)?;
        write_file(&dir.join("ca.pem"), get("ca.pem")?, 0o644)?;
        write_file(&dir.join("client.pem"), get("client.pem")?, 0o644)?;
        write_file(&dir.join("client.key"), get("client.key")?, 0o600)?;
        write_file(&dir.join("profile.toml"), toml::to_string(&profile)?.as_bytes(), 0o644)?;
        self.write_config(&Config { active_robot: Some(fp) })?;
        Ok(Installed { profile, dir })
    }

    pub fn list(&self) -> Vec<Installed> {
        let mut out = Vec::new();
        if let Ok(rd) = fs::read_dir(self.robots()) {
            for e in rd.flatten() {
                let dir = e.path();
                if let Some(p) = fs::read_to_string(dir.join("profile.toml")).ok().and_then(|s| toml::from_str::<Profile>(&s).ok()) {
                    out.push(Installed { profile: p, dir });
                }
            }
        }
        out.sort_by(|a, b| a.profile.name.cmp(&b.profile.name));
        out
    }

    pub fn active(&self) -> Result<Installed> {
        let id = self
            .read_config()
            .active_robot
            .ok_or_else(|| anyhow!("no robot paired: run `steamdeck-remote-joy robot add --bundle <file>`"))?;
        self.list()
            .into_iter()
            .find(|i| i.profile.id == id)
            .ok_or_else(|| anyhow!("active robot {id} has no profile in {}", self.robots().display()))
    }

    /// Removes a robot by name (or id prefix). Clears the active robot if it was this one.
    pub fn forget(&self, which: &str) -> Result<Profile> {
        let hits: Vec<_> = self.list().into_iter().filter(|i| i.profile.name == which || i.profile.id.starts_with(which)).collect();
        match hits.as_slice() {
            [] => bail!("no robot {which:?}"),
            [one] => {
                fs::remove_dir_all(&one.dir)?;
                let mut c = self.read_config();
                if c.active_robot.as_deref() == Some(one.profile.id.as_str()) {
                    c.active_robot = None;
                    self.write_config(&c)?;
                }
                Ok(one.profile.clone())
            }
            _ => bail!("{which:?} matches {} robots; use a longer id prefix", hits.len()),
        }
    }
}
