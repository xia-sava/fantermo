//! 設定ダイアログの外観のページ。フォント、サイズ、セルより広い文字、スクロールバーの幅、余白。

use std::cell::{Cell, RefCell};

use fantermo_core::config::{self, Config, WideGlyph};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        UI::{
            Controls::CB_SETMINVISIBLE,
            WindowsAndMessaging::{
                BN_CLICKED, CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CBN_SELCHANGE, EN_CHANGE,
                SendMessageW,
            },
        },
    },
    core::HSTRING,
};

use super::{
    BUTTON_GAP, CONTROL_LEFT, CONTROL_WIDTH, DialogContext, FontChoice, LABEL_OFFSET, MARGIN,
    NUMBER_WIDTH, Page, ROW_HEIGHT, label_rect, row,
    widgets::{
        Builder, NumberField, NumberInput, NumberKind, RadioGroup, format_integer, step_integer,
    },
};

/// このページのコントロールの ID の範囲。範囲がページの間で重ならないことと、このページの ID が範囲に
/// 入ることを単体のテストで見る。
#[cfg(test)]
pub(super) const ID_RANGE: std::ops::RangeInclusive<i32> = 200..=239;
const ID_FONT: i32 = 200;
const ID_SIZE: i32 = 201;
const ID_SIZE_SPIN: i32 = 202;
/// `font.wide_glyph` のラジオボタン。[`WideGlyph::ALL`] の順に ID が続く。
const ID_WIDE_GLYPH: i32 = 210;
const ID_SCROLL_BAR_WIDTH: i32 = 220;
const ID_SCROLL_BAR_WIDTH_SPIN: i32 = 221;
const ID_PADDING: i32 = 230;
const ID_PADDING_SPIN: i32 = 231;

/// フォントの一覧を開いたときに見せる行の数。
const VISIBLE_FONTS: usize = 16;

/// `font.wide_glyph` の選択肢の表示。[`WideGlyph::ALL`] の順。
const WIDE_GLYPH_LABELS: [&str; WideGlyph::ALL.len()] = [
    "横に縮めてセルに収める",
    "縮めずに、はみ出た部分を切り取る",
    "縮めずに、右のセルへはみ出す",
    "縮めずに、中央から左右のセルへはみ出す",
];

/// `font.size_pt` の入力欄。
pub(super) const FONT_SIZE_KIND: NumberKind<f32> = NumberKind {
    parse: |text| config::parse_input("font.size", text),
    format: format_size,
    step: step_size,
    start: |config| config.font.size_pt,
    message: "フォントサイズは、1 から 128 までの数値を入力してください。",
};

/// `scroll_bar.width` の入力欄。
pub(super) const SCROLL_BAR_WIDTH_KIND: NumberKind<u32> = NumberKind {
    parse: |text| config::parse_input("scroll_bar.width", text),
    format: format_integer,
    step: |current, delta| step_integer(current, delta, &config::SCROLL_BAR_WIDTH_RANGE),
    start: |config| config.scroll_bar.width,
    message: "スクロールバーの幅は、4 から 64 までの整数を入力してください。",
};

/// `window.padding` の入力欄。
pub(super) const PADDING_KIND: NumberKind<u32> = NumberKind {
    parse: |text| config::parse_input("window.padding", text),
    format: format_integer,
    step: |current, delta| step_integer(current, delta, &config::PADDING_RANGE),
    start: |config| config.window.padding,
    message: "余白は、0 から 64 までの整数を入力してください。",
};

/// 外観のページ。フォント、サイズ、セルより広い文字、スクロールバーの幅、余白。
pub(super) struct AppearancePage {
    controls: Vec<HWND>,
    font: HWND,
    size: NumberField<f32>,
    wide_glyph: RadioGroup<WideGlyph>,
    scroll_bar_width: NumberField<u32>,
    padding: NumberField<u32>,
    /// フォントの一覧。
    fonts: Vec<FontChoice>,
    /// 最後に書いたときの設定（[`DialogContext::base`]）の `font.families` のそれぞれの、システムの
    /// フォントの中の番号。
    base_indices: RefCell<Vec<Option<u32>>>,
    /// 最後に書いたときに選んでいたフォントの、一覧の中の位置。
    base_font: Cell<usize>,
}

impl AppearancePage {
    /// ページのコントロールを作る。フォントの一覧 `fonts` の `selected_font` 番目が、いま使っている
    /// フォント（`family_indices` は `font.families` のそれぞれの、システムのフォントの中の番号）。
    pub(super) fn create(
        b: &mut Builder,
        fonts: Vec<FontChoice>,
        family_indices: Vec<Option<u32>>,
        selected_font: usize,
    ) -> windows::core::Result<Self> {
        let mut controls = Vec::new();
        controls.push(b.label("フォント(&F):", label_rect(row(0)))?);
        let font = b.combo(
            ID_FONT,
            [CONTROL_LEFT, row(0), CONTROL_WIDTH, ROW_HEIGHT],
            std::iter::empty::<&str>(),
        )?;
        controls.push(font);
        // SAFETY: font は有効なコンボボックス。
        unsafe { SendMessageW(font, CB_SETMINVISIBLE, Some(WPARAM(VISIBLE_FONTS)), None) };
        controls.push(b.label("サイズ(&S):", label_rect(row(1)))?);
        let size = NumberField::create(
            b,
            ID_SIZE,
            ID_SIZE_SPIN,
            [CONTROL_LEFT, row(1), NUMBER_WIDTH, ROW_HEIGHT],
            FONT_SIZE_KIND,
        )?;
        controls.extend(size.controls());
        controls.push(b.label(
            "ポイント（1〜128）",
            [
                CONTROL_LEFT + NUMBER_WIDTH + BUTTON_GAP,
                row(1) + LABEL_OFFSET,
                CONTROL_WIDTH - NUMBER_WIDTH - BUTTON_GAP,
                ROW_HEIGHT,
            ],
        )?);
        controls.push(b.label("セルより広い文字(&W):", label_rect(row(2)))?);
        let wide_glyph = RadioGroup::create(
            b,
            ID_WIDE_GLYPH,
            &WideGlyph::ALL,
            &WIDE_GLYPH_LABELS,
            |index| {
                [
                    CONTROL_LEFT,
                    row(2) + index * ROW_HEIGHT,
                    CONTROL_WIDTH,
                    ROW_HEIGHT,
                ]
            },
        )?;
        controls.extend_from_slice(wide_glyph.buttons());
        let width_row = row(2) + WideGlyph::ALL.len() as i32 * ROW_HEIGHT + MARGIN;
        controls.push(b.label("スクロールバーの幅(&B):", label_rect(width_row))?);
        let scroll_bar_width = NumberField::create(
            b,
            ID_SCROLL_BAR_WIDTH,
            ID_SCROLL_BAR_WIDTH_SPIN,
            [CONTROL_LEFT, width_row, NUMBER_WIDTH, ROW_HEIGHT],
            SCROLL_BAR_WIDTH_KIND,
        )?;
        controls.extend(scroll_bar_width.controls());
        controls.push(b.label(
            "ピクセル（4〜64。画面の拡大率に合わせて広げる）",
            [
                CONTROL_LEFT + NUMBER_WIDTH + BUTTON_GAP,
                width_row + LABEL_OFFSET,
                CONTROL_WIDTH - NUMBER_WIDTH - BUTTON_GAP,
                ROW_HEIGHT,
            ],
        )?);
        let padding_row = width_row + ROW_HEIGHT + MARGIN;
        controls.push(b.label("余白(&G):", label_rect(padding_row))?);
        let padding = NumberField::create(
            b,
            ID_PADDING,
            ID_PADDING_SPIN,
            [CONTROL_LEFT, padding_row, NUMBER_WIDTH, ROW_HEIGHT],
            PADDING_KIND,
        )?;
        controls.extend(padding.controls());
        controls.push(b.label(
            "ピクセル（0〜64。画面の拡大率に合わせて広げる）",
            [
                CONTROL_LEFT + NUMBER_WIDTH + BUTTON_GAP,
                padding_row + LABEL_OFFSET,
                CONTROL_WIDTH - NUMBER_WIDTH - BUTTON_GAP,
                ROW_HEIGHT,
            ],
        )?);
        Ok(Self {
            controls,
            font,
            size,
            wide_glyph,
            scroll_bar_width,
            padding,
            fonts,
            base_indices: RefCell::new(family_indices),
            base_font: Cell::new(selected_font),
        })
    }

    /// 設定ファイルに書けた。フォントを選び直していれば、選んでいるフォントを最後に書いたときの
    /// フォントにする。`base` は書く前の、最後に書いたときの設定。
    pub(super) fn saved(&self, base: &Config) {
        // SAFETY: コンボボックスは有効なウィンドウ。
        let selected = unsafe { SendMessageW(self.font, CB_GETCURSEL, None, None) }.0;
        if let Ok(selected) = usize::try_from(selected)
            && selected != self.base_font.get()
            && let Some(choice) = self.fonts.get(selected)
        {
            let indices =
                families_choosing(&base.font.families, &self.base_indices.borrow(), choice)
                    .into_iter()
                    .map(|(_, index)| index)
                    .collect();
            *self.base_indices.borrow_mut() = indices;
            self.base_font.set(selected);
        }
    }
}

impl Page for AppearancePage {
    fn controls(&self) -> &[HWND] {
        &self.controls
    }

    fn number_fields(&self) -> Vec<&dyn NumberInput> {
        vec![
            &self.size as &dyn NumberInput,
            &self.scroll_bar_width,
            &self.padding,
        ]
    }

    /// コントロールに設定 `config` と、一覧の最後に書いたときのフォントを入れる。
    fn fill(&self, _ctx: &dyn DialogContext, config: &Config) {
        for font in &self.fonts {
            // SAFETY: コンボボックスは有効なウィンドウで、文字列はこの呼び出しの間は生きている。
            unsafe {
                SendMessageW(
                    self.font,
                    CB_ADDSTRING,
                    None,
                    Some(LPARAM(HSTRING::from(font.name.as_str()).as_ptr() as isize)),
                )
            };
        }
        // SAFETY: コンボボックスは有効なウィンドウ。
        unsafe {
            SendMessageW(
                self.font,
                CB_SETCURSEL,
                Some(WPARAM(self.base_font.get())),
                None,
            )
        };
        self.size.fill(config.font.size_pt);
        self.wide_glyph.fill(config.font.wide_glyph);
        self.scroll_bar_width.fill(config.scroll_bar.width);
        self.padding.fill(config.window.padding);
    }

    fn read(
        &self,
        _ctx: &dyn DialogContext,
        config: &mut Config,
    ) -> Result<(), (&'static str, HWND)> {
        // SAFETY: コンボボックスは有効なウィンドウ。
        let selected = unsafe { SendMessageW(self.font, CB_GETCURSEL, None, None) }.0;
        if let Ok(selected) = usize::try_from(selected)
            && selected != self.base_font.get()
            && let Some(choice) = self.fonts.get(selected)
        {
            config.font.families =
                families_choosing(&config.font.families, &self.base_indices.borrow(), choice)
                    .into_iter()
                    .map(|(name, _)| name)
                    .collect();
        }
        config.font.size_pt = self.size.read()?;
        if let Some(wide_glyph) = self.wide_glyph.read() {
            config.font.wide_glyph = wide_glyph;
        }
        config.scroll_bar.width = self.scroll_bar_width.read()?;
        config.window.padding = self.padding.read()?;
        Ok(())
    }

    fn on_command(&self, ctx: &dyn DialogContext, id: i32, code: u32) -> bool {
        match (id, code) {
            (ID_FONT, CBN_SELCHANGE) => ctx.preview(),
            // ラジオボタンは、押したらすぐ反映する。
            (id, BN_CLICKED) if self.wide_glyph.owns(id) => ctx.preview(),
            // 数の入力欄は、打ち終わるのを待って反映する。
            (id, EN_CHANGE) if self.number_fields().iter().any(|field| field.owns(id)) => {
                ctx.schedule_preview();
            }
            _ => return false,
        }
        true
    }
}

/// フォントの名前の並び `families`（それぞれの番号は `indices`）で、`chosen` を先頭にした並び。同じ
/// 書体（名前か番号が同じもの）は後ろから除く。名前と番号の組で返す。
fn families_choosing(
    families: &[String],
    indices: &[Option<u32>],
    chosen: &FontChoice,
) -> Vec<(String, Option<u32>)> {
    let same = |name: &str, index: Option<u32>| {
        name.eq_ignore_ascii_case(&chosen.name) || (index.is_some() && index == chosen.index)
    };
    std::iter::once((chosen.name.clone(), chosen.index))
        .chain(
            families
                .iter()
                .enumerate()
                .map(|(i, name)| (name.clone(), indices.get(i).copied().flatten()))
                .filter(|(name, index)| !same(name, *index)),
        )
        .collect()
}

/// フォントの大きさを、矢印で一つ動かした値。整数の大きさへ動かし、範囲の端で止める。
fn step_size(current: f32, delta: i32) -> f32 {
    let next = if delta > 0 {
        current.floor() + 1.0
    } else {
        current.ceil() - 1.0
    };
    next.clamp(
        *config::FONT_SIZE_RANGE_PT.start(),
        *config::FONT_SIZE_RANGE_PT.end(),
    )
}

/// 入力欄に見せる、フォントの大きさ。整数なら小数点を付けない。
fn format_size(size_pt: f32) -> String {
    size_pt.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(name: &str, index: Option<u32>) -> FontChoice {
        FontChoice {
            name: name.to_owned(),
            index,
        }
    }

    #[test]
    fn chosen_family_goes_first_without_duplicates() {
        let families = ["Bizin Gothic", "BIZ UDGothic", "Consolas"].map(str::to_owned);
        let indices = [Some(96), Some(77), Some(5)];
        assert_eq!(
            families_choosing(&families, &indices, &choice("BIZ UDゴシック", Some(77))),
            [
                ("BIZ UDゴシック".to_owned(), Some(77)),
                ("Bizin Gothic".to_owned(), Some(96)),
                ("Consolas".to_owned(), Some(5)),
            ]
        );
        assert_eq!(
            families_choosing(&families, &indices, &choice("consolas", None)),
            [
                ("consolas".to_owned(), None),
                ("Bizin Gothic".to_owned(), Some(96)),
                ("BIZ UDGothic".to_owned(), Some(77)),
            ]
        );
        // 番号の分からない名前は、名前だけで比べる。
        assert_eq!(
            families_choosing(&families, &[], &choice("Cascadia Mono", Some(3))).len(),
            4
        );
    }

    #[test]
    fn arrows_step_to_whole_sizes_within_the_range() {
        assert_eq!(step_size(12.0, 1), 13.0);
        assert_eq!(step_size(12.0, -1), 11.0);
        assert_eq!(step_size(10.5, 1), 11.0);
        assert_eq!(step_size(10.5, -1), 10.0);
        assert_eq!(step_size(1.0, -1), 1.0);
        assert_eq!(step_size(128.0, 1), 128.0);
        let width = config::SCROLL_BAR_WIDTH_RANGE;
        assert_eq!(step_integer(12, 1, &width), 13);
        assert_eq!(step_integer(4, -1, &width), 4);
        assert_eq!(step_integer(64, 1, &width), 64);
        let padding = config::PADDING_RANGE;
        assert_eq!(step_integer(4, -1, &padding), 3);
        assert_eq!(step_integer(0, -1, &padding), 0);
        assert_eq!(step_integer(64, 1, &padding), 64);
    }

    #[test]
    fn the_ids_are_within_the_range_of_the_page() {
        let last_wide_glyph = ID_WIDE_GLYPH + WideGlyph::ALL.len() as i32 - 1;
        for id in [
            ID_FONT,
            ID_SIZE,
            ID_SIZE_SPIN,
            ID_WIDE_GLYPH,
            last_wide_glyph,
            ID_SCROLL_BAR_WIDTH,
            ID_SCROLL_BAR_WIDTH_SPIN,
            ID_PADDING,
            ID_PADDING_SPIN,
        ] {
            assert!(ID_RANGE.contains(&id), "{id}");
        }
    }

    #[test]
    fn sizes_are_shown_as_written() {
        assert_eq!(format_size(12.0), "12");
        assert_eq!(format_size(10.5), "10.5");
    }
}
