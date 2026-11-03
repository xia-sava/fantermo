//! 設定ダイアログの色のページ。配色と、端末・タブバー・スクロールバーの色の表。

use std::cell::{Cell, RefCell};

use fantermo_core::{
    config::{self, ColorScheme, ColorTable, ColorsConfig, Config, format_color},
    frame::Rgb,
};
use windows::Win32::{
    Foundation::{COLORREF, HWND},
    Graphics::Gdi::{CreateSolidBrush, DeleteObject, HBRUSH, HGDIOBJ, InvalidateRect},
    UI::{
        Controls::Dialogs::{CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW},
        Input::KeyboardAndMouse::EnableWindow,
        WindowsAndMessaging::{BN_CLICKED, WINDOW_EX_STYLE, WS_EX_CLIENTEDGE},
    },
};

use super::{
    BUTTON_GAP, CONTROL_LEFT, DialogContext, LABEL_OFFSET, LABEL_WIDTH, MARGIN, PAGE_LEFT,
    PAGE_WIDTH, Page,
    widgets::{Builder, RadioGroup, set_text},
};
use crate::scroll_bar;

/// 色のページのコントロールの ID の始まり。色ごとに [`COLOR_ID_STRIDE`] ずつ、見本・値・選ぶ・既定に
/// 戻すの順に続く。
const ID_COLORS: i32 = 600;
const COLOR_ID_STRIDE: i32 = 4;
const COLOR_SWATCH: i32 = 0;
const COLOR_TEXT: i32 = 1;
const COLOR_CHOOSE: i32 = 2;
const COLOR_RESET: i32 = 3;
/// 色のページの一行の高さ。
const COLOR_ROW_HEIGHT: i32 = 22;
/// 色の名前を、表の見出しより右へ寄せる幅。
const COLOR_LABEL_INDENT: i32 = 4;
/// 色の見本の幅と、値の文字の幅。
const SWATCH_WIDTH: i32 = 32;
const COLOR_TEXT_WIDTH: i32 = 110;
const CHOOSE_WIDTH: i32 = 72;
const COLOR_RESET_WIDTH: i32 = 88;

/// 色の表の見出し。[`ColorTable::ALL`] の順。
const COLOR_TABLE_LABELS: [&str; ColorTable::ALL.len()] = ["端末", "タブバー", "スクロールバー"];

/// `colors.scheme` のラジオボタン。[`ColorScheme::ALL`] の順に ID が続く。
const ID_COLOR_SCHEME: i32 = 590;
/// `colors.scheme` の選択肢の表示。[`ColorScheme::ALL`] の順。
const COLOR_SCHEME_LABELS: [&str; ColorScheme::ALL.len()] = ["ダーク", "ライト"];
/// 配色のラジオボタン一つの幅。
const COLOR_SCHEME_WIDTH: i32 = 96;

/// 端末の色の表示。[`config::TERMINAL_COLOR_KEYS`] の順。
const TERMINAL_COLOR_LABELS: [&str; config::TERMINAL_COLOR_KEYS.len()] = ["背景色", "文字色"];

/// タブバーの色の表示。[`config::TAB_BAR_COLOR_KEYS`] の順。
const TAB_BAR_COLOR_LABELS: [&str; config::TAB_BAR_COLOR_KEYS.len()] = [
    "背景",
    "表示しているタブ",
    "表示しているタブの縁",
    "表示しているタブの文字",
    "表示していないタブの文字",
    "タブの区切り線",
    "マウスを載せているタブ",
    "「+」「▾」「≡」",
    "ベルの印",
];

/// スクロールバーの色の表示。[`config::SCROLL_BAR_COLOR_KEYS`] の順。
const SCROLL_BAR_COLOR_LABELS: [&str; config::SCROLL_BAR_COLOR_KEYS.len()] = [
    "トラック",
    "つまみ",
    "マウスを載せているつまみ",
    "ドラッグしているつまみ",
];

fn color_labels(table: ColorTable) -> &'static [&'static str] {
    match table {
        ColorTable::Terminal => &TERMINAL_COLOR_LABELS,
        ColorTable::TabBar => &TAB_BAR_COLOR_LABELS,
        ColorTable::ScrollBar => &SCROLL_BAR_COLOR_LABELS,
    }
}

/// 色のページの色の数。
const COLOR_ITEM_COUNT: usize = config::TERMINAL_COLOR_KEYS.len()
    + config::TAB_BAR_COLOR_KEYS.len()
    + config::SCROLL_BAR_COLOR_KEYS.len();

/// 色のページの色の並び。テーブルと、テーブルの中の位置。テーブルは [`ColorTable::ALL`] の順。
const COLOR_ITEMS: [(ColorTable, usize); COLOR_ITEM_COUNT] = {
    let tables = [
        (ColorTable::Terminal, config::TERMINAL_COLOR_KEYS.len()),
        (ColorTable::TabBar, config::TAB_BAR_COLOR_KEYS.len()),
        (ColorTable::ScrollBar, config::SCROLL_BAR_COLOR_KEYS.len()),
    ];
    let mut items = [(ColorTable::Terminal, 0); COLOR_ITEM_COUNT];
    let mut item = 0;
    let mut table = 0;
    while table < tables.len() {
        let mut index = 0;
        while index < tables[table].1 {
            items[item] = (tables[table].0, index);
            item += 1;
            index += 1;
        }
        table += 1;
    }
    items
};

/// 色を Win32 の `COLORREF`（`0x00BBGGRR`）にする。
fn colorref(color: Rgb) -> COLORREF {
    COLORREF(u32::from(color.r) | (u32::from(color.g) << 8) | (u32::from(color.b) << 16))
}

/// Win32 の `COLORREF` を色にする。
fn rgb_of(colorref: COLORREF) -> Rgb {
    let [r, g, b, _] = colorref.0.to_le_bytes();
    Rgb { r, g, b }
}

/// 色 `colors` のときの、テーブル `table` の `index` 番目の既定の色（設定に書いていないときに使う色）。
/// 端末の色は配色から、スクロールバーの色は端末の背景色と文字の色から、タブバーの色は
/// `tab_bar_defaults`（システムの色）から決める。
fn default_color(
    colors: &ColorsConfig,
    tab_bar_defaults: &[Rgb; config::TAB_BAR_COLOR_KEYS.len()],
    table: ColorTable,
    index: usize,
) -> Rgb {
    match table {
        ColorTable::Terminal => {
            let scheme = colors.scheme.colors();
            [scheme.background, scheme.foreground][index]
        }
        ColorTable::TabBar => tab_bar_defaults[index],
        ColorTable::ScrollBar => {
            let terminal = colors.terminal_colors();
            scroll_bar::Palette::from_terminal(terminal.background, terminal.foreground).colors()
                [index]
        }
    }
}

/// 色を選ぶダイアログの「作成した色」の数。
const CUSTOM_COLORS: usize = 16;

/// 色のページの一行のコントロール。
struct ColorRow {
    swatch: HWND,
    text: HWND,
    reset: HWND,
}

/// 色のページ。配色と、端末・タブバー・スクロールバーの色の表。
pub(super) struct ColorsPage {
    controls: Vec<HWND>,
    /// 配色のラジオボタン。
    color_scheme: RadioGroup<ColorScheme>,
    /// 色のページの行。[`COLOR_ITEMS`] の順。
    color_rows: Vec<ColorRow>,
    /// 編集中の色。
    colors: RefCell<ColorsConfig>,
    /// 書いていないタブバーの色として見せる色。
    tab_bar_defaults: [Rgb; config::TAB_BAR_COLOR_KEYS.len()],
    /// 色の見本を塗るブラシ。[`COLOR_ITEMS`] の順。
    swatch_brushes: RefCell<Vec<HBRUSH>>,
    /// 色を選ぶダイアログの「作成した色」。ダイアログを開いている間は覚えておく。
    custom_colors: Cell<[COLORREF; CUSTOM_COLORS]>,
}

impl ColorsPage {
    /// ページのコントロールを作る。`tab_bar_defaults` は、書いていないタブバーの色として見せる色。
    pub(super) fn create(
        b: &mut Builder,
        tab_bar_defaults: [Rgb; config::TAB_BAR_COLOR_KEYS.len()],
    ) -> windows::core::Result<Self> {
        let mut controls = Vec::new();
        let mut color_rows = Vec::new();
        // 端末の色の表の見出しの下に作る。
        let mut color_scheme = None;
        let mut top = MARGIN;
        for (table_index, table) in ColorTable::ALL.into_iter().enumerate() {
            controls.push(b.label(
                COLOR_TABLE_LABELS[table_index],
                [PAGE_LEFT, top + LABEL_OFFSET, PAGE_WIDTH, COLOR_ROW_HEIGHT],
            )?);
            top += COLOR_ROW_HEIGHT;
            if table == ColorTable::Terminal {
                controls.push(b.label(
                    "配色(&M):",
                    [
                        PAGE_LEFT + COLOR_LABEL_INDENT,
                        top + LABEL_OFFSET,
                        LABEL_WIDTH - COLOR_LABEL_INDENT,
                        COLOR_ROW_HEIGHT,
                    ],
                )?);
                let scheme = RadioGroup::create(
                    b,
                    ID_COLOR_SCHEME,
                    &ColorScheme::ALL,
                    &COLOR_SCHEME_LABELS,
                    |index| {
                        [
                            CONTROL_LEFT + index * COLOR_SCHEME_WIDTH,
                            top,
                            COLOR_SCHEME_WIDTH,
                            COLOR_ROW_HEIGHT,
                        ]
                    },
                )?;
                controls.extend_from_slice(scheme.buttons());
                color_scheme = Some(scheme);
                top += COLOR_ROW_HEIGHT;
            }
            for label_text in color_labels(table) {
                let id = ID_COLORS + color_rows.len() as i32 * COLOR_ID_STRIDE;
                controls.push(b.label(
                    label_text,
                    [
                        PAGE_LEFT + COLOR_LABEL_INDENT,
                        top + LABEL_OFFSET,
                        LABEL_WIDTH - COLOR_LABEL_INDENT,
                        COLOR_ROW_HEIGHT,
                    ],
                )?);
                let swatch = b.static_text(
                    id + COLOR_SWATCH,
                    WS_EX_CLIENTEDGE,
                    [CONTROL_LEFT, top + 2, SWATCH_WIDTH, COLOR_ROW_HEIGHT - 4],
                )?;
                let mut left = CONTROL_LEFT + SWATCH_WIDTH + BUTTON_GAP;
                let text = b.static_text(
                    id + COLOR_TEXT,
                    WINDOW_EX_STYLE(0),
                    [left, top + LABEL_OFFSET, COLOR_TEXT_WIDTH, COLOR_ROW_HEIGHT],
                )?;
                left += COLOR_TEXT_WIDTH + BUTTON_GAP;
                let choose = b.button(
                    id + COLOR_CHOOSE,
                    "選択…",
                    [left, top, CHOOSE_WIDTH, COLOR_ROW_HEIGHT - 2],
                )?;
                left += CHOOSE_WIDTH + BUTTON_GAP;
                let reset = b.button(
                    id + COLOR_RESET,
                    "既定に戻す",
                    [left, top, COLOR_RESET_WIDTH, COLOR_ROW_HEIGHT - 2],
                )?;
                controls.extend([swatch, text, choose, reset]);
                color_rows.push(ColorRow {
                    swatch,
                    text,
                    reset,
                });
                top += COLOR_ROW_HEIGHT;
            }
        }
        Ok(Self {
            controls,
            color_scheme: color_scheme.expect("the terminal colors are listed"),
            color_rows,
            colors: RefCell::new(ColorsConfig::default()),
            tab_bar_defaults,
            swatch_brushes: RefCell::new(vec![HBRUSH::default(); COLOR_ITEMS.len()]),
            custom_colors: Cell::new([COLORREF(0); CUSTOM_COLORS]),
        })
    }

    /// 色のページのすべての色を、編集中の色に合わせる（配色や端末の色を変えると、ほかの色の既定の
    /// 色も変わる）。
    fn refresh_colors(&self) {
        for item in 0..self.color_rows.len() {
            self.refresh_color(item);
        }
    }

    /// テーブル `table` の `index` 番目の色の、編集中の色での既定の色。
    fn default_color(&self, table: ColorTable, index: usize) -> Rgb {
        default_color(&self.colors.borrow(), &self.tab_bar_defaults, table, index)
    }

    /// 色のページの `item` 番目の色の、見本・値・既定に戻すボタンを、編集中の色に合わせる。
    fn refresh_color(&self, item: usize) {
        let (table, index) = COLOR_ITEMS[item];
        let written = self.colors.borrow().table(table)[index];
        let color = written.unwrap_or(self.default_color(table, index));
        let row = &self.color_rows[item];
        let text = match written {
            Some(_) => format_color(color),
            None => format!("{}（既定）", format_color(color)),
        };
        set_text(row.text, &text);
        // SAFETY: 色を渡してブラシを作るだけ。
        let brush = unsafe { CreateSolidBrush(colorref(color)) };
        let previous = std::mem::replace(&mut self.swatch_brushes.borrow_mut()[item], brush);
        // SAFETY: row のコントロールはダイアログの子の有効なウィンドウ。
        unsafe {
            _ = EnableWindow(row.reset, written.is_some());
            _ = InvalidateRect(Some(row.swatch), None, true);
        }
        if !previous.is_invalid() {
            // SAFETY: 前のブラシは、描き直しを頼んだ見本がもう使わない（WM_CTLCOLORSTATIC で渡すのは
            // いまのブラシ）。
            unsafe { _ = DeleteObject(HGDIOBJ(previous.0)) };
        }
    }

    /// 色のページのボタン `id` が押された。
    fn on_color_button(&self, ctx: &dyn DialogContext, id: i32) {
        let offset = id - ID_COLORS;
        let item = (offset / COLOR_ID_STRIDE) as usize;
        let Some(&(table, index)) = COLOR_ITEMS.get(item) else {
            return;
        };
        let new = match offset % COLOR_ID_STRIDE {
            COLOR_CHOOSE => {
                let written = self.colors.borrow().table(table)[index];
                let current = written.unwrap_or(self.default_color(table, index));
                let Some(chosen) = self.choose_color(ctx, current) else {
                    return;
                };
                Some(chosen)
            }
            COLOR_RESET => None,
            _ => return,
        };
        self.colors.borrow_mut().table_mut(table)[index] = new;
        self.refresh_colors();
        ctx.preview();
    }

    /// 配色のラジオボタンが押された。
    fn on_color_scheme(&self, ctx: &dyn DialogContext) {
        let Some(scheme) = self.color_scheme.read() else {
            return;
        };
        self.colors.borrow_mut().scheme = scheme;
        self.refresh_colors();
        ctx.preview();
    }

    /// 色を選ぶダイアログを、`current` を選んだ状態で開く。やめたら `None`。
    fn choose_color(&self, ctx: &dyn DialogContext, current: Rgb) -> Option<Rgb> {
        let mut custom = self.custom_colors.get();
        let mut choose = CHOOSECOLORW {
            lStructSize: size_of::<CHOOSECOLORW>() as u32,
            hwndOwner: ctx.hwnd(),
            rgbResult: colorref(current),
            lpCustColors: custom.as_mut_ptr(),
            Flags: CC_RGBINIT | CC_FULLOPEN,
            ..Default::default()
        };
        // SAFETY: choose は初期化済みで、custom はこの呼び出しの間は生きている 16 個の COLORREF。
        // 借用は持っていない。
        let chosen = unsafe { ChooseColorW(&mut choose) }.as_bool();
        self.custom_colors.set(custom);
        chosen.then(|| rgb_of(choose.rgbResult))
    }

    /// 色の見本 `control` を塗るブラシ。色の見本でなければ `None`。
    pub(super) fn swatch_brush(&self, control: HWND) -> Option<HBRUSH> {
        let item = self
            .color_rows
            .iter()
            .position(|row| row.swatch == control)?;
        self.swatch_brushes.borrow().get(item).copied()
    }

    /// 色の見本を塗るブラシを消す。ダイアログが閉じてから呼ぶ。
    pub(super) fn delete_swatch_brushes(&self) {
        for brush in self.swatch_brushes.take() {
            if !brush.is_invalid() {
                // SAFETY: ブラシは色の見本のために作ったもので、ダイアログはもう閉じている。
                unsafe { _ = DeleteObject(HGDIOBJ(brush.0)) };
            }
        }
    }
}

impl Page for ColorsPage {
    fn controls(&self) -> &[HWND] {
        &self.controls
    }

    fn fill(&self, _ctx: &dyn DialogContext, config: &Config) {
        *self.colors.borrow_mut() = config.colors.clone();
        self.color_scheme.fill(config.colors.scheme);
        self.refresh_colors();
    }

    fn read(
        &self,
        _ctx: &dyn DialogContext,
        config: &mut Config,
    ) -> Result<(), (&'static str, HWND)> {
        config.colors = self.colors.borrow().clone();
        Ok(())
    }

    fn on_command(&self, ctx: &dyn DialogContext, id: i32, code: u32) -> bool {
        match (id, code) {
            (id, BN_CLICKED) if id >= ID_COLORS => self.on_color_button(ctx, id),
            (id, BN_CLICKED) if self.color_scheme.owns(id) => self.on_color_scheme(ctx),
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::{BUTTON_HEIGHT, CLIENT_HEIGHT, CLIENT_WIDTH, PAGES_WIDTH};
    use super::*;

    #[test]
    fn colorrefs_put_red_in_the_low_byte() {
        let color = Rgb {
            r: 0x12,
            g: 0x34,
            b: 0x56,
        };
        assert_eq!(colorref(color), COLORREF(0x0056_3412));
        assert_eq!(rgb_of(COLORREF(0x0056_3412)), color);
    }

    #[test]
    fn every_color_has_a_label_and_fits_above_the_buttons() {
        for table in ColorTable::ALL {
            assert_eq!(color_labels(table).len(), table.keys().len());
        }
        assert_eq!(COLOR_TABLE_LABELS.len(), ColorTable::ALL.len());
        assert_eq!(COLOR_SCHEME_LABELS.len(), ColorScheme::ALL.len());
        let items = COLOR_ITEMS;
        let expected: Vec<(ColorTable, usize)> = ColorTable::ALL
            .iter()
            .flat_map(|&table| (0..table.keys().len()).map(move |index| (table, index)))
            .collect();
        assert_eq!(items.to_vec(), expected);
        assert_eq!(items.len(), 15);
        assert_eq!(items[0], (ColorTable::Terminal, 0));
        assert_eq!(items[11], (ColorTable::ScrollBar, 0));
        // 表の見出しと配色の行と色の行。
        let rows = (ColorTable::ALL.len() + 1 + items.len()) as i32;
        let button_top = CLIENT_HEIGHT - MARGIN - BUTTON_HEIGHT;
        assert!(MARGIN + rows * COLOR_ROW_HEIGHT <= button_top - MARGIN);
        let row_width = LABEL_WIDTH
            + SWATCH_WIDTH
            + COLOR_TEXT_WIDTH
            + CHOOSE_WIDTH
            + COLOR_RESET_WIDTH
            + 3 * BUTTON_GAP;
        assert!(row_width <= CLIENT_WIDTH - (MARGIN + PAGES_WIDTH + MARGIN) - MARGIN);
    }

    #[test]
    fn default_colors_follow_the_scheme_and_the_terminal_colors() {
        let tab_bar = [Rgb { r: 9, g: 9, b: 9 }; config::TAB_BAR_COLOR_KEYS.len()];
        let light = ColorsConfig {
            scheme: ColorScheme::Light,
            ..ColorsConfig::default()
        };
        let scheme = config::LIGHT_COLORS;
        assert_eq!(
            default_color(&light, &tab_bar, ColorTable::Terminal, 0),
            scheme.background
        );
        assert_eq!(
            default_color(&light, &tab_bar, ColorTable::Terminal, 1),
            scheme.foreground
        );
        assert_eq!(
            default_color(&light, &tab_bar, ColorTable::TabBar, 3),
            tab_bar[3]
        );
        let thumb =
            |colors: &ColorsConfig| default_color(colors, &tab_bar, ColorTable::ScrollBar, 1);
        assert_eq!(
            thumb(&light),
            scroll_bar::Palette::from_terminal(scheme.background, scheme.foreground).colors()[1]
        );
        // 書いた背景色は、端末の色の既定には効かず、スクロールバーの既定には効く。
        let black = Rgb { r: 0, g: 0, b: 0 };
        let written = ColorsConfig {
            terminal: [Some(black), None],
            ..light.clone()
        };
        assert_eq!(
            default_color(&written, &tab_bar, ColorTable::Terminal, 0),
            scheme.background
        );
        assert_eq!(
            thumb(&written),
            scroll_bar::Palette::from_terminal(black, scheme.foreground).colors()[1]
        );
    }
}
