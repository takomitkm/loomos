//! 组件包格式（ADR-011 v1.1 §8.2）——`.wasm` 正本 + 目标 AOT + 校验和的打包契约。
//!
//! 布局：
//! ```text
//! component-package/
//! ├── manifest.toml          # 身份/接口依赖/Host ABI 版本/入口
//! ├── module.wasm            # 正本 wasm32-wasip1 产物
//! ├── artifacts/<target>/module.aot(.compat.json)   # wamrc 产物 + 兼容键 sidecar
//! ├── interfaces/host-abi-v1.json                   # Host ABI 接口声明（由表生成）
//! └── checksums.txt          # 全部文件 sha256（sha256sum 格式："<hash>  <path>"）
//! ```
//!
//! 纪律：
//! - 包内**不得**含秘密/可变配置/私有凭据（ADR-012 §决定 5）；
//! - 校验和覆盖**每一个**文件（含 checksums.txt 自身除外——自引用不可能）；
//! - 读取时全量验签（tamper 即拒绝）；每个工件可溯源到源 commit/工具链/旗标
//!   （compat.json + manifest.source 字段）。

use crate::error::{WamrError, WamrResult};
use loomos_runtime::hash::sha256_hex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 包清单（manifest.toml）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageManifest {
    /// 组件 ID（与 ComponentManifest.id 一致）。
    pub id: String,
    /// 组件版本。
    pub version: u32,
    /// 组件针对的 Host ABI 版本。
    pub host_abi_version: u32,
    /// 入口 wasm 文件（相对包根）。
    pub entry: String,
    /// 目标 AOT 工件：<target> → 相对路径。
    #[serde(default)]
    pub artifacts: BTreeMap<String, String>,
    /// 溯源：源仓库 commit（可空=未知，如实记录）。
    #[serde(default)]
    pub source_commit: Option<String>,
    /// 溯源：构建工具链指纹（如 "rust 1.99.1 wasm32-wasip1"）。
    #[serde(default)]
    pub toolchain: Option<String>,
}

/// 已校验的组件包。
#[derive(Debug)]
pub struct ComponentPackage {
    pub root: PathBuf,
    pub manifest: PackageManifest,
}

/// 约定布局的文件清单（相对包根）。
const ENTRY_MANIFEST: &str = "manifest.toml";
const ENTRY_CHECKSUMS: &str = "checksums.txt";
const ENTRY_INTERFACES: &str = "interfaces/host-abi-v1.json";

impl ComponentPackage {
    /// 打包：写入 manifest/module/接口声明 + 全量校验和。
    ///
    /// `aot_artifacts`：<target> → (aot 文件, compat.json 内容)。
    pub fn build(
        dir: &Path,
        manifest: &PackageManifest,
        module_wasm: &[u8],
        aot_artifacts: &BTreeMap<String, (Vec<u8>, Vec<u8>)>,
    ) -> WamrResult<()> {
        std::fs::create_dir_all(dir.join("artifacts")).map_err(io_err(dir))?;
        // manifest.toml
        let toml_str = toml::to_string_pretty(manifest)
            .map_err(|e| manifest_err(format!("manifest 序列化失败: {e}")))?;
        std::fs::write(dir.join(ENTRY_MANIFEST), toml_str).map_err(io_err(dir))?;
        // module.wasm
        let entry_path = dir.join(&manifest.entry);
        std::fs::write(&entry_path, module_wasm).map_err(io_err(&entry_path))?;
        // artifacts/<target>/module.aot + compat.json
        for (target, (aot, compat)) in aot_artifacts {
            let tdir = dir.join("artifacts").join(target);
            std::fs::create_dir_all(&tdir).map_err(io_err(&tdir))?;
            std::fs::write(tdir.join("module.aot"), aot).map_err(io_err(&tdir))?;
            std::fs::write(tdir.join("module.aot.compat.json"), compat).map_err(io_err(&tdir))?;
        }
        // interfaces/host-abi-v1.json
        std::fs::create_dir_all(dir.join("interfaces")).map_err(io_err(dir))?;
        let iface_json = interfaces_json()?;
        std::fs::write(dir.join(ENTRY_INTERFACES), iface_json).map_err(io_err(dir))?;
        // checksums.txt（最后写：覆盖其余全部文件）
        let files = collect_files(dir)?;
        let mut lines = Vec::new();
        for rel in &files {
            let abs = dir.join(rel);
            let bytes = std::fs::read(&abs).map_err(io_err(&abs))?;
            lines.push(format!("{}  {}", sha256_hex(&bytes), rel.display()));
        }
        std::fs::write(dir.join(ENTRY_CHECKSUMS), lines.join("\n") + "\n").map_err(io_err(dir))?;
        Ok(())
    }

    /// 读取并全量校验（manifest 解析 → checksums 覆盖完整性 → 逐文件验签）。
    pub fn open(dir: &Path) -> WamrResult<Self> {
        let manifest_path = dir.join(ENTRY_MANIFEST);
        let raw = std::fs::read_to_string(&manifest_path).map_err(io_err(&manifest_path))?;
        let manifest: PackageManifest =
            toml::from_str(&raw).map_err(|e| manifest_err(format!("manifest 解析失败: {e}")))?;
        if manifest.host_abi_version != crate::host_abi::HOST_ABI_VERSION {
            return Err(WamrError::AbiVersionMismatch {
                guest: manifest.host_abi_version,
                host: crate::host_abi::HOST_ABI_VERSION,
            });
        }
        // 实际文件集合（相对路径）
        let files = collect_files(dir)?;
        // checksums.txt 解析
        let cs_path = dir.join(ENTRY_CHECKSUMS);
        let cs_raw = std::fs::read_to_string(&cs_path).map_err(io_err(&cs_path))?;
        let mut expected: BTreeMap<String, String> = BTreeMap::new();
        for line in cs_raw.lines().filter(|l| !l.trim().is_empty()) {
            let (hash, path) = split_checksum_line(line)?;
            expected.insert(path, hash);
        }
        // 覆盖完整性：checksums 必须列尽所有文件（除 checksums.txt 自身）
        let must_list: Vec<String> = files
            .iter()
            .map(|p| p.display().to_string())
            .filter(|p| p != ENTRY_CHECKSUMS)
            .collect();
        for f in &must_list {
            if !expected.contains_key(f) {
                return Err(manifest_err(format!("checksums.txt 缺少条目: {f}")));
            }
        }
        for f in expected.keys() {
            if !must_list.contains(f) {
                return Err(manifest_err(format!("checksums.txt 多余条目: {f}")));
            }
        }
        // 逐文件验签（tamper 即拒绝）
        for (rel, want) in &expected {
            let abs = dir.join(rel);
            let bytes = std::fs::read(&abs).map_err(io_err(&abs))?;
            let got = sha256_hex(&bytes);
            if got != *want {
                return Err(manifest_err(format!(
                    "校验和不符: {rel}（want {want} got {got}）"
                )));
            }
        }
        Ok(Self {
            root: dir.to_path_buf(),
            manifest,
        })
    }

    /// 入口 wasm 字节。
    pub fn module_wasm(&self) -> WamrResult<Vec<u8>> {
        let p = self.root.join(&self.manifest.entry);
        std::fs::read(&p).map_err(io_err(&p))
    }

    /// 指定目标的 AOT 字节（不存在 → NoSuchModule 语义复用：目标缺工件）。
    pub fn artifact_aot(&self, target: &str) -> WamrResult<Vec<u8>> {
        let rel = self
            .manifest
            .artifacts
            .get(target)
            .ok_or_else(|| WamrError::NoSuchModule(format!("目标 {target} 无 AOT 工件")))?;
        let p = self.root.join(rel);
        std::fs::read(&p).map_err(io_err(&p))
    }
}

/// Host ABI 接口声明 JSON（由 HOST_ABI_V1 表生成，供 Guest SDK 对齐）。
pub fn interfaces_json() -> WamrResult<Vec<u8>> {
    let ifaces: Vec<_> = crate::host_abi::HOST_ABI_V1
        .iter()
        .map(|(name, sig, ret, doc)| {
            serde_json::json!({
                "name": name,
                "signature": sig,
                "returns": ret,
                "doc": doc,
            })
        })
        .collect();
    let body = serde_json::json!({
        "abi_version": crate::host_abi::HOST_ABI_VERSION,
        "namespace": crate::host_abi::HOST_NAMESPACE,
        "payload_limit": crate::host_abi::PAYLOAD_LIMIT,
        "interfaces": ifaces,
    });
    serde_json::to_vec_pretty(&body).map_err(|e| manifest_err(format!("接口声明序列化失败: {e}")))
}

fn collect_files(dir: &Path) -> WamrResult<Vec<PathBuf>> {
    let mut out = Vec::new();
    fn walk(dir: &Path, base: &Path, out: &mut Vec<PathBuf>) -> WamrResult<()> {
        let entries = std::fs::read_dir(dir).map_err(io_err(dir))?;
        for e in entries {
            let e = e.map_err(io_err(dir))?;
            let p = e.path();
            if p.is_dir() {
                walk(&p, base, out)?;
            } else {
                let rel = p
                    .strip_prefix(base)
                    .map_err(|_| manifest_err("相对路径计算失败".into()))?;
                out.push(rel.to_path_buf());
            }
        }
        Ok(())
    }
    walk(dir, dir, &mut out)?;
    out.sort();
    Ok(out)
}

fn split_checksum_line(line: &str) -> WamrResult<(String, String)> {
    // sha256sum 格式："hash  path"（两个空格分隔）
    let mut parts = line.splitn(2, "  ");
    let hash = parts
        .next()
        .filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| manifest_err(format!("校验和行格式非法: {line}")))?;
    let path = parts
        .next()
        .filter(|p| !p.is_empty())
        .ok_or_else(|| manifest_err(format!("校验和行缺路径: {line}")))?;
    Ok((hash.to_string(), path.to_string()))
}

fn io_err(ctx: impl AsRef<Path>) -> impl Fn(std::io::Error) -> WamrError {
    let ctx = ctx.as_ref().display().to_string();
    move |e| WamrError::InvalidFormat {
        path: ctx.clone(),
        reason: format!("IO: {e}"),
    }
}

fn manifest_err(reason: String) -> WamrError {
    WamrError::InvalidFormat {
        path: "<package>".into(),
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WASM_BYTES: &[u8] = b"\0asm\x01\x00\x00\x00fake-module-payload";
    const AOT_BYTES: &[u8] = b"\0aot\x06\x00\x00\x00fake-aot-payload";

    fn manifest() -> PackageManifest {
        PackageManifest {
            id: "nlu".into(),
            version: 1,
            host_abi_version: crate::host_abi::HOST_ABI_VERSION,
            entry: "module.wasm".into(),
            artifacts: BTreeMap::from([("aarch64".into(), "artifacts/aarch64/module.aot".into())]),
            source_commit: Some("06f6c8c".into()),
            toolchain: Some("rust 1.99.1 wasm32-wasip1".into()),
        }
    }

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("loomos-pkg-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn build_open_roundtrip_with_artifacts() {
        let dir = tmp_dir("round");
        let aots = BTreeMap::from([("aarch64".into(), (AOT_BYTES.to_vec(), b"{}".to_vec()))]);
        ComponentPackage::build(&dir, &manifest(), WASM_BYTES, &aots).unwrap();
        let pkg = ComponentPackage::open(&dir).unwrap();
        assert_eq!(pkg.manifest.id, "nlu");
        assert_eq!(pkg.module_wasm().unwrap(), WASM_BYTES);
        assert_eq!(pkg.artifact_aot("aarch64").unwrap(), AOT_BYTES);
        assert!(pkg.artifact_aot("x86_64").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tampered_module_rejected() {
        let dir = tmp_dir("tamper");
        ComponentPackage::build(&dir, &manifest(), WASM_BYTES, &BTreeMap::new()).unwrap();
        // 篡改 wasm（校验和不再匹配）
        std::fs::write(dir.join("module.wasm"), b"\0asmTAMPERED").unwrap();
        let err = ComponentPackage::open(&dir).unwrap_err();
        assert!(
            matches!(err, WamrError::InvalidFormat { ref reason, .. } if reason.contains("校验和不符"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checksum_entry_missing_rejected() {
        let dir = tmp_dir("missing");
        ComponentPackage::build(&dir, &manifest(), WASM_BYTES, &BTreeMap::new()).unwrap();
        // 额外塞一个文件（checksums 未覆盖）→ 覆盖完整性拒绝
        std::fs::write(dir.join("extra.bin"), b"x").unwrap();
        let err = ComponentPackage::open(&dir).unwrap_err();
        assert!(
            matches!(err, WamrError::InvalidFormat { ref reason, .. } if reason.contains("缺少条目"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checksum_extra_entry_rejected() {
        let dir = tmp_dir("extra-entry");
        ComponentPackage::build(&dir, &manifest(), WASM_BYTES, &BTreeMap::new()).unwrap();
        // 删除 interfaces 文件但 checksums 还列着 → 多余条目拒绝
        std::fs::remove_file(dir.join(ENTRY_INTERFACES)).unwrap();
        let err = ComponentPackage::open(&dir).unwrap_err();
        assert!(
            matches!(err, WamrError::InvalidFormat { ref reason, .. } if reason.contains("多余条目"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn abi_version_mismatch_rejected() {
        let dir = tmp_dir("abi");
        let mut m = manifest();
        m.host_abi_version = 99;
        ComponentPackage::build(&dir, &m, WASM_BYTES, &BTreeMap::new()).unwrap();
        let err = ComponentPackage::open(&dir).unwrap_err();
        assert!(matches!(
            err,
            WamrError::AbiVersionMismatch { guest: 99, .. }
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn interfaces_json_lists_all_v1_ifaces() {
        let body = interfaces_json().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let names: Vec<&str> = v["interfaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(names.len(), crate::host_abi::HOST_ABI_V1.len());
        assert!(names.contains(&"loomos.log_v1"));
        assert!(names.contains(&"loomos.infer_submit_v1"));
        assert_eq!(v["abi_version"], crate::host_abi::HOST_ABI_VERSION);
    }

    #[test]
    fn checksum_line_format_is_sha256sum_compatible() {
        assert!(split_checksum_line(&format!("{}  module.wasm", "a".repeat(64))).is_ok());
        assert!(split_checksum_line("short  f").is_err());
        assert!(split_checksum_line(&format!("{} module.wasm", "a".repeat(64))).is_err()); // 单空格
    }
}
