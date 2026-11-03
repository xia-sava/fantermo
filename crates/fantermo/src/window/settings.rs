//! 設定。設定ファイルの読み直し、設定の入れ替え、設定ダイアログ。

use std::path::PathBuf;

use fantermo_core::config::{Config, FontConfig, WideGlyph};
use windows::Win32::Foundation::HWND;

use super::{
    App,
    proc::{Deferred, WithApp, run_deferred},
    tab_bar::system_colors,
};
use crate::{
    config_file::{self, Loaded},
    font::{self, Font},
    notice::{self, Topic},
    renderer,
    settings_dialog::{self, FontChoice},
    tab_bar,
};

impl App {
    /// 設定ファイルを読み直し、[`App::apply_config`] で入れ替える。
    /// エラーがあれば、それを見せることを返す（借用を手放してから [`run_deferred`] で見せる）。前に読んだ
    /// ときと同じエラーなら見せない。エラーがなければ、設定の警告を取り下げる（開いているメッセージボックス
    /// も閉じさせる）。
    ///
    /// 設定ダイアログを開いている間は読まず、閉じてから読む。
    fn reload_config(&mut self) -> Option<Deferred> {
        if self.settings_dialog_open {
            self.reload_pending = true;
            return None;
        }
        let Loaded {
            config,
            notice,
            notice_key,
        } = config_file::reload(self.config_path.as_deref(), &self.config);
        let warn = match notice {
            Some(_) if notice_key == self.settings_notice_key => None,
            Some(text) => Some(Deferred::Warn {
                topic: Topic::Settings,
                text,
            }),
            None => {
                notice::withdraw(self.hwnd, Topic::Settings);
                None
            }
        };
        self.settings_notice_key = notice_key;
        self.apply_config(config);
        warn
    }

    /// 読み直した設定、または設定ダイアログで編集中の設定を入れ替える。
    ///
    /// フォントの設定が変わっていて、そのフォントを測れたら、フォントとアトラスを作り直す。フォント、
    /// スクロールバーの幅、余白が変わったら、ウィンドウの位置と大きさは変えず、いまのクライアント領域で
    /// 端末の桁数と行数を決め直す。グリフの描き方だけが変わったときは、セルの寸法は変わらないので、
    /// アトラスだけを作り直す。貼り付けの設定はすぐに入れ替える。プロファイルは次に開く端末から使う。
    fn apply_config(&mut self, config: Config) {
        // フォントは、測れたものだけを `apply_font_config` が入れ替える。
        let new_font = config.font.clone();
        let font_in_use = self.config.font.clone();
        let old = std::mem::replace(
            &mut self.config,
            Config {
                font: font_in_use,
                ..config
            },
        );
        if self.config.background != old.background {
            // 項目ごとの表示方法と不透明度だけが変わったなら、選んだ画像のまま描き直す（描くときに項目から
            // 決める）。
            if !self.config.background.same_paths(&old.background) {
                self.rechoose_background_images();
            }
            self.invalidate();
        }
        if self.config.colors != old.colors {
            let terminal_colors = self.config.colors.terminal_colors();
            if terminal_colors != old.colors.terminal_colors() {
                for session in &mut self.sessions {
                    if let Err(e) = session.vt.set_default_colors(&terminal_colors) {
                        tracing::warn!("failed to change the terminal colors: {e}");
                    }
                }
            }
            self.invalidate();
        }
        let insets_changed = self.config.scroll_bar.width != old.scroll_bar.width
            || self.config.window.padding != old.window.padding;
        let font = self.apply_font_config(new_font);
        let font_changed = font.is_some();
        if let Some(font) = font {
            self.apply_font(font);
        }
        if font_changed || insets_changed {
            self.fit_to_client();
            self.invalidate();
        }
    }

    /// 読み直したフォントの設定を入れ替える。フォントを測り直す作り直しが要れば、いまの DPI で測った
    /// フォントを返す（作り直しは呼び出し元が [`App::apply_font`] で行う）。
    fn apply_font_config(&mut self, font: FontConfig) -> Option<Font> {
        if font == self.config.font {
            return None;
        }
        let same_face =
            font.families == self.config.font.families && font.size_pt == self.config.font.size_pt;
        if same_face {
            self.set_wide_glyph(font.wide_glyph);
            return None;
        }
        let measured = match font::find_or_default(&self.dwrite, &font, self.dpi) {
            Ok(measured) => measured,
            Err(e) => {
                tracing::warn!("keeping the current font: {e}");
                return None;
            }
        };
        self.config.font = font;
        Some(measured)
    }

    /// 設定ダイアログに渡すもの。いま使っている設定と、フォントの一覧。
    fn settings_request(&mut self) -> settings_dialog::Request {
        let current = self.config.clone();
        let tab_bar_colors = tab_bar::Palette::from_system(system_colors()).colors();
        let fonts = match font::monospaced_families(&self.dwrite) {
            Ok(families) => families
                .into_iter()
                .map(|family| FontChoice {
                    name: family.name,
                    index: Some(family.index),
                })
                .collect(),
            Err(e) => {
                tracing::warn!("failed to list the fonts: {e}");
                Vec::new()
            }
        };
        let family_indices = current
            .font
            .families
            .iter()
            .map(|family| font::family_index(&self.dwrite, family))
            .collect();
        let font_in_use = FontChoice {
            name: self.font.family.clone(),
            index: font::family_index(&self.dwrite, &self.font.family),
        };
        settings_dialog::Request {
            current,
            family_indices,
            fonts,
            font_in_use,
            tab_bar_colors,
        }
    }

    /// セルより広いグリフの描き方を変え、グリフアトラスを作り直す。失敗したら前の描き方のまま。
    fn set_wide_glyph(&mut self, wide_glyph: WideGlyph) {
        // `Renderer` がないとき（作り直しを待つ間と、描画をやめたあと）と、デバイスを失ったときも、描き方は
        // 入れ替え、作り直す `Renderer` に使う。
        let rebuilt = match self.renderer.as_mut() {
            Some(renderer) => {
                renderer.set_font(&self.dwrite, &self.font, self.ui_font.clone(), wide_glyph)
            }
            None => Ok(()),
        };
        let lost = match rebuilt {
            Ok(()) => None,
            Err(e) if renderer::is_device_lost(&e) => Some(e),
            Err(e) => {
                tracing::warn!("failed to rebuild the glyph atlas for {wide_glyph:?}: {e}");
                return;
            }
        };
        self.config.font.wide_glyph = wide_glyph;
        if let Some(e) = lost {
            self.on_device_lost(&e);
        }
        self.invalidate();
    }
}

/// 設定ファイルを読み直して入れ替える。エラーがあれば見せる。
pub(super) fn reload_settings(hwnd: HWND, with_app: &WithApp) {
    let mut deferred = None;
    with_app(&mut |app| deferred = app.reload_config());
    run_deferred(hwnd, deferred, with_app);
}

/// 設定ダイアログの編集を端末と設定ファイルにつなぐもの。
struct SettingsHost<'a, 'b> {
    with_app: &'a WithApp<'b>,
    config_path: PathBuf,
    /// 「OK」か「適用」で保存した。
    saved: bool,
}

impl settings_dialog::Host for SettingsHost<'_, '_> {
    fn preview(&mut self, config: &Config) {
        (self.with_app)(&mut |app| app.apply_config(config.clone()));
    }

    fn save(&mut self, base: &Config, edited: &Config) -> Result<(), String> {
        config_file::save(&self.config_path, base, edited)?;
        self.saved = true;
        Ok(())
    }
}

/// 設定ダイアログを開き、閉じるまで待つ。保存したか、開いている間に設定ファイルが書き換えられていたら、
/// 閉じてから読み直す。保存は変えた項目だけをファイルに書くので、開いている間に別のプログラムが書いた分は
/// ファイルに残っていて、読み直しで反映される。
pub(super) fn edit_settings(hwnd: HWND, with_app: &WithApp) {
    let mut opened = None;
    with_app(&mut |app| {
        let Some(path) = app.config_path.clone() else {
            tracing::warn!("the settings file location is unknown");
            return;
        };
        app.settings_dialog_open = true;
        opened = Some((path, app.settings_request()));
    });
    let Some((config_path, request)) = opened else {
        return;
    };
    let mut host = SettingsHost {
        with_app,
        config_path,
        saved: false,
    };
    settings_dialog::run(hwnd, request, &mut host);
    let mut pending = false;
    with_app(&mut |app| {
        app.settings_dialog_open = false;
        pending = std::mem::take(&mut app.reload_pending);
    });
    if pending || host.saved {
        reload_settings(hwnd, with_app);
    }
}
