//! スクロールバー。マウスを載せたとき、つまみのドラッグ、つまみの外を押し続けたときのページ送り、描くもの。

use std::time::Duration;

use fantermo_core::vt::ScrollPosition;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use super::{App, SCROLL_BAR_TIMER_ID};
use crate::{paint::Paint, scroll_bar};

/// スクロールバーのつまみの外を押してから、押し続けたとしてページを送り始めるまで。
const SCROLL_BAR_REPEAT_DELAY: Duration = Duration::from_millis(250);

/// スクロールバーのつまみの外を押し続けている間に、ページを送る間隔。
const SCROLL_BAR_REPEAT_INTERVAL: Duration = Duration::from_millis(50);

/// スクロールバーの上で左ボタンを押してから離すまでの操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ScrollBarDrag {
    /// つまみをドラッグしている。`grab` は、押した位置のつまみの上端からの高さ。動かしても、つまみの
    /// 中の押した位置をマウスの位置に保つ。
    Thumb { grab: i32 },
    /// つまみの外を押し続けている。つまみがマウスの位置 `y` に来るまで、`direction` へページを送る。
    Page {
        direction: scroll_bar::Direction,
        y: i32,
    },
}

impl App {
    /// マウスがスクロールバーの上にあるかを決め直す。変わったら描き直す。
    pub(super) fn update_scroll_bar_hover(&mut self) {
        let hover = self
            .pointer_in_window
            .is_some_and(|(x, y)| self.scroll_bar_area.contains(x, y));
        if hover == self.scroll_bar_hover {
            return;
        }
        tracing::debug!("pointer on the scroll bar: {hover}");
        self.scroll_bar_hover = hover;
        self.invalidate();
    }

    /// 表示している範囲を示す、いまのつまみ。スクロールバックがなければ `None`。
    fn scroll_bar_thumb(&self) -> Option<scroll_bar::Thumb> {
        self.scroll_bar_thumb_at(self.scroll_position()?)
    }

    /// 表示している範囲が `position` のときのつまみ。スクロールバックがなければ `None`。
    pub(super) fn scroll_bar_thumb_at(
        &self,
        position: ScrollPosition,
    ) -> Option<scroll_bar::Thumb> {
        scroll_bar::thumb_in(self.scroll_bar_area, self.scroll_bar_metrics(), position)
    }

    /// スクロールバーの上で左ボタンを押した。つまみの上ならドラッグを始め、つまみの外なら、その向きへ
    /// 表示している行数だけ動かして、押し続けたら送り続けるタイマーを掛ける。アプリがマウスを
    /// 求めていても、表示する範囲を動かす。
    pub(super) fn press_scroll_bar(&mut self, y: i32) -> Option<ScrollBarDrag> {
        let thumb = self.scroll_bar_thumb()?;
        match scroll_bar::part_at(thumb, y) {
            scroll_bar::Part::Thumb => Some(ScrollBarDrag::Thumb {
                grab: y - thumb.top as i32,
            }),
            scroll_bar::Part::Page(direction) => {
                self.scroll_page(direction);
                self.set_scroll_bar_timer(SCROLL_BAR_REPEAT_DELAY);
                Some(ScrollBarDrag::Page { direction, y })
            }
        }
    }

    /// 表示している行数だけ `direction` へ動かす。
    fn scroll_page(&mut self, direction: scroll_bar::Direction) {
        let page = self.session().vt.size().rows as isize;
        self.session_mut().vt.scroll(direction.sign() * page);
        self.on_viewport_moved();
    }

    /// スクロールバーの上で押したまま、マウスが高さ `y` へ動いた。つまみをドラッグしていれば、押した
    /// 位置がマウスの位置に来るようにつまみを動かし、その位置の行を表示する。
    pub(super) fn drag_scroll_bar(&mut self, y: i32) {
        let Some((button, Some(drag))) = self.scroll_bar_press else {
            return;
        };
        match drag {
            ScrollBarDrag::Thumb { grab } => {
                let Some(position) = self.scroll_position() else {
                    return;
                };
                let Some(thumb) = self.scroll_bar_thumb_at(position) else {
                    return;
                };
                let track = scroll_bar::track(self.scroll_bar_area, self.scroll_bar_metrics());
                let row = scroll_bar::row_at(track, thumb.length, position, y - grab);
                if row == position.offset {
                    return;
                }
                self.session_mut().vt.scroll_to_row(row);
                self.on_viewport_moved();
            }
            ScrollBarDrag::Page { direction, .. } => {
                self.scroll_bar_press = Some((button, Some(ScrollBarDrag::Page { direction, y })));
            }
        }
    }

    /// つまみの外を押し続けている間のタイマーが来た。つまみがまだマウスの位置に来ていなければ、もう
    /// 1 ページ動かす。
    pub(super) fn on_scroll_bar_timer(&mut self) {
        let Some((_, Some(ScrollBarDrag::Page { direction, y }))) = self.scroll_bar_press else {
            self.kill_scroll_bar_timer();
            return;
        };
        self.set_scroll_bar_timer(SCROLL_BAR_REPEAT_INTERVAL);
        let Some(thumb) = self.scroll_bar_thumb() else {
            return;
        };
        if scroll_bar::part_at(thumb, y) == scroll_bar::Part::Page(direction) {
            self.scroll_page(direction);
        }
    }

    /// スクロールバーの上で押した操作を終える。
    pub(super) fn end_scroll_bar_press(&mut self) {
        if self.scroll_bar_press.take().is_none() {
            return;
        }
        self.kill_scroll_bar_timer();
        self.invalidate();
    }

    fn set_scroll_bar_timer(&self, delay: Duration) {
        // SAFETY: self.hwnd は有効なウィンドウ。同じ番号のタイマーは掛け直しになる。
        if unsafe {
            SetTimer(
                Some(self.hwnd),
                SCROLL_BAR_TIMER_ID,
                delay.as_millis() as u32,
                None,
            )
        } == 0
        {
            tracing::warn!(
                "failed to start the scroll bar timer: {}",
                windows::core::Error::from_thread()
            );
        }
    }

    fn kill_scroll_bar_timer(&self) {
        // SAFETY: self.hwnd は有効なウィンドウ。掛けていなければ失敗するだけ。
        if unsafe { KillTimer(Some(self.hwnd), SCROLL_BAR_TIMER_ID) }.is_err() {
            tracing::debug!("the scroll bar timer was not running");
        }
    }

    /// 表示する範囲を動かした。マウスを載せているリンクを探し直し、描き直す。
    pub(super) fn on_viewport_moved(&mut self) {
        self.refresh_hovered_link_now();
        self.invalidate();
    }

    /// 表示している端末の、表示している範囲の位置。読めなければ警告を残して `None`。
    pub(super) fn scroll_position(&self) -> Option<ScrollPosition> {
        self.session()
            .vt
            .scroll_position()
            .inspect_err(|e| tracing::warn!("failed to read the scroll position: {e}"))
            .ok()
    }

    /// 表示している範囲が `position` のときに、スクロールバーに描くもの。色は `colors`。`over_image` なら、
    /// 背景画像の上に描く。
    pub(super) fn scroll_bar_paints(
        &self,
        position: ScrollPosition,
        colors: &scroll_bar::Palette,
        over_image: bool,
    ) -> Vec<Paint> {
        let drag = self.scroll_bar_press.and_then(|(_, drag)| drag);
        let dragging = matches!(drag, Some(ScrollBarDrag::Thumb { .. }));
        // つまみの外を押し続けている間は、マウスがスクロールバーの外へ出ても、載せているときの形にする。
        let paging = matches!(drag, Some(ScrollBarDrag::Page { .. }));
        scroll_bar::paints(
            self.scroll_bar_area,
            self.scroll_bar_metrics(),
            position,
            scroll_bar::Highlight {
                hovered: self.scroll_bar_hover || paging,
                dragging,
            },
            colors,
            over_image,
        )
    }

    /// スクロールバーの寸法。
    fn scroll_bar_metrics(&self) -> scroll_bar::Metrics {
        scroll_bar::Metrics {
            dpi: self.dpi,
            base_width: self.config.scroll_bar.width,
        }
    }
}
