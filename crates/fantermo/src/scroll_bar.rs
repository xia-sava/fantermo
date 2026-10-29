//! 窓の右端の縦のスクロールバー（Windows の標準のもの、`WS_VSCROLL`）。つまみの位置と長さで、
//! スクロールバックを含む全体の中の、表示している範囲を示す。スクロールバックがないときも消さずに、
//! 押せない形で出しておく（出し入れでクライアント領域の幅が変わり、桁数が変わらないように）。

use fantermo_core::vt::ScrollPosition;
use windows::Win32::{
    Foundation::HWND,
    UI::{
        Controls::SetScrollInfo,
        HiDpi::GetSystemMetricsForDpi,
        WindowsAndMessaging::{
            GetScrollInfo, SB_BOTTOM, SB_LINEDOWN, SB_LINEUP, SB_PAGEDOWN, SB_PAGEUP,
            SB_THUMBPOSITION, SB_THUMBTRACK, SB_TOP, SB_VERT, SCROLLBAR_COMMAND, SCROLLINFO,
            SIF_DISABLENOSCROLL, SIF_PAGE, SIF_POS, SIF_RANGE, SIF_TRACKPOS, SM_CXVSCROLL,
        },
    },
};

/// スクロールバーに渡す値（`SCROLLINFO` の `nMin`・`nMax`・`nPage`・`nPos`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub min: i32,
    pub max: i32,
    pub page: u32,
    pub pos: i32,
}

/// 何もないときの値。押せない形で、つまみは全体いっぱい。
const EMPTY: Range = Range {
    min: 0,
    max: 0,
    page: 1,
    pos: 0,
};

/// 表示している範囲の位置 `position` を、スクロールバーの値にする。1 行を 1 にとり、全体を
/// `0..=total - 1`、つまみの長さを表示している行数、位置を表示している範囲の先頭の行にする。
/// 表示している範囲が全体を覆えば（スクロールバックがない、代替画面）、`page` が範囲を覆うので
/// Windows はつまみを全体いっぱいにして押せない形で描く（[`SIF_DISABLENOSCROLL`]）。
pub fn range(position: ScrollPosition) -> Range {
    if position.total == 0 {
        return EMPTY;
    }
    let to_i32 = |value: usize| i32::try_from(value).unwrap_or(i32::MAX);
    Range {
        min: 0,
        max: to_i32(position.total - 1),
        page: u32::try_from(position.len).unwrap_or(u32::MAX),
        pos: to_i32(position.offset),
    }
}

/// スクロールバーの操作（WM_VSCROLL）で、表示する範囲をどう動かすか。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// 行の数だけ動かす。負の値で過去（上）へ。矢印のボタン。
    Lines(isize),
    /// 表示している行数を単位に動かす。負の値で過去（上）へ。つまみの外のクリック。
    Pages(isize),
    /// いちばん古い行へ。右クリックのメニューの「一番上」。
    Top,
    /// いちばん新しい行へ。右クリックのメニューの「一番下」。
    Bottom,
    /// つまみをドラッグしている位置へ。位置は [`track_position`] で読む。
    Track,
}

/// WM_VSCROLL の `wparam` の下位の語 `command` の操作。動かさないもの（SB_ENDSCROLL）は `None`。
pub fn request(command: i32) -> Option<Request> {
    match SCROLLBAR_COMMAND(command) {
        SB_LINEUP => Some(Request::Lines(-1)),
        SB_LINEDOWN => Some(Request::Lines(1)),
        SB_PAGEUP => Some(Request::Pages(-1)),
        SB_PAGEDOWN => Some(Request::Pages(1)),
        SB_TOP => Some(Request::Top),
        SB_BOTTOM => Some(Request::Bottom),
        SB_THUMBTRACK | SB_THUMBPOSITION => Some(Request::Track),
        _ => None,
    }
}

/// 窓 `hwnd` のスクロールバーを `range` にする。範囲がつまみに収まっても、消さずに押せない形にする。
pub fn set(hwnd: HWND, range: Range) {
    let info = SCROLLINFO {
        cbSize: size_of::<SCROLLINFO>() as u32,
        fMask: SIF_RANGE | SIF_PAGE | SIF_POS | SIF_DISABLENOSCROLL,
        nMin: range.min,
        nMax: range.max,
        nPage: range.page,
        nPos: range.pos,
        nTrackPos: 0,
    };
    // SAFETY: hwnd は有効な窓で、info は初期化済み。戻り値は新しい位置で、失敗を表さない。
    unsafe { SetScrollInfo(hwnd, SB_VERT, &info, true) };
}

/// 窓 `hwnd` のスクロールバーを、何もないときの形（押せない、つまみは全体いっぱい）にする。
pub fn set_empty(hwnd: HWND) {
    set(hwnd, EMPTY);
}

/// つまみをドラッグしている位置（32 ビット。WM_VSCROLL の `wparam` の上位の語は 16 ビットまで）。
pub fn track_position(hwnd: HWND) -> windows::core::Result<i32> {
    let mut info = SCROLLINFO {
        cbSize: size_of::<SCROLLINFO>() as u32,
        fMask: SIF_TRACKPOS,
        ..Default::default()
    };
    // SAFETY: hwnd は有効な窓で、info は書き込める。
    unsafe { GetScrollInfo(hwnd, SB_VERT, &mut info)? };
    Ok(info.nTrackPos)
}

/// DPI が `dpi` のときの、スクロールバーの幅（ピクセル）。
pub fn width(dpi: u32) -> i32 {
    // SAFETY: 決まった番号の寸法を問い合わせるだけ。
    unsafe { GetSystemMetricsForDpi(SM_CXVSCROLL, dpi) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(total: usize, offset: usize, len: usize) -> ScrollPosition {
        ScrollPosition { total, offset, len }
    }

    #[test]
    fn the_thumb_covers_the_shown_rows() {
        // 30 行の画面と 70 行のスクロールバック。いちばん新しい行では、つまみは下端に付く
        // （位置の最大は max - page + 1）。
        let bottom = range(position(100, 70, 30));
        assert_eq!(
            bottom,
            Range {
                min: 0,
                max: 99,
                page: 30,
                pos: 70
            }
        );
        assert_eq!(bottom.pos, bottom.max - bottom.page as i32 + 1);
        assert_eq!(range(position(100, 0, 30)).pos, 0);
        assert_eq!(range(position(100, 25, 30)).pos, 25);
    }

    #[test]
    fn without_scrollback_the_thumb_fills_the_bar() {
        // スクロールバックがない・代替画面では、全体が表示している範囲と同じで、つまみが範囲を覆う。
        let full = range(position(30, 0, 30));
        assert_eq!(
            full,
            Range {
                min: 0,
                max: 29,
                page: 30,
                pos: 0
            }
        );
        assert!(full.page as i32 > full.max - full.min);
        assert_eq!(range(position(0, 0, 0)), EMPTY);
    }

    #[test]
    fn huge_counts_are_clamped() {
        let huge = range(position(usize::MAX, usize::MAX - 10, 10));
        assert_eq!(huge.max, i32::MAX);
        assert_eq!(huge.pos, i32::MAX);
        assert_eq!(huge.page, 10);
    }

    #[test]
    fn scroll_commands_map_to_requests() {
        assert_eq!(request(SB_LINEUP.0), Some(Request::Lines(-1)));
        assert_eq!(request(SB_LINEDOWN.0), Some(Request::Lines(1)));
        assert_eq!(request(SB_PAGEUP.0), Some(Request::Pages(-1)));
        assert_eq!(request(SB_PAGEDOWN.0), Some(Request::Pages(1)));
        assert_eq!(request(SB_TOP.0), Some(Request::Top));
        assert_eq!(request(SB_BOTTOM.0), Some(Request::Bottom));
        assert_eq!(request(SB_THUMBTRACK.0), Some(Request::Track));
        assert_eq!(request(SB_THUMBPOSITION.0), Some(Request::Track));
        assert_eq!(
            request(windows::Win32::UI::WindowsAndMessaging::SB_ENDSCROLL.0),
            None
        );
    }
}
