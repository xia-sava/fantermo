//! 実行ファイルに埋め込んだアプリのアイコン（`fantermo.rc`）を、DPI に合う大きさで読む。管理者として動いている
//! ウィンドウのアイコンには、右下に UAC の盾を重ねる。

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

use crate::{gdi_image, image_decoder::Decoder};

/// アイコンのリソースの番号。`fantermo.rc` の `IDI_FANTERMO` と揃える。
const ICON_RESOURCE_ID: u16 = 1;

/// ウィンドウのアイコンの二つの大きさ。
#[derive(Clone, Copy, Debug, Default)]
pub struct Icons {
    /// Alt+Tab とタスクバーに出す大きいアイコン。
    pub big: Option<HICON>,
    /// タイトルバーとタスクバーのボタンに出す小さいアイコン。
    pub small: Option<HICON>,
}

/// DPI が `dpi` のときの大きいアイコンと小さいアイコンを読む。`elevated` なら右下に UAC の盾を重ねる（重ね
/// られなければ警告して盾なしのアイコン）。読めなかったほうは `None`。返したアイコンは呼び出し側が持ち、
/// 要らなくなったら `DestroyIcon` で捨てる。盾を重ねるときは WIC を使うので、COM を初期化したスレッドで呼ぶ。
pub fn load(instance: HINSTANCE, dpi: u32, elevated: bool) -> Icons {
    let decoder = if elevated {
        Decoder::new()
            .inspect_err(|e| tracing::warn!("failed to create a decoder for the shield: {e}"))
            .ok()
    } else {
        None
    };
    let load = |width_metric, height_metric| {
        let (icon, width) = load_one(instance, dpi, width_metric, height_metric)?;
        Some(match &decoder {
            Some(decoder) => with_shield(decoder, icon, width),
            None => icon,
        })
    };
    Icons {
        big: load(SM_CXICON, SM_CYICON),
        small: load(SM_CXSMICON, SM_CYSMICON),
    }
}

/// 一辺 `side` のアイコン `icon` の右下に盾を重ねたアイコン。重ねられたら `icon` は捨てる。重ねられなければ
/// 警告して `icon` を返す。
fn with_shield(decoder: &Decoder, icon: HICON, side: u32) -> HICON {
    let marked = decoder
        .decode_icon_handle(icon, side)
        .and_then(|mut image| {
            decoder.overlay_shield(&mut image)?;
            gdi_image::icon(&image)
        });
    match marked {
        Ok(marked) => {
            // SAFETY: load_one が読んだアイコンで、ほかからは使っていない。
            if let Err(e) = unsafe { DestroyIcon(icon) } {
                tracing::warn!("failed to destroy the application icon: {e}");
            }
            marked
        }
        Err(e) => {
            tracing::warn!("failed to put the shield on the application icon: {e}");
            icon
        }
    }
}

/// アイコンと、その幅。
fn load_one(
    instance: HINSTANCE,
    dpi: u32,
    width_metric: SYSTEM_METRICS_INDEX,
    height_metric: SYSTEM_METRICS_INDEX,
) -> Option<(HICON, u32)> {
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
        Ok(icon) => Some((icon, width.max(0) as u32)),
        Err(e) => {
            tracing::warn!("failed to load the {width}x{height} application icon: {e}");
            None
        }
    }
}

/// ウィンドウ `hwnd` のアイコンを DPI が `dpi` の大きさにする。`elevated` なら盾を重ねる（[`load`]）。前に
/// `set` で付けたアイコンは捨てる（ウィンドウのクラスのアイコンは捨てない）。
pub fn set(hwnd: HWND, instance: HINSTANCE, dpi: u32, elevated: bool) {
    let icons = load(instance, dpi, elevated);
    for (kind, icon) in [(ICON_BIG, icons.big), (ICON_SMALL, icons.small)] {
        let Some(icon) = icon else {
            continue;
        };
        // SAFETY: hwnd は有効なウィンドウ。WM_SETICON は前に WM_SETICON で付けたアイコンを返す。
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
        // SAFETY: 前の WM_SETICON でこの関数が付けたアイコンで、もうウィンドウは使わない。
        if let Err(e) = unsafe { DestroyIcon(HICON(previous.0 as *mut _)) } {
            tracing::warn!("failed to destroy the previous window icon: {e}");
        }
    }
}
