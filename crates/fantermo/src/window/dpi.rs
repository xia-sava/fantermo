//! DPI とフォント。端末のフォントと UI のフォントを DPI で測り直し、グリフアトラスを作り直す。起動したときと
//! DPI が変わったときの `WM_DPICHANGED` の送り方。

use std::rc::Rc;

use windows::Win32::{
    Foundation::{HWND, LPARAM, RECT, SIZE, WPARAM},
    UI::WindowsAndMessaging::{SendMessageW, WM_DPICHANGED},
};

use super::{
    App,
    layout::{client_size_for_terminal, grid_pixels, window_frame, window_rect},
    proc::app_of,
};
use crate::{
    font::{self, Font},
    renderer,
    ui_font::UiFont,
};

/// 開発用。起動したときのウィンドウの DPI を、この環境変数の値とみなす。倍率の違うモニタへ移したときの
/// 作り直しを、モニタを使わずに確認するために使う。
const DPI_ENV: &str = "FANTERMO_DPI";

pub(super) fn log_ui_font(font: &UiFont, dpi: u32) {
    tracing::info!(
        "UI font {} {}px at {dpi} DPI: line {} px",
        font.family,
        font.size_px,
        font.height_px()
    );
}

pub(super) fn log_font(font: &Font, dpi: u32) {
    tracing::info!(
        "font {} {}pt at {dpi} DPI: cell {}x{} px",
        font.family,
        font.size_pt,
        font.cell_width_px,
        font.cell_height_px
    );
}

/// [`DPI_ENV`] で指定された DPI。指定がないか、正の整数でなければ `None`。
pub(super) fn dpi_from_env() -> Option<u32> {
    let value = std::env::var(DPI_ENV).ok()?;
    match value.parse::<u32>() {
        Ok(dpi) if dpi > 0 => Some(dpi),
        _ => {
            tracing::warn!("ignoring {DPI_ENV}={value:?}: not a positive integer");
            None
        }
    }
}

/// `WM_DPICHANGED` を推奨の矩形 `rect` を添えて送り、DPI が `dpi` に変わったときの処理（フォントと
/// アトラスの作り直しと、`rect` への移動）をさせる。借用を持っていないときに呼ぶ。
pub(super) fn send_dpi_changed(hwnd: HWND, dpi: u32, rect: &RECT) {
    // WM_DPICHANGED の wparam は、下位に横の DPI、上位に縦の DPI。
    let wparam = WPARAM((dpi | (dpi << 16)) as usize);
    // SAFETY: hwnd は有効なウィンドウ。rect は処理が終わるまで生きている。借用は持っていない。
    unsafe {
        SendMessageW(
            hwnd,
            WM_DPICHANGED,
            Some(wparam),
            Some(LPARAM(std::ptr::from_ref(rect) as isize)),
        )
    };
}

/// ウィンドウの位置と大きさを変えずに、DPI が `dpi` に変わったときの処理をさせる。桁数と行数は、いまの
/// クライアント領域にその DPI のセルで入る数になる。
pub(super) fn change_dpi_keeping_window(hwnd: HWND, dpi: u32) {
    send_dpi_changed(hwnd, dpi, &window_rect(hwnd));
}

/// ウィンドウの左上を動かさずに、DPI が `dpi` に変わったときの処理をさせる。大きさは、いまの桁数と行数が
/// その DPI でちょうど入る大きさにする。覚えていた大きさがないときの起動に使う。
pub(super) fn change_dpi_keeping_grid(hwnd: HWND, dpi: u32) {
    let Some(app) = app_of(hwnd) else {
        return;
    };
    let client = app.borrow().scaled_client_size(dpi);
    let client = match client {
        Ok(client) => client,
        Err(e) => {
            tracing::warn!("failed to compute the window size for {dpi} DPI: {e}");
            return;
        }
    };
    let mut rect = window_rect(hwnd);
    // 枠の大きさは、いまのウィンドウの枠から取る。DPI_ENV で DPI を差し替えたときも、枠はウィンドウの実際の
    // DPI で付くため。
    let frame = window_frame(hwnd);
    rect.right = rect.left + client.cx + frame.cx;
    rect.bottom = rect.top + client.cy + frame.cy;
    send_dpi_changed(hwnd, dpi, &rect);
}

impl App {
    /// DPI が `dpi` に変わった。端末のフォントを測り直し、[`App::apply_font`] で入れ替え、タブのアイコンを
    /// その DPI の一辺で作り直す。端末の大きさは呼び出し元が合わせる（このあと届く WM_SIZE の
    /// [`App::on_resize`]）。フォントを測れなければ前のフォントのまま描く。
    pub(super) fn apply_dpi(&mut self, dpi: u32) {
        self.dpi = dpi;
        match font::find_or_default(&self.dwrite, &self.config.font, dpi) {
            Ok(font) => self.apply_font(font),
            Err(e) => tracing::warn!("failed to measure the font for {dpi} DPI: {e}"),
        }
        // フォントを測れなかったときと、アトラスを作り直せなかったときも、アイコンの一辺は新しい DPI に合わせる。
        self.update_tab_icons();
    }

    /// いまの DPI で測った端末のフォント `font` に入れ替える。UI のフォントを測り直し、グリフアトラスを
    /// 作り直す。端末の大きさは呼び出し元が合わせる。アトラスを作り直せなければ前のフォントのまま描く
    /// （UI のフォントだけを作れなければ、UI のフォントだけ前のまま）。
    pub(super) fn apply_font(&mut self, font: Font) {
        let dpi = self.dpi;
        let ui_font = match UiFont::for_dpi(&self.dwrite, dpi) {
            Ok(ui_font) => Rc::new(ui_font),
            Err(e) => {
                tracing::warn!("failed to create the UI font for {dpi} DPI: {e}");
                self.ui_font.clone()
            }
        };
        // `Renderer` がないとき（作り直しを待つ間と、描画をやめたあと）と、デバイスを失ったときも、
        // フォントは入れ替え、作り直す `Renderer` に使う。
        let rebuilt = match self.renderer.as_mut() {
            Some(renderer) => renderer.set_font(
                &self.dwrite,
                &font,
                ui_font.clone(),
                self.config.font.wide_glyph,
            ),
            None => Ok(()),
        };
        let lost = match rebuilt {
            Ok(()) => None,
            Err(e) if renderer::is_device_lost(&e) => Some(e),
            Err(e) => {
                tracing::warn!("failed to rebuild the glyph atlas for {dpi} DPI: {e}");
                return;
            }
        };
        log_font(&font, dpi);
        log_ui_font(&ui_font, dpi);
        self.font = font;
        self.ui_font = ui_font;
        // 頭文字の書体が変わっていれば、タブのアイコンを作り直す。
        self.update_tab_icons();
        if let Some(e) = lost {
            self.on_device_lost(&e);
        }
        self.invalidate();
    }

    /// DPI が `dpi` に変わったときに、いまの桁数と行数がそのまま収まるクライアント領域の大きさ。
    fn scaled_client_size(&self, dpi: u32) -> windows::core::Result<SIZE> {
        let font = font::find_or_default(&self.dwrite, &self.config.font, dpi)?;
        let size = self.session().vt.size();
        Ok(client_size_for_terminal(
            grid_pixels(&font, size.cols, size.rows),
            self.insets(dpi),
        ))
    }
}
