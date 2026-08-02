//! `IEnumString` 实现——browse 枚举 item id（[`StringEnum`]）。
//!
//! [`StringEnum`] 持 `String` 快照 + 游标，`Next` 分配 wide string（`CoTaskMemAlloc`）交 client
//!（client `CoTaskMemFree` 每个）。游标只前进；`Reset` 归零；`Clone` 复制快照+游标。
//! 参考 `opc-da-client` 的 `MockEnumString`（`iterator.rs`）模式，server 侧自实现。

// `#[implement]` 展开的 COM 胶水触发若干 pedantic lints；同 group.rs 模式 allow。
#![allow(
    clippy::ref_as_ptr,
    clippy::inline_always,
    clippy::undocumented_unsafe_blocks,
    clippy::not_unsafe_ptr_arg_deref
)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use windows::Win32::Foundation::{E_OUTOFMEMORY, E_POINTER, S_FALSE, S_OK};
use windows::Win32::System::Com::{CoTaskMemAlloc, IEnumString, IEnumString_Impl};
use windows::core::{HRESULT, PWSTR, Result, implement};

/// 取锁；mutex poison 时返回 guard（不 panic）。同 group.rs/server.rs 模式。
fn locked<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `IEnumString` 实现——`String` 快照枚举器（browse item id 用）。
///
/// 构造时取一份 `String` 快照（`browse` 时 `DataSource::namespace().leaves()` 的拷贝），
/// 之后只读；`Next` 分配 wide string 写入调用方缓冲，所有权交 client。
#[implement(IEnumString)]
pub struct StringEnum {
    items: Vec<String>,
    cursor: Mutex<usize>,
}

impl StringEnum {
    /// 新建枚举器（持 items 快照，游标 0）。
    pub(crate) fn new(items: Vec<String>) -> Self {
        Self {
            items,
            cursor: Mutex::new(0),
        }
    }
}

impl IEnumString_Impl for StringEnum_Impl {
    #[allow(clippy::cast_possible_truncation)] // avail/i 均 ≤ celt(u32)，as u32 安全
    fn Next(&self, celt: u32, rgelt: *mut PWSTR, pceltfetched: *mut u32) -> HRESULT {
        if celt == 0 || rgelt.is_null() {
            return E_POINTER;
        }
        let mut cur = locked(&self.cursor);
        let avail = (self.items.len().saturating_sub(*cur)).min(celt as usize);
        // SAFETY: 调用方承诺 rgelt 容 celt 个 PWSTR；avail <= celt。每个 CoTaskMemAlloc wide
        //（含 null 终止），copy_nonoverlapping 拷贝；PWSTR 所有权交 client（client CoTaskMemFree）。
        unsafe {
            for i in 0..avail {
                let s = &self.items[*cur + i];
                let w: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
                let ptr = CoTaskMemAlloc(w.len() * 2).cast::<u16>();
                if ptr.is_null() {
                    // OOM：游标前进已分配数，返回已 fetched（少于请求）。
                    *cur += i;
                    drop(cur);
                    if !pceltfetched.is_null() {
                        *pceltfetched = i as u32;
                    }
                    return E_OUTOFMEMORY;
                }
                std::ptr::copy_nonoverlapping(w.as_ptr(), ptr, w.len());
                *rgelt.add(i) = PWSTR(ptr);
            }
            *cur += avail;
        }
        drop(cur);
        if !pceltfetched.is_null() {
            // SAFETY: pceltfetched 非空时为调用方 out 值。
            unsafe {
                *pceltfetched = avail as u32;
            }
        }
        // 全部满足 S_OK；不足（到尾）S_FALSE——两者皆成功 HRESULT。
        if avail as u32 == celt { S_OK } else { S_FALSE }
    }

    fn Skip(&self, celt: u32) -> HRESULT {
        let mut cur = locked(&self.cursor);
        let remaining = self.items.len().saturating_sub(*cur);
        *cur += (celt as usize).min(remaining);
        S_OK
    }

    fn Reset(&self) -> Result<()> {
        *locked(&self.cursor) = 0;
        Ok(())
    }

    fn Clone(&self) -> Result<IEnumString> {
        let cur = *locked(&self.cursor);
        Ok(StringEnum {
            items: self.items.clone(),
            cursor: Mutex::new(cur),
        }
        .into())
    }
}
