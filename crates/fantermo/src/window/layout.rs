//! ウィンドウのクライアント領域の配置。タブバー、端末を描く領域、余白、スクロールバー、背景画像を敷く領域の
//! 計算と、クライアント領域・端末を描く領域・画面の座標の変換。

use fantermo_core::{backend::GridSize, vt::PointerPosition};
use windows::Win32::{
    Foundation::{HWND, LPARAM, POINT, RECT, SIZE},
    Graphics::Gdi::{ClientToScreen, ScreenToClient},
    UI::{
        HiDpi::AdjustWindowRectExForDpi,
        WindowsAndMessaging::{GetClientRect, GetWindowRect, IsIconic, WINDOW_EX_STYLE},
    },
};

use super::{App, MAIN_WINDOW_STYLE, mouse::client_position};
use crate::{
    font::Font,
    paint::{Area, scale},
    renderer, scroll_bar, tab_bar,
};

/// ウィンドウの位置と大きさ（枠を含む、画面の座標）。
pub(super) fn window_rect(hwnd: HWND) -> RECT {
    let mut rect = RECT::default();
    // SAFETY: hwnd は有効なウィンドウで、rect は書き込める。
    if let Err(e) = unsafe { GetWindowRect(hwnd, &mut rect) } {
        tracing::warn!("failed to get the window position: {e}");
    }
    rect
}

/// ウィンドウの枠の大きさ（ウィンドウの大きさとクライアント領域の大きさの差）。
pub(super) fn window_frame(hwnd: HWND) -> SIZE {
    let rect = window_rect(hwnd);
    let (width, height) = client_size(hwnd);
    SIZE {
        cx: (rect.right - rect.left) - width as i32,
        cy: (rect.bottom - rect.top) - height as i32,
    }
}

/// `font` で `cols` × `rows` のセルを並べた大きさ（ピクセル）。
pub(super) fn grid_pixels(font: &Font, cols: u16, rows: u16) -> SIZE {
    SIZE {
        cx: (u32::from(cols) * font.cell_width_px) as i32,
        cy: (u32::from(rows) * font.cell_height_px) as i32,
    }
}

/// ウィンドウのクライアント領域のうち、端末の画面の周りに取る場所の寸法。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Insets {
    /// 上端のタブバーの高さ（[`tab_bar::height`]）。
    tab_bar_height: u32,
    /// 端末の画面の上・下・左・右の余白。上はタブバーとの間、右はスクロールバーとの間。何も描かず、
    /// 描画が端末の背景色で塗るところに任せる（`Renderer::render` はどの領域にも入らないところを端末の
    /// 背景色で塗る）。
    padding: u32,
    /// 右端のスクロールバーの幅（[`scroll_bar::Metrics::width`]）。
    scroll_bar_width: u32,
}

impl Insets {
    /// DPI が `dpi` のウィンドウでの寸法。`scroll_bar_width` は設定のスクロールバーの幅、`padding` は設定の
    /// 余白（どちらも 96 DPI でのピクセル。DPI に比例させる）。
    pub(super) fn new(dpi: u32, scroll_bar_width: u32, padding: u32) -> Self {
        Self {
            tab_bar_height: tab_bar::height(dpi),
            // scale は 1 ピクセルより細くしないので、余白なし（0）はそのまま。
            padding: if padding == 0 { 0 } else { scale(padding, dpi) },
            scroll_bar_width: scroll_bar::Metrics {
                dpi,
                base_width: scroll_bar_width,
            }
            .width(),
        }
    }
}

/// 大きさが `width` × `height` のクライアント領域のうち、表示している端末を描く領域。上端のタブバーと
/// 右端のスクロールバーを除き、その内側の上・下・左・右に余白（寸法は `insets`）を空けた残りの全体に
/// 描く。
pub(super) fn terminal_area_in_client(width: u32, height: u32, insets: Insets) -> Area {
    let top = (insets.tab_bar_height + insets.padding).min(height);
    let available_width = width.saturating_sub(insets.scroll_bar_width);
    Area {
        x: insets.padding.min(available_width),
        y: top,
        width: available_width.saturating_sub(2 * insets.padding),
        height: (height - top).saturating_sub(insets.padding),
    }
}

/// 大きさが `width` のクライアント領域のうち、スクロールバーを描く領域。クライアント領域の右端に幅
/// `insets.scroll_bar_width` で、上下は端末を描く領域 `terminal` に揃える。
pub(super) fn scroll_bar_area_in_client(width: u32, terminal: Area, insets: Insets) -> Area {
    let x = width.saturating_sub(insets.scroll_bar_width);
    Area {
        x,
        y: terminal.y,
        width: width - x,
        height: terminal.height,
    }
}

/// 大きさが `width` × `height` のクライアント領域のうち、背景画像を敷く領域。上端のタブバーを除いた
/// 残りの全体（端末を描く領域、その周りの余白、右端のスクロールバーの列）。
pub(super) fn backdrop_area_in_client(width: u32, height: u32, insets: Insets) -> Area {
    let top = insets.tab_bar_height.min(height);
    Area {
        x: 0,
        y: top,
        width,
        height: height - top,
    }
}

/// 端末を描く領域の大きさが `terminal` になるクライアント領域の大きさ。[`terminal_area_in_client`] と
/// 対になる。
pub(super) fn client_size_for_terminal(terminal: SIZE, insets: Insets) -> SIZE {
    let padding = 2 * insets.padding as i32;
    SIZE {
        cx: terminal.cx + padding + insets.scroll_bar_width as i32,
        cy: terminal.cy + padding + insets.tab_bar_height as i32,
    }
}

/// 端末を描く領域の左上からの位置 (`x`, `y`) を、大きさが `size` の端末のセルを並べた画面の中の、
/// いちばん近い位置にする。
fn clamp_to_grid(x: i32, y: i32, size: GridSize) -> (i32, i32) {
    let limit = |count: u16, cell_px: u32| (u32::from(count) * cell_px).saturating_sub(1) as i32;
    (
        x.clamp(0, limit(size.cols, size.cell_width_px)),
        y.clamp(0, limit(size.rows, size.cell_height_px)),
    )
}

/// クライアント領域の中の位置を、`area` の左上からの位置にする。
fn client_to_area(area: Area, x: i32, y: i32) -> (i32, i32) {
    (x - area.x as i32, y - area.y as i32)
}

/// `area` の左上からの位置を、クライアント領域の中の位置にする。[`client_to_area`] と対になる。
fn area_to_client(area: Area, x: i32, y: i32) -> (i32, i32) {
    (x + area.x as i32, y + area.y as i32)
}

/// クライアント領域の大きさが `client` になる、DPI が `dpi` のウィンドウの大きさ（枠を含む）。
pub(super) fn window_size(client: SIZE, dpi: u32) -> windows::core::Result<SIZE> {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: client.cx,
        bottom: client.cy,
    };
    // SAFETY: rect は書き込める。
    unsafe {
        AdjustWindowRectExForDpi(&mut rect, MAIN_WINDOW_STYLE, false, WINDOW_EX_STYLE(0), dpi)?
    };
    Ok(SIZE {
        cx: rect.right - rect.left,
        cy: rect.bottom - rect.top,
    })
}

/// 端末を描く領域 `area` に収まる大きさ。少なくとも 1 セル。
pub(super) fn grid_size(font: &Font, area: Area) -> GridSize {
    GridSize {
        cols: (area.width / font.cell_width_px).clamp(1, u32::from(u16::MAX)) as u16,
        rows: (area.height / font.cell_height_px).clamp(1, u32::from(u16::MAX)) as u16,
        cell_width_px: font.cell_width_px,
        cell_height_px: font.cell_height_px,
    }
}

pub(super) fn client_size(hwnd: HWND) -> (u32, u32) {
    let mut rect = RECT::default();
    // SAFETY: hwnd は有効なウィンドウで、rect は書き込める。
    if let Err(e) = unsafe { GetClientRect(hwnd, &mut rect) } {
        tracing::warn!("failed to get the client size: {e}");
    }
    (
        (rect.right - rect.left).max(0) as u32,
        (rect.bottom - rect.top).max(0) as u32,
    )
}

impl App {
    /// ウィンドウのクライアント領域の中の位置を、端末を描く領域の左上からの位置にする。
    fn client_to_terminal(&self, x: i32, y: i32) -> (i32, i32) {
        client_to_area(self.terminal_area, x, y)
    }

    /// 端末を描く領域の左上からの位置を、ウィンドウのクライアント領域の中の位置にする。
    pub(super) fn terminal_to_client(&self, x: i32, y: i32) -> (i32, i32) {
        area_to_client(self.terminal_area, x, y)
    }

    /// ウィンドウのクライアント領域の中の位置を、端末を描く領域の左上からの位置にする。クライアント領域の
    /// 中でセルの外（端末の画面の余白など）なら、いちばん近いセルの中の位置にする。クライアント領域の外
    /// （ドラッグでウィンドウの外へ出たとき）はそのまま。マウスの選択とアプリへの報告に使う。
    pub(super) fn pointer_in_terminal(&self, x: i32, y: i32) -> (i32, i32) {
        let (terminal_x, terminal_y) = self.client_to_terminal(x, y);
        let (width, height) = client_size(self.hwnd);
        if x < 0 || y < 0 || x as u32 >= width || y as u32 >= height {
            return (terminal_x, terminal_y);
        }
        clamp_to_grid(terminal_x, terminal_y, self.session().vt.size())
    }

    /// ウィンドウのクライアント領域の中の位置を、表示している範囲のセルに直す。端末を描く領域の外は端の
    /// セルにする。
    pub(super) fn pointer(&self, x: i32, y: i32) -> PointerPosition {
        let (x, y) = self.pointer_in_terminal(x, y);
        let size = self.session().vt.size();
        let cell = |pixel: i32, cell_px: u32, count: u16| -> u16 {
            let index = pixel.max(0) as u32 / cell_px;
            index.min(u32::from(count.saturating_sub(1))) as u16
        };
        PointerPosition {
            col: cell(x, size.cell_width_px, size.cols),
            row: cell(y, size.cell_height_px, size.rows),
            x: f64::from(x),
            y: f64::from(y),
        }
    }

    /// ウィンドウのクライアント領域の中の位置にあるセル（桁、行）。セルの外（端末の画面の余白、
    /// スクロールバー）なら `None`。
    pub(super) fn cell_at(&self, x: i32, y: i32) -> Option<(u16, u16)> {
        let (x, y) = self.client_to_terminal(x, y);
        let size = self.session().vt.size();
        if x < 0
            || y < 0
            || x as u32 >= u32::from(size.cols) * size.cell_width_px
            || y as u32 >= u32::from(size.rows) * size.cell_height_px
        {
            return None;
        }
        let col = (x as u32 / size.cell_width_px).min(u32::from(u16::MAX)) as u16;
        let row = (y as u32 / size.cell_height_px).min(u32::from(u16::MAX)) as u16;
        Some((col, row))
    }

    /// ウィンドウのクライアント領域の中の位置を、画面での位置にする。
    pub(super) fn client_to_screen(&self, x: i32, y: i32) -> (i32, i32) {
        let mut point = POINT { x, y };
        // SAFETY: self.hwnd は有効なウィンドウで、point は書き込める。
        if !unsafe { ClientToScreen(self.hwnd, &mut point) }.as_bool() {
            tracing::warn!("failed to convert the menu position");
        }
        (point.x, point.y)
    }

    /// 画面での位置（WM_MOUSEWHEEL の `lparam`）を、ウィンドウのクライアント領域の中の位置にする。
    pub(super) fn screen_to_client(&self, lparam: LPARAM) -> (i32, i32) {
        let (x, y) = client_position(lparam);
        let mut point = POINT { x, y };
        // SAFETY: self.hwnd は有効なウィンドウで、point は書き込める。
        if !unsafe { ScreenToClient(self.hwnd, &mut point) }.as_bool() {
            tracing::warn!("failed to convert the wheel position");
        }
        (point.x, point.y)
    }

    /// ウィンドウのクライアント領域の大きさが変わった。端末を描く領域と、端末の大きさを合わせる。
    pub(super) fn on_resize(&mut self, width: u32, height: u32) {
        if let Some(renderer) = self.renderer.as_mut()
            && let Err(e) = renderer.resize(width, height)
        {
            if renderer::is_device_lost(&e) {
                self.on_device_lost(&e);
            } else {
                tracing::warn!("failed to resize the swap chain: {e}");
            }
        }
        self.layout(width, height);
    }

    /// いまのクライアント領域の大きさに合わせる。最小化していれば何もしない（大きさ 0 を端末に伝えないため。
    /// 戻したときに届く WM_SIZE で合わせる）。
    pub(super) fn fit_to_client(&mut self) {
        // SAFETY: self.hwnd は有効なウィンドウ。
        if unsafe { IsIconic(self.hwnd) }.as_bool() {
            return;
        }
        let (width, height) = client_size(self.hwnd);
        self.on_resize(width, height);
    }

    /// クライアント領域の大きさが `width` × `height` のときの、端末を描く領域に合わせる。
    fn layout(&mut self, width: u32, height: u32) {
        let insets = self.insets(self.dpi);
        self.terminal_area = terminal_area_in_client(width, height, insets);
        self.scroll_bar_area = scroll_bar_area_in_client(width, self.terminal_area, insets);
        self.backdrop_area = backdrop_area_in_client(width, height, insets);
        self.resize_sessions();
        self.update_tab_bar_hover();
        self.update_scroll_bar_hover();
        // 余白やスクロールバーの幅だけが変わると、マウスの下のセルが変わる。
        self.refresh_hovered_link_now();
    }

    /// すべての端末の大きさを、端末を描く領域に合わせる。表示していない端末も同じ大きさにする。
    fn resize_sessions(&mut self) {
        let size = grid_size(&self.font, self.terminal_area);
        let mut changed = false;
        for session in &mut self.sessions {
            changed |= session.resize(size);
        }
        if !changed {
            return;
        }
        self.refresh_hovered_link_now();
        self.invalidate();
    }

    /// DPI が `dpi` のときの、端末の画面の周りに取る場所の寸法。
    pub(super) fn insets(&self, dpi: u32) -> Insets {
        Insets::new(dpi, self.scroll_bar_width, self.padding)
    }

    /// タブバーを描く領域。ウィンドウの上端に、ウィンドウの幅いっぱい。その下の端末の画面の上の余白は
    /// 含まない（余白の上のマウスは、ほかの余白と同じく端末のいちばん近いセルとみなす）。
    pub(super) fn tab_bar_area(&self) -> Area {
        Area {
            x: 0,
            y: 0,
            width: self.scroll_bar_area.right(),
            height: self
                .insets(self.dpi)
                .tab_bar_height
                .min(self.terminal_area.y),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_terminal_is_surrounded_by_the_padding() {
        // 96 DPI ではタブバーの 26、既定の余白 4、既定の幅のスクロールバー 12。192 DPI では 52、8、24。
        // 設定の余白は 0 から、DPI に比例させる。
        for (dpi, setting, bar, padding, scroll_bar) in [
            (96, fantermo_core::config::DEFAULT_PADDING, 26, 4, 12),
            (192, fantermo_core::config::DEFAULT_PADDING, 52, 8, 24),
            (96, 0, 26, 0, 12),
            (96, 20, 26, 20, 12),
            (192, 20, 52, 40, 24),
        ] {
            let insets = Insets::new(
                dpi,
                fantermo_core::config::DEFAULT_SCROLL_BAR_WIDTH,
                setting,
            );
            assert_eq!(
                insets,
                Insets {
                    tab_bar_height: bar,
                    padding,
                    scroll_bar_width: scroll_bar
                }
            );
            let terminal = SIZE { cx: 800, cy: 480 };
            let client = client_size_for_terminal(terminal, insets);
            assert_eq!(
                (client.cx as u32, client.cy as u32),
                (800 + 2 * padding + scroll_bar, 480 + bar + 2 * padding)
            );
            let area = terminal_area_in_client(client.cx as u32, client.cy as u32, insets);
            let top = bar + padding;
            assert_eq!(
                area,
                Area {
                    x: padding,
                    y: top,
                    width: 800,
                    height: 480
                }
            );
            // スクロールバーはクライアント領域の右端で、端末との間に余白。上下は端末に揃える。
            assert_eq!(
                scroll_bar_area_in_client(client.cx as u32, area, insets),
                Area {
                    x: 800 + 2 * padding,
                    y: top,
                    width: scroll_bar,
                    height: 480
                }
            );
            // 端末の左上のセルの左上は、クライアント領域の (padding, top)。余白の中は端末の外。
            let (left, top) = (padding as i32, top as i32);
            assert_eq!(client_to_area(area, left, top), (0, 0));
            assert_eq!(client_to_area(area, left - 1, top - 1), (-1, -1));
            assert_eq!(client_to_area(area, left + 800, top + 480), (800, 480));
            assert_eq!(area_to_client(area, 16, 32), (left + 16, top + 32));
            // 背景画像は端末と余白とスクロールバーの列に敷く。
            assert_eq!(
                backdrop_area_in_client(client.cx as u32, client.cy as u32, insets),
                Area {
                    x: 0,
                    y: bar,
                    width: 800 + 2 * padding + scroll_bar,
                    height: 480 + 2 * padding
                }
            );
        }
        // タブバーより低いクライアント領域では、端末を描く領域は空。スクロールバーと左右の余白より
        // 狭ければ、端末の幅は 0 で、スクロールバーはクライアント領域の右端に設定の幅。
        let insets = Insets {
            tab_bar_height: 26,
            padding: 4,
            scroll_bar_width: 12,
        };
        assert_eq!(terminal_area_in_client(800, 20, insets).height, 0);
        assert_eq!(terminal_area_in_client(800, 32, insets).height, 0);
        let narrow = terminal_area_in_client(18, 100, insets);
        assert_eq!((narrow.x, narrow.width), (4, 0));
        assert_eq!(
            scroll_bar_area_in_client(18, narrow, insets),
            Area {
                x: 6,
                y: 30,
                width: 12,
                height: 66
            }
        );
        assert_eq!(scroll_bar_area_in_client(8, narrow, insets).width, 8);
        // 設定の幅は DPI に比例させる。
        assert_eq!(Insets::new(144, 20, 4).scroll_bar_width, 30);
        assert_eq!(Insets::new(144, 12, 5).padding, 8);
    }

    #[test]
    fn positions_outside_the_cells_go_to_the_nearest_cell() {
        // 10 × 5 のセル、一つが 8 × 16 ピクセル。
        let size = GridSize {
            cols: 10,
            rows: 5,
            cell_width_px: 8,
            cell_height_px: 16,
        };
        // 左と上の余白は先頭の列と行の端、右と下の余白は最後の列と行の端。
        assert_eq!(clamp_to_grid(-4, -4, size), (0, 0));
        assert_eq!(clamp_to_grid(85, 90, size), (79, 79));
        assert_eq!(clamp_to_grid(-1, 40, size), (0, 40));
        // セルの中はそのまま。
        assert_eq!(clamp_to_grid(0, 0, size), (0, 0));
        assert_eq!(clamp_to_grid(79, 79, size), (79, 79));
        assert_eq!(clamp_to_grid(33, 17, size), (33, 17));
    }
}
