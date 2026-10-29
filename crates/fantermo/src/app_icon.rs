//! 実行ファイルに埋め込んだアプリのアイコン（`fantermo.rc`）を、DPI に合う大きさで読む。

use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, WPARAM},
        UI::{
            Controls::LoadIconWithScaleDown,
            HiDpi::GetSystemMetricsForDpi,
            WindowsAndMessaging::{
                DestroyIcon, HICON, ICON_BIG, ICON_SMALL, SM_CXICON, SM_CXSMICON, SM_CYICON,
                SM_CYSMICON, SYSTEM_METRICS_INDEX, SendMessageW, WM_SETICON,
            },
        },
    },
    core::PCWSTR,
};

/// アイコンのリソースの番号。`fantermo.rc` の `IDI_FANTERMO` と揃える。
const ICON_RESOURCE_ID: u16 = 1;

/// 窓のアイコンの二つの大きさ。
#[derive(Clone, Copy, Debug, Default)]
pub struct Icons {
    /// Alt+Tab とタスクバーに出す大きいアイコン。
    pub big: Option<HICON>,
    /// タイトルバーとタスクバーのボタンに出す小さいアイコン。
    pub small: Option<HICON>,
}

/// DPI が `dpi` のときの大きいアイコンと小さいアイコンを読む。読めなかったほうは `None`。
/// 返したアイコンは呼び出し側が持ち、要らなくなったら `DestroyIcon` で捨てる。
pub fn load(instance: HINSTANCE, dpi: u32) -> Icons {
    Icons {
        big: load_one(instance, dpi, SM_CXICON, SM_CYICON),
        small: load_one(instance, dpi, SM_CXSMICON, SM_CYSMICON),
    }
}

fn load_one(
    instance: HINSTANCE,
    dpi: u32,
    width_metric: SYSTEM_METRICS_INDEX,
    height_metric: SYSTEM_METRICS_INDEX,
) -> Option<HICON> {
    // SAFETY: 大きさを問い合わせるだけ。
    let (width, height) = unsafe {
        (
            GetSystemMetricsForDpi(width_metric, dpi),
            GetSystemMetricsForDpi(height_metric, dpi),
        )
    };
    // 番号で指すリソースの名前は、下位 16 ビットに番号を入れたポインタ（MAKEINTRESOURCE）。
    let name = PCWSTR(usize::from(ICON_RESOURCE_ID) as *const u16);
    // SAFETY: instance はこのモジュールで、名前は番号。ICO の中から大きさに合うものを選び、なければ
    // 大きいものを縮めて作る。
    match unsafe { LoadIconWithScaleDown(Some(instance), name, width, height) } {
        Ok(icon) => Some(icon),
        Err(e) => {
            tracing::warn!("failed to load the {width}x{height} application icon: {e}");
            None
        }
    }
}

/// 窓 `hwnd` のアイコンを DPI が `dpi` の大きさにする。前に `set` で付けたアイコンは捨てる
/// （窓のクラスのアイコンは捨てない）。
pub fn set(hwnd: HWND, instance: HINSTANCE, dpi: u32) {
    let icons = load(instance, dpi);
    for (kind, icon) in [(ICON_BIG, icons.big), (ICON_SMALL, icons.small)] {
        let Some(icon) = icon else {
            continue;
        };
        // SAFETY: hwnd は有効な窓。WM_SETICON は前に WM_SETICON で付けたアイコンを返す。
        let previous = unsafe {
            SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(kind as usize)),
                Some(LPARAM(icon.0 as isize)),
            )
        };
        if previous.0 == 0 {
            continue;
        }
        // SAFETY: 前の WM_SETICON でこの関数が付けたアイコンで、もう窓は使わない。
        if let Err(e) = unsafe { DestroyIcon(HICON(previous.0 as *mut _)) } {
            tracing::warn!("failed to destroy the previous window icon: {e}");
        }
    }
}
