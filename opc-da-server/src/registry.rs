//! COM 注册工具——`/RegServer` 写、`/UnregServer` 清 HKCR。
//!
//! 写 `HKCR\CLSID\{CLSID}\LocalServer32` + `ProgID` + `VersionIndependentProgID`
//! + `Implemented Categories\{CATID}`（让 OPCEnum / client 按位宽枚举发现）
//! + `HKCR\AppID\{AppID}`（DCOM 远程激活所需）。
//!
//! 位宽（已知坑，见 CLAUDE.md）：双视图写入（`KEY_WOW64_64KEY` + `KEY_WOW64_32KEY`），
//! 让 32 位 OPCEnum 也能枚举到（否则 OPCEnum 读 WOW6432Node 32 位视图，单写 64 位视图找不到）。

use std::path::Path;
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CLASSES_ROOT, KEY_CREATE_SUB_KEY, KEY_SET_VALUE, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
    REG_OPTION_NON_VOLATILE, REG_SAM_FLAGS, REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW,
    RegSetValueExW,
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
    /// server 描述（ProgID/VersionIndependentProgID 顶层 default 值 + OPC\Vendor）。
    pub description: &'a str,
}

/// `/RegServer`：写 HKCR 注册项（需管理员权限）。
///
/// # Errors
/// 任何一次注册表写失败即返回 `Err`（已写的键不回滚——重跑 `/RegServer` 幂等）。
pub fn register(reg: &ServerRegistration<'_>) -> Result<()> {
    let clsid = clsid_string(&reg.clsid);
    let exe = reg.exe_path.to_string_lossy().into_owned();

    // CLSID 顶层 default = 服务器描述（COM 标准：CLSID 键默认值 = 人类可读类名）。
    // 标准 OPC client（Prosys/KEPware/Takebishi）按 Implemented Categories 收集候选 CLSID 后，
    // 读此 default 值作服务器显示名；为空则条目被过滤/不显示 → 表现为"枚举不到"。
    // （Rust opc-da-client 走 OPCEnum + ProgID 子键，不读此值，故能枚举到——掩盖了此缺陷。）
    set_reg_sz(
        HKEY_CLASSES_ROOT,
        &format!("CLSID\\{clsid}"),
        reg.description,
    )?;

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

    // ProgID 顶层（default=描述 + CLSID + CurVer + OPC 标记）
    set_reg_sz(HKEY_CLASSES_ROOT, reg.prog_id, reg.description)?;
    set_reg_sz(
        HKEY_CLASSES_ROOT,
        &format!("{}\\CLSID", reg.prog_id),
        &clsid,
    )?;
    set_reg_sz(
        HKEY_CLASSES_ROOT,
        &format!("{}\\CurVer", reg.prog_id),
        reg.version_independent_prog_id,
    )?;
    // ProgID\OPC 子键（OPC server 标记，部分 client 检查）
    set_reg_sz(HKEY_CLASSES_ROOT, &format!("{}\\OPC", reg.prog_id), "")?;

    // VersionIndependentProgID 顶层（default=描述 + CLSID + CurVer）
    set_reg_sz(
        HKEY_CLASSES_ROOT,
        reg.version_independent_prog_id,
        reg.description,
    )?;
    set_reg_sz(
        HKEY_CLASSES_ROOT,
        &format!("{}\\CLSID", reg.version_independent_prog_id),
        &clsid,
    )?;
    set_reg_sz(
        HKEY_CLASSES_ROOT,
        &format!("{}\\CurVer", reg.version_independent_prog_id),
        reg.prog_id,
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

/// `/UnregServer`：递归删 HKCR 注册项（64 + 32 位视图，幂等）。
///
/// 删 register 写的 3 个子树：`CLSID\{clsid}` / `{prog_id}` / `AppID\{appid}`。
/// 键不存在（未注册/已删）不算错误——幂等，可重复运行。
///
/// # Errors
/// 任何一次删除失败（非"键不存在"）即返回 `Err`。
pub fn unregister(reg: &ServerRegistration<'_>) -> Result<()> {
    let clsid = clsid_string(&reg.clsid);
    let appid = clsid_string(&reg.app_id);
    // 递归删 register 写的子树（64 + 32 视图）。
    delete_subtree(&format!("CLSID\\{clsid}"))?;
    delete_subtree(reg.prog_id)?;
    // register() 也写了 version-independent ProgID（default=描述 + CLSID/CurVer 子键）。
    // 漏删会让 /UnregServer 残留半死 ProgID（CLSID 键已删但 ProgID 仍指向它）。
    delete_subtree(reg.version_independent_prog_id)?;
    delete_subtree(&format!("AppID\\{appid}"))?;
    Ok(())
}

/// 递归删 HKCR 下 `{path}` 子键树（64 位视图 + 32 位 WOW6432Node 视图）。
fn delete_subtree(path: &str) -> Result<()> {
    delete_subtree_view(path, "")?;
    delete_subtree_view(path, "WOW6432Node\\")?;
    Ok(())
}

/// 递归删 HKCR\`{prefix}{path}` 子键树。键不存在（`ERROR_FILE_NOT_FOUND`）忽略（幂等）。
fn delete_subtree_view(path: &str, prefix: &str) -> Result<()> {
    let full = format!("{prefix}{path}");
    let wide = wide(&full);
    // SAFETY: RegDeleteTreeW 递归删 HKCR 下子键树（含所有子键）；返回 WIN32_ERROR。
    let err = unsafe { RegDeleteTreeW(HKEY_CLASSES_ROOT, PCWSTR(wide.as_ptr())) };
    // 键不存在（未注册/已删）忽略——unregister 幂等；其他失败返回。
    if err == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        err.ok()
    }
}

/// 写一个 HKCR `REG_SZ` 值到 **64 位 + 32 位视图**（创建所需子键）。
///
/// 双视图写入让 32 位 OPCEnum 也能枚举到自建 server（CLAUDE.md 位宽坑：32 位 client/OPCEnum
/// 读 WOW6432Node 视图，单写 64 位视图则枚举不到）。64 位 `LocalServer32` exe 被 32 位 client
/// 激活时，SCM 跨位宽启动 out-of-process（无问题）。
fn set_reg_sz(parent: HKEY, subkey: &str, value: &str) -> Result<()> {
    set_reg_sz_view(parent, subkey, value, KEY_WOW64_64KEY)?;
    set_reg_sz_view(parent, subkey, value, KEY_WOW64_32KEY)?;
    Ok(())
}

/// 写一个 HKCR `REG_SZ` 值到指定视图（64/32 位）。
fn set_reg_sz_view(parent: HKEY, subkey: &str, value: &str, view: REG_SAM_FLAGS) -> Result<()> {
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
            KEY_CREATE_SUB_KEY | KEY_SET_VALUE | view,
            None,
            &raw mut hkey,
            None,
        );
        err.ok()?;

        let value_wide = wide(value);
        let bytes =
            std::slice::from_raw_parts(value_wide.as_ptr().cast::<u8>(), value_wide.len() * 2);
        let err = RegSetValueExW(hkey, None, None, REG_SZ, Some(bytes));
        // 先 close 再 ?：RegSetValueExW 失败时也要关闭 hkey，否则句柄泄漏（虽进程退出 OS 回收，
        // 但违反"无泄漏错误路径"）。CoTaskMemFree/RegCloseKey 容忍已关闭句柄（此处仅关一次）。
        let _ = RegCloseKey(hkey);
        err.ok()?;
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
