//! COM 注册工具——`/RegServer` 写、`/UnregServer` 清 HKCR。
//!
//! 写 `HKCR\CLSID\{CLSID}\LocalServer32` + `ProgID` + `VersionIndependentProgID`
//! + `Implemented Categories\{CATID}`（让 OPCEnum / client 按位宽枚举发现）
//! + `HKCR\AppID\{AppID}`（DCOM 远程激活所需）。
//!
//! 位宽（已知坑，见 CLAUDE.md）：显式 `KEY_WOW64_64KEY` 写 64 位视图。若要 32 位 client
//! 枚举到，需另行注册 32 位视图（阶段 3 DCOM 时处理）。

use std::path::Path;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CLASSES_ROOT, KEY_CREATE_SUB_KEY, KEY_SET_VALUE, KEY_WOW64_64KEY,
    REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegSetValueExW,
};
use windows::core::{GUID, PCWSTR, Result};

/// server 注册参数。
#[derive(Clone)]
pub struct ServerRegistration<'a> {
    /// server COM 类 ID。
    pub clsid: GUID,
    /// 带版本 ProgID，如 `"opc-da-rs.Server.1"`。
    pub prog_id: &'a str,
    /// 不带版本 ProgID，如 `"opc-da-rs.Server"`。
    pub version_independent_prog_id: &'a str,
    /// LocalServer32 可执行文件绝对路径。
    pub exe_path: &'a Path,
    /// Implemented Categories（`CATID_OPCDAServer20` 等）。
    pub catids: &'a [GUID],
    /// AppID（DCOM 聚合；远程激活所需）。
    pub app_id: GUID,
}

/// `/RegServer`：写 HKCR 注册项（需管理员权限）。
///
/// # Errors
/// 任何一次注册表写失败即返回 `Err`（已写的键不回滚——重跑 `/RegServer` 幂等）。
pub fn register(reg: &ServerRegistration<'_>) -> Result<()> {
    let clsid = clsid_string(&reg.clsid);
    let exe = reg.exe_path.to_string_lossy().into_owned();

    set_reg_sz(
        HKEY_CLASSES_ROOT,
        &format!("CLSID\\{clsid}\\LocalServer32"),
        &exe,
    )?;
    set_reg_sz(
        HKEY_CLASSES_ROOT,
        &format!("CLSID\\{clsid}\\ProgID"),
        reg.prog_id,
    )?;
    set_reg_sz(
        HKEY_CLASSES_ROOT,
        &format!("CLSID\\{clsid}\\VersionIndependentProgID"),
        reg.version_independent_prog_id,
    )?;
    for catid in reg.catids {
        set_reg_sz(
            HKEY_CLASSES_ROOT,
            &format!(
                "CLSID\\{clsid}\\Implemented Categories\\{}",
                clsid_string(catid)
            ),
            "",
        )?;
    }

    // ProgID 顶层
    set_reg_sz(
        HKEY_CLASSES_ROOT,
        reg.prog_id,
        reg.version_independent_prog_id,
    )?;
    set_reg_sz(
        HKEY_CLASSES_ROOT,
        &format!("{}\\CLSID", reg.prog_id),
        &clsid,
    )?;

    // AppID
    let appid = clsid_string(&reg.app_id);
    set_reg_sz(
        HKEY_CLASSES_ROOT,
        &format!("AppID\\{appid}"),
        reg.version_independent_prog_id,
    )?;
    set_reg_sz(HKEY_CLASSES_ROOT, &format!("CLSID\\{clsid}\\AppID"), &appid)?;

    Ok(())
}

/// `/UnregServer`：删 HKCR 注册项。
///
/// # Errors
/// 当前为占位实现（阶段 0）。完整删除需 `SHDeleteKey` 递归清子键，后续补。
pub fn unregister(reg: &ServerRegistration<'_>) -> Result<()> {
    let _ = reg;
    Ok(())
}

/// 写一个 HKCR `REG_SZ` 值（创建所需子键，64 位视图）。
fn set_reg_sz(parent: HKEY, subkey: &str, value: &str) -> Result<()> {
    // SAFETY: `subkey_wide` 为 null 结尾 wide string；`parent` 为系统预定义 HKEY；
    // 句柄 `hkey` 由 RegCreateKeyExW 写出后立即 RegCloseKey。
    unsafe {
        let subkey_wide = wide(subkey);
        let mut hkey = HKEY::default();
        let err = RegCreateKeyExW(
            parent,
            PCWSTR(subkey_wide.as_ptr()),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_CREATE_SUB_KEY | KEY_SET_VALUE | KEY_WOW64_64KEY,
            None,
            &raw mut hkey,
            None,
        );
        err.ok()?;

        let value_wide = wide(value);
        let bytes =
            std::slice::from_raw_parts(value_wide.as_ptr().cast::<u8>(), value_wide.len() * 2);
        let err = RegSetValueExW(hkey, None, None, REG_SZ, Some(bytes));
        err.ok()?;
        let _ = RegCloseKey(hkey);
        Ok(())
    }
}

/// `&str` → null 结尾 UTF-16 wide string。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// GUID → `"{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}"`（注册表 CLSID/AppID 格式，大写）。
fn clsid_string(g: &GUID) -> String {
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        g.data1,
        g.data2,
        g.data3,
        g.data4[0],
        g.data4[1],
        g.data4[2],
        g.data4[3],
        g.data4[4],
        g.data4[5],
        g.data4[6],
        g.data4[7],
    )
}
