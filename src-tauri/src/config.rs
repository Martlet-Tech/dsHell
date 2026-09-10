//! `%USERPROFILE%\.dshell\config.json`
//!
//! 只记用户显式指定的路径。**任何读取失败都静默回落到默认值**——
//! 配置文件坏了不能变成新的启动故障点。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub node: Option<String>,
    #[serde(default)]
    pub npm: Option<String>,
    #[serde(default)]
    pub dsh: Option<String>,
    /// `npm prefix -g` 的结果，装完 dsh 后用来定位 `dsh.cmd`
    #[serde(default)]
    pub npm_prefix: Option<String>,
}

impl Config {
    pub fn dir() -> PathBuf {
        let home = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".dshell")
    }

    pub fn path() -> PathBuf {
        Self::dir().join("config.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// 先写临时文件再 rename，避免写一半断电留下半个 JSON。
    pub fn save(&self) -> std::io::Result<()> {
        let dir = Self::dir();
        std::fs::create_dir_all(&dir)?;
        let tmp = dir.join("config.json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, Self::path())
    }

    pub fn set(&mut self, id: &str, path: &Path) {
        let v = path.display().to_string();
        match id {
            "node" => self.node = Some(v),
            "npm" => self.npm = Some(v),
            "dsh" => self.dsh = Some(v),
            _ => {}
        }
    }

    pub fn get(&self, id: &str) -> Option<PathBuf> {
        let s = match id {
            "node" => self.node.as_ref(),
            "npm" => self.npm.as_ref(),
            "dsh" => self.dsh.as_ref(),
            _ => None,
        }?;
        let p = PathBuf::from(s);
        // 失效路径直接当作"没有指定"，回退 PATH 探测
        p.is_file().then_some(p)
    }

    fn npm_global_dir() -> Option<PathBuf> {
        let appdata = std::env::var("APPDATA").ok()?;
        Some(PathBuf::from(appdata).join("npm"))
    }

    /// 需要前置进子进程 PATH 的目录（用户自定义路径时必须生效，否则 dsh 起来的
    /// node 仍然是旧的那个，或者根本找不到）。
    pub fn prelude_dirs(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = Vec::new();
        for p in [self.node.as_ref(), self.npm.as_ref(), self.dsh.as_ref()]
            .into_iter()
            .flatten()
        {
            if let Some(d) = Path::new(p).parent() {
                v.push(d.to_path_buf());
            }
        }
        if let Some(p) = &self.npm_prefix {
            v.push(PathBuf::from(p));
        }
        if let Some(d) = Self::npm_global_dir() {
            v.push(d);
        }
        v.sort();
        v.dedup();
        v
    }

    /// 给子进程用的完整 PATH。
    pub fn child_path_env(&self) -> String {
        let mut parts: Vec<String> = self
            .prelude_dirs()
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        parts.push(std::env::var("PATH").unwrap_or_default());
        parts.join(";")
    }
}
