//! 描画。背景画像の選び方、Direct3D のデバイスを失ったときの作り直し、画面の書き出し、点滅と画像の
//! アニメーションのタイマー。

use std::{
    hash::{BuildHasher, Hasher, RandomState},
    path::Path,
    time::{Duration, SystemTime},
};

use fantermo_core::background::Candidate;
use windows::Win32::{
    Foundation::{HWND, WPARAM},
    UI::WindowsAndMessaging::{IsIconic, KillTimer, SetTimer},
};

use super::{
    App, BLINK_TIMER_ID, IMAGE_ANIMATION_TIMER_ID, RENDERER_RETRY_TIMER_ID, WM_APP_DEVICE_REMOVED,
    layout::client_size, poster,
};
use crate::{
    background::DiskFiles,
    renderer::{self, Backdrop, Renderer},
    scroll_bar,
};

/// 点滅の文字を見せる間と隠す間の、それぞれの長さ。
const BLINK_INTERVAL: Duration = Duration::from_millis(500);

/// `Renderer` を作り直せなかったときに、もう一度試すまでの間。
const RENDERER_RETRY_DELAY: Duration = Duration::from_millis(1500);

/// 画像のアニメーションで描き直す間隔の下限。
const MIN_IMAGE_ANIMATION_INTERVAL: Duration = Duration::from_millis(8);

/// Direct3D のデバイスを失ったときに、`Renderer` を作り直しても描けないことがこの回数続いたら、描画を
/// やめる。
const MAX_RENDERER_RECREATIONS: u32 = 3;

/// 背景画像を選ぶ乱数の種。起動するたびに変わる。
pub(super) fn random_seed() -> u64 {
    let mut hasher = RandomState::new().build_hasher();
    if let Ok(elapsed) = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        hasher.write_u128(elapsed.as_nanos());
    }
    hasher.finish()
}

impl App {
    /// 設定の背景画像の一覧から、ランダムに一枚選んで読む。読めない画像は警告を残して飛ばす。一覧が
    /// 空か、どれも読めなければ `None`。`Renderer` がないとき（作り直しを待つ間と、描画をやめたあと）も
    /// `None`（作り直したときに選び直す）。
    pub(super) fn choose_background_image(&mut self) -> Option<Candidate> {
        if self.config.background.images.is_empty() {
            return None;
        }
        let renderer = self.renderer.as_mut()?;
        let base = self.config_path.as_deref().and_then(Path::parent);
        let expanded =
            fantermo_core::background::expand(&self.config.background.images, base, &DiskFiles);
        for warning in &expanded.warnings {
            tracing::warn!("{warning}");
        }
        let mut candidates = expanded.candidates;
        let random = &mut self.random;
        let picked = fantermo_core::background::pick(
            &mut candidates,
            &mut |count| random.below(count),
            |candidate| match renderer.load_image(&candidate.path) {
                Ok(()) => Some(()),
                Err(e) => {
                    tracing::warn!(
                        "cannot read the background image {}: {e}",
                        candidate.path.display()
                    );
                    None
                }
            },
        );
        if picked.is_none() {
            tracing::warn!("none of the background images can be read");
        }
        picked.map(|(candidate, ())| candidate)
    }

    /// すべての端末の背景画像を選び直す。
    pub(super) fn rechoose_background_images(&mut self) {
        for index in 0..self.sessions.len() {
            self.sessions[index].background_image = self.choose_background_image();
        }
        self.drop_unused_background_images();
        self.invalidate();
    }

    /// どの端末も使っていない背景画像を捨てる。
    pub(super) fn drop_unused_background_images(&mut self) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let sessions = &self.sessions;
        renderer.retain_images(|path| {
            sessions.iter().any(|session| {
                session
                    .background_image
                    .as_ref()
                    .is_some_and(|image| image.path == path)
            })
        });
    }

    /// Direct3D のデバイスを失って `error` になった。理由をログに残し、`Renderer` を作り直す。最初の
    /// 作り直しはこの場で、作り直したのに描けなかったあとは [`RENDERER_RETRY_DELAY`] のあとに試す。
    /// 作り直しても描けないことが [`MAX_RENDERER_RECREATIONS`] 回続いていたら、作り直さずに描画をやめる。
    pub(super) fn on_device_lost(&mut self, error: &windows::core::Error) {
        if let Some(renderer) = self.renderer.take() {
            tracing::warn!(
                "lost the Direct3D device ({error}), reason: {}",
                renderer.device_removed_reason()
            );
            // 同じウィンドウに新しいスワップチェーンを作るので、古いものを先に手放す。
            renderer.release();
        }
        if self.renderer_recreations >= MAX_RENDERER_RECREATIONS {
            tracing::warn!(
                "stopped rendering: could not draw after recreating the renderer {} times",
                self.renderer_recreations
            );
            return;
        }
        if self.renderer_recreations == 0 {
            self.recreate_renderer();
        } else {
            self.start_renderer_retry_timer();
        }
    }

    /// Direct3D のデバイスを失ったと知らされた（[`WM_APP_DEVICE_REMOVED`]）。今の `Renderer` のデバイスを
    /// 失っていれば、描くときに気づいたときと同じく [`App::on_device_lost`] で作り直す。
    pub(super) fn on_device_removed_message(&mut self) {
        let removed = self.renderer.as_ref().map(Renderer::device_removed);
        match lost_device_to_recreate(removed) {
            Some(error) => {
                tracing::info!("notified that the Direct3D device was removed");
                self.on_device_lost(&error);
            }
            None => tracing::debug!(
                "ignored the notice of a removed Direct3D device: the current renderer is not affected"
            ),
        }
    }

    /// 失ったデバイスの代わりに `Renderer` を作り直し、各端末が選んだ背景画像を読み直す。読み直せなかった
    /// 端末と、`Renderer` がない間に開いた端末には、背景画像を選び直す。端末の中の画像とタブのアイコンの
    /// テクスチャは、描くときに作り直される。
    ///
    /// 作れなければ [`RENDERER_RETRY_DELAY`] のあとにもう一度試す。作り直しても描けないことが
    /// [`MAX_RENDERER_RECREATIONS`] 回続いたら、描画をやめる。
    fn recreate_renderer(&mut self) {
        if self.renderer_recreations >= MAX_RENDERER_RECREATIONS {
            return;
        }
        self.renderer_recreations += 1;
        let (width, height) = client_size(self.hwnd);
        let renderer = match Renderer::new(
            self.hwnd,
            width,
            height,
            &self.dwrite,
            &self.font,
            self.ui_font.clone(),
            self.config.font.wide_glyph,
            device_removed_poster(self.hwnd),
        ) {
            Ok(renderer) => renderer,
            Err(e) if self.renderer_recreations >= MAX_RENDERER_RECREATIONS => {
                tracing::warn!(
                    "stopped rendering: could not recreate the renderer or draw with it {} times: {e}",
                    self.renderer_recreations
                );
                return;
            }
            Err(e) => {
                tracing::warn!("failed to recreate the renderer, retrying later: {e}");
                self.start_renderer_retry_timer();
                return;
            }
        };
        tracing::info!("recreated the renderer after losing the Direct3D device");
        // 最小化している間は描かないので、描けたときを待たずに数え直す。
        // SAFETY: self.hwnd は有効なウィンドウ。
        if unsafe { IsIconic(self.hwnd) }.as_bool() {
            self.renderer_recreations = 0;
        }
        let renderer = self.renderer.insert(renderer);
        for session in &mut self.sessions {
            let Some(image) = &session.background_image else {
                continue;
            };
            if let Err(e) = renderer.load_image(&image.path) {
                tracing::warn!(
                    "cannot read the background image {} again: {e}",
                    image.path.display()
                );
                session.background_image = None;
            }
        }
        for index in 0..self.sessions.len() {
            if self.sessions[index].background_image.is_none() {
                self.sessions[index].background_image = self.choose_background_image();
            }
        }
        self.invalidate();
    }

    /// [`RENDERER_RETRY_DELAY`] のあとに `Renderer` の作り直しをもう一度試すタイマーを動かす。
    fn start_renderer_retry_timer(&self) {
        let millis = RENDERER_RETRY_DELAY.as_millis() as u32;
        // SAFETY: self.hwnd は有効なウィンドウ。同じ番号のタイマーは置き換わる。
        if unsafe { SetTimer(Some(self.hwnd), RENDERER_RETRY_TIMER_ID, millis, None) } == 0 {
            tracing::warn!(
                "stopped rendering: failed to start the timer to recreate the renderer: {}",
                windows::core::Error::from_thread()
            );
        }
    }

    /// `Renderer` の作り直しをもう一度試す時刻になった。一度だけのタイマーとして止めてから試す。
    pub(super) fn on_renderer_retry_timer(&mut self) {
        // SAFETY: self.hwnd は有効なウィンドウ。
        if let Err(e) = unsafe { KillTimer(Some(self.hwnd), RENDERER_RETRY_TIMER_ID) } {
            tracing::warn!("failed to stop the timer to recreate the renderer: {e}");
        }
        if self.renderer.is_none() {
            self.recreate_renderer();
        }
    }

    pub(super) fn render(&mut self) {
        self.render_to(None);
    }

    pub(super) fn write_dump(&mut self) {
        let Some(dump) = &self.dump else {
            return;
        };
        dump.write_screen(
            &self.session().vt,
            &self.font,
            self.sessions.len(),
            self.active,
            self.session()
                .background_image
                .as_ref()
                .map(|image| image.path.as_path()),
        );
        let frame_path = dump.frame_path();
        self.render_to(Some(&frame_path));
    }

    /// 表示している端末を端末を描く領域に、タブバーをその上に描いて表示する。スクロールバーは、描くたびに
    /// 読んだ表示している範囲の位置で描く（出力、ウィンドウの大きさの変更、タブの切り替えなど、描き直す
    /// ことはどれも位置を動かしうる）。
    /// `capture` を渡すと、表示する前の画面を PNG で書き出す。
    pub(super) fn render_to(&mut self, capture: Option<&std::path::Path>) {
        let position = self.scroll_position();
        self.tick_image_animations();
        let mut frame = match self.session_mut().vt.frame() {
            Ok(frame) => frame,
            Err(e) => {
                tracing::warn!("failed to read the screen: {e}");
                return;
            }
        };
        self.set_blinking(self.window_active && frame.has_blink());
        if !self.blink_visible {
            frame.hide_blinking_text();
        }
        let mut paints = self.tab_bar_paints();
        let scroll_bar_colors =
            scroll_bar::Palette::from_terminal(frame.background, frame.foreground)
                .with_colors(&self.config.colors.scroll_bar);
        let over_image = self.session().background_image.is_some();
        if let Some(position) = position {
            paints.extend(self.scroll_bar_paints(position, &scroll_bar_colors, over_image));
        }
        let backdrop = self.sessions[self.active]
            .background_image
            .as_ref()
            .map(|image| {
                let (fit, image_opacity) = self.config.background.settings_for(image.entry);
                Backdrop {
                    image: &image.path,
                    region: self.backdrop_area,
                    fit,
                    image_opacity,
                }
            });
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let session = &self.sessions[self.active];
        renderer.prepare_terminal_images(session.id().0, &frame.images, &session.vt);
        match renderer.render(
            &[(&frame, self.terminal_area)],
            backdrop.as_ref(),
            &paints,
            capture,
        ) {
            Ok(()) => self.renderer_recreations = 0,
            Err(e) if renderer::is_device_lost(&e) => self.on_device_lost(&e),
            Err(e) => tracing::warn!("failed to render: {e}"),
        }
    }

    /// 表示している端末の画像のアニメーションを進め、次のコマで描き直すタイマーを動かす。再生中の
    /// ものがなければ止める。
    fn tick_image_animations(&mut self) {
        let now = self.clock.elapsed();
        let next = match self.session_mut().vt.tick_image_animations(now) {
            Ok(next) => next,
            Err(e) => {
                tracing::warn!("failed to advance the image animations: {e}");
                None
            }
        };
        let Some(delay) = next else {
            if self.image_animation_timer_running {
                self.stop_image_animation_timer();
            }
            return;
        };
        let delay = delay.max(MIN_IMAGE_ANIMATION_INTERVAL);
        let millis = u32::try_from(delay.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: self.hwnd は有効なウィンドウ。同じ番号のタイマーは置き換わる。
        let started =
            unsafe { SetTimer(Some(self.hwnd), IMAGE_ANIMATION_TIMER_ID, millis, None) } != 0;
        if !started {
            tracing::warn!(
                "failed to start the image animation timer: {}",
                windows::core::Error::from_thread()
            );
        }
        self.image_animation_timer_running = started;
    }

    fn stop_image_animation_timer(&mut self) {
        // SAFETY: self.hwnd は有効なウィンドウ。
        if let Err(e) = unsafe { KillTimer(Some(self.hwnd), IMAGE_ANIMATION_TIMER_ID) } {
            tracing::warn!("failed to stop the image animation timer: {e}");
        }
        self.image_animation_timer_running = false;
    }

    /// 画像のアニメーションの次のコマの時刻になった。一度だけのタイマーとして止めて描き直す。
    pub(super) fn on_image_animation_timer(&mut self) {
        self.stop_image_animation_timer();
        self.invalidate();
    }

    /// 点滅のタイマーを動かす・止める。止めたら点滅の文字を見せた状態に戻す。動かし始めも見せた状態から。
    fn set_blinking(&mut self, blinking: bool) {
        if blinking == self.blink_timer_running {
            return;
        }
        self.blink_visible = true;
        if blinking {
            // SAFETY: self.hwnd は有効なウィンドウ。
            let started = unsafe {
                SetTimer(
                    Some(self.hwnd),
                    BLINK_TIMER_ID,
                    BLINK_INTERVAL.as_millis() as u32,
                    None,
                )
            } != 0;
            if !started {
                tracing::warn!(
                    "failed to start the blink timer: {}",
                    windows::core::Error::from_thread()
                );
                return;
            }
        } else {
            // SAFETY: self.hwnd は有効なウィンドウ。
            if let Err(e) = unsafe { KillTimer(Some(self.hwnd), BLINK_TIMER_ID) } {
                tracing::warn!("failed to stop the blink timer: {e}");
            }
        }
        self.blink_timer_running = blinking;
        tracing::debug!("blink timer running: {blinking}");
    }

    /// 点滅のタイマーが来た。見せる・隠すを切り替えて描き直す。
    pub(super) fn on_blink_timer(&mut self) {
        self.blink_visible = !self.blink_visible;
        self.invalidate();
    }

    /// ウィンドウがアクティブになった・でなくなった。アクティブでない間は点滅を止め、文字を見せておく。
    /// 点滅を始めるかどうかは、次に描くときに画面に点滅の文字があるかで決める。
    pub(super) fn on_activate(&mut self, active: bool) {
        self.window_active = active;
        if !active {
            let was_hidden = !self.blink_visible;
            self.set_blinking(false);
            if !was_hidden {
                return;
            }
        }
        self.invalidate();
    }
}

/// デバイスを失ったと知らされたときに、`Renderer` を作り直すきっかけにするエラー。`removed` は今の
/// `Renderer` のデバイスを失ったか（[`Renderer::device_removed`]）で、`Renderer` がなければ `None`。
///
/// 作り直すのは、今の `Renderer` のデバイスを失っているときだけ。`Renderer` がないときは、作り直しを
/// タイマーで待っているか描画をやめたあとで、失っていないときは、知らせたデバイスの `Renderer` を描くときに
/// 気づいてすでに作り直したあと。
fn lost_device_to_recreate(
    removed: Option<windows::core::Result<()>>,
) -> Option<windows::core::Error> {
    removed?.err()
}

/// `Renderer` がデバイスを失ったときに呼ぶもの。`hwnd` に [`WM_APP_DEVICE_REMOVED`] を積む。ウィンドウの
/// ハンドルの値だけを持つ。`Renderer` を捨てるときに呼ばれなくなるので、ウィンドウを壊している途中に
/// 積むのに失敗することがあるだけ。
pub(super) fn device_removed_poster(hwnd: HWND) -> renderer::DeviceRemovedNotify {
    Box::new(poster(hwnd, WM_APP_DEVICE_REMOVED, WPARAM(0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_of_a_removed_device_recreates_only_a_renderer_that_lost_its_device() {
        use windows::Win32::Graphics::Dxgi::{DXGI_ERROR_DEVICE_HUNG, DXGI_ERROR_DEVICE_REMOVED};

        // 作り直しをタイマーで待っているか、描画をやめたあと。
        assert!(lost_device_to_recreate(None).is_none());
        // 描くときに気づいて、すでに作り直したあと。
        assert!(lost_device_to_recreate(Some(Ok(()))).is_none());
        // 今の Renderer のデバイスを失っている。理由は描くときに気づくエラーに限らない。
        for code in [DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_HUNG] {
            let error = lost_device_to_recreate(Some(Err(code.into())));
            assert_eq!(error.map(|error| error.code()), Some(code));
        }
    }
}
