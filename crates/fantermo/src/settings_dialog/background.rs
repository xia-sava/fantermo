//! 設定ダイアログの背景のページ。上に既定の値の枠、下に背景画像の一覧と選択した項目の値の枠。

use std::{cell::RefCell, path::PathBuf};

use fantermo_core::{
    background::IMAGE_EXTENSIONS,
    config::{self, BackgroundFit, BackgroundImage, Config},
};
use windows::Win32::{
    Foundation::HWND,
    UI::{
        Input::KeyboardAndMouse::{EnableWindow, SetFocus},
        WindowsAndMessaging::{BN_CLICKED, CBN_SELCHANGE, EN_CHANGE, LBN_SELCHANGE},
    },
};

use super::{
    BUTTON_GAP, BUTTON_HEIGHT, DialogContext, LABEL_OFFSET, NUMBER_WIDTH, PAGE_LEFT, PAGE_WIDTH,
    Page, ROW_HEIGHT, filters_for, row,
    widgets::{
        Builder, ChoiceBox, ListBox, NumberField, NumberInput, NumberKind, RadioGroup, set_text,
    },
};
use crate::file_dialog;

/// このページのコントロールの ID の範囲。範囲がページの間で重ならないことと、このページの ID が範囲に
/// 入ることを単体のテストで見る。
#[cfg(test)]
pub(super) const ID_RANGE: std::ops::RangeInclusive<i32> = 450..=499;
const ID_BACKGROUND_IMAGES: i32 = 450;
const ID_BACKGROUND_ADD_FILES: i32 = 451;
const ID_BACKGROUND_ADD_FOLDER: i32 = 452;
const ID_BACKGROUND_REMOVE: i32 = 453;
const ID_BACKGROUND_OPACITY: i32 = 454;
const ID_BACKGROUND_OPACITY_SPIN: i32 = 455;
const ID_BACKGROUND_FIT: i32 = 456;
const ID_BACKGROUND_ITEM_FIT: i32 = 457;
const ID_BACKGROUND_ITEM_OPACITY: i32 = 458;
const ID_BACKGROUND_ITEM_OPACITY_SPIN: i32 = 459;
/// 項目の不透明度の「既定」と「指定」のラジオボタン。[`ITEM_OPACITY_CHOICES`] の順に ID が続く。
const ID_BACKGROUND_ITEM_OPACITY_CHOICE: i32 = 460;

/// 背景のページの枠（グループボックス）の、枠の線から中のコントロールまでの幅。上は見出しの分を足す。
const GROUP_PADDING: i32 = 10;
const GROUP_CAPTION_HEIGHT: i32 = 20;
/// 背景のページの枠の中の、見出しの列の幅。コントロールはその右に置く。
const BACKGROUND_LABEL_WIDTH: i32 = 190;
/// 背景画像の一覧の高さと、その右に並べるボタンの幅。
const BACKGROUND_LIST_HEIGHT: i32 = 106;
const BACKGROUND_BUTTON_WIDTH: i32 = 124;
/// 項目の不透明度の「指定」のラジオボタンの幅。入力欄はその右に置く。
const CUSTOM_OPACITY_WIDTH: i32 = 56;
/// 背景画像の不透明度を、上下の矢印で動かす幅。
const OPACITY_STEP: f32 = 0.05;

/// `background.fit` の選択肢の表示。[`BackgroundFit::ALL`] の順。Windows の壁紙の設定と同じ語。
const BACKGROUND_FIT_LABELS: [&str; BackgroundFit::ALL.len()] = [
    "ページ幅に合わせる",
    "画面のサイズに合わせる",
    "拡大して表示",
    "並べて表示",
    "中央に表示",
];

/// `fit` の表示。
fn fit_label(fit: BackgroundFit) -> &'static str {
    BackgroundFit::ALL
        .iter()
        .position(|&value| value == fit)
        .map_or("", |index| BACKGROUND_FIT_LABELS[index])
}

/// 一覧の項目の値を `[background]` の値に任せることの表示。`value` は任せる先の値の表示。
fn default_choice_label(value: &str) -> String {
    format!("既定（{value}）")
}

/// 背景画像の一覧の行。パスと、項目に書いた表示方法と不透明度（「C:\a.png（並べて表示、不透明度
/// 0.3）」）。
fn background_row_text(image: &BackgroundImage) -> String {
    let settings: Vec<String> = image
        .fit
        .map(|fit| fit_label(fit).to_owned())
        .into_iter()
        .chain(
            image
                .image_opacity
                .map(|opacity| format!("不透明度 {}", format_opacity(opacity))),
        )
        .collect();
    if settings.is_empty() {
        return image.path.clone();
    }
    format!("{}（{}）", image.path, settings.join("、"))
}

/// 一覧の項目の表示方法の選択肢。先頭は `[background]` の値に任せること、続いて [`BackgroundFit::ALL`]
/// の順。
const ITEM_FIT_CHOICES: [Option<BackgroundFit>; BackgroundFit::ALL.len() + 1] = {
    let mut choices = [None; BackgroundFit::ALL.len() + 1];
    let mut index = 0;
    while index < BackgroundFit::ALL.len() {
        choices[index + 1] = Some(BackgroundFit::ALL[index]);
        index += 1;
    }
    choices
};

/// 項目の不透明度の「既定」と「指定」。「指定」なら `true`。
const ITEM_OPACITY_CHOICES: [bool; 2] = [false, true];

/// `background.image_opacity` の入力欄。
pub(super) const BACKGROUND_OPACITY_KIND: NumberKind<f32> = NumberKind {
    parse: |text| config::parse_input("background.image_opacity", text),
    format: format_opacity,
    step: step_opacity,
    start: |config| config.background.image_opacity,
    message: "不透明度は、0 から 1 までの数値を入力してください。",
};

/// 背景画像の一覧で選んでいる項目の不透明度の入力欄。読めなければ、既定の不透明度から動かす
/// （背景のページの [`Page::step_start`] では、`background.image_opacity` を編集中の既定の不透明度に
/// 差し替えてある）。
pub(super) const BACKGROUND_ITEM_OPACITY_KIND: NumberKind<f32> = NumberKind {
    parse: |text| config::parse_input("background.image_opacity", text),
    format: format_opacity,
    step: step_opacity,
    start: |config| config.background.image_opacity,
    message: "選択した項目の不透明度は、0 から 1 までの数値を入力してください。",
};

/// 背景のページ。上に既定の値の枠、下に背景画像の一覧と選択した項目の値の枠。
pub(super) struct BackgroundPage {
    controls: Vec<HWND>,
    /// 背景画像の一覧。
    background_images: ListBox,
    /// 背景画像の一覧の「削除」のボタン。
    background_remove: HWND,
    /// 既定の不透明度と表示方法。
    background_opacity: NumberField<f32>,
    background_fit: ChoiceBox<BackgroundFit>,
    /// 一覧で選んでいる項目の表示方法。
    background_item_fit: ChoiceBox<Option<BackgroundFit>>,
    /// 一覧で選んでいる項目の不透明度の「既定」と「指定」のラジオボタンと、「指定」の値。
    background_item_opacity_choice: RadioGroup<bool>,
    background_item_opacity: NumberField<f32>,
    /// 編集中の背景画像の一覧。
    images: RefCell<Vec<BackgroundImage>>,
}

impl BackgroundPage {
    /// ページのコントロールを作る。
    pub(super) fn create(b: &mut Builder) -> windows::core::Result<Self> {
        let mut controls = Vec::new();
        let inner_left = PAGE_LEFT + GROUP_PADDING;
        let inner_width = PAGE_WIDTH - 2 * GROUP_PADDING;
        let background_control_left = inner_left + BACKGROUND_LABEL_WIDTH;
        let background_control_width = inner_left + inner_width - background_control_left;
        let background_label_rect = |top: i32| {
            [
                inner_left,
                top + LABEL_OFFSET,
                BACKGROUND_LABEL_WIDTH,
                ROW_HEIGHT,
            ]
        };
        let opacity_note_rect = |top: i32, left: i32| {
            [
                left + NUMBER_WIDTH + BUTTON_GAP,
                top + LABEL_OFFSET,
                inner_left + inner_width - left - NUMBER_WIDTH - BUTTON_GAP,
                ROW_HEIGHT,
            ]
        };

        let defaults_top = row(0);
        let fit_row = defaults_top + GROUP_CAPTION_HEIGHT;
        let opacity_row = fit_row + ROW_HEIGHT + BUTTON_GAP;
        let defaults_bottom = opacity_row + ROW_HEIGHT + GROUP_PADDING;
        controls.push(b.group(
            "既定",
            [
                PAGE_LEFT,
                defaults_top,
                PAGE_WIDTH,
                defaults_bottom - defaults_top,
            ],
        )?);
        controls.push(b.label("表示方法(&T):", background_label_rect(fit_row))?);
        let background_fit = ChoiceBox::create(
            b,
            ID_BACKGROUND_FIT,
            [
                background_control_left,
                fit_row,
                background_control_width,
                ROW_HEIGHT,
            ],
            &BackgroundFit::ALL,
            BACKGROUND_FIT_LABELS,
        )?;
        controls.push(background_fit.hwnd());
        controls.push(b.label("不透明度(&Y):", background_label_rect(opacity_row))?);
        let background_opacity = NumberField::create(
            b,
            ID_BACKGROUND_OPACITY,
            ID_BACKGROUND_OPACITY_SPIN,
            [
                background_control_left,
                opacity_row,
                NUMBER_WIDTH,
                ROW_HEIGHT,
            ],
            BACKGROUND_OPACITY_KIND,
        )?;
        controls.extend(background_opacity.controls());
        controls.push(b.label(
            "0（透明）〜1（不透明）",
            opacity_note_rect(opacity_row, background_control_left),
        )?);

        let images_top = defaults_bottom + BUTTON_GAP;
        let list_top = images_top + GROUP_CAPTION_HEIGHT;
        let item_fit_row = list_top + BACKGROUND_LIST_HEIGHT + BUTTON_GAP;
        let item_opacity_row = item_fit_row + ROW_HEIGHT + BUTTON_GAP;
        let custom_opacity_row = item_opacity_row + ROW_HEIGHT;
        let images_bottom = custom_opacity_row + ROW_HEIGHT + GROUP_PADDING;
        controls.push(b.group(
            "背景画像の一覧(&I)",
            [
                PAGE_LEFT,
                images_top,
                PAGE_WIDTH,
                images_bottom - images_top,
            ],
        )?);
        let list_width = inner_width - BUTTON_GAP - BACKGROUND_BUTTON_WIDTH;
        let background_images = b.list(
            ID_BACKGROUND_IMAGES,
            [inner_left, list_top, list_width, BACKGROUND_LIST_HEIGHT],
            &[],
        )?;
        controls.push(background_images.hwnd());
        let image_button_rect = |index: i32| {
            [
                inner_left + list_width + BUTTON_GAP,
                list_top + index * (BUTTON_HEIGHT + BUTTON_GAP),
                BACKGROUND_BUTTON_WIDTH,
                BUTTON_HEIGHT,
            ]
        };
        controls.push(b.button(
            ID_BACKGROUND_ADD_FILES,
            "ファイルを追加(&L)…",
            image_button_rect(0),
        )?);
        controls.push(b.button(
            ID_BACKGROUND_ADD_FOLDER,
            "フォルダを追加(&O)…",
            image_button_rect(1),
        )?);
        let background_remove = b.button(ID_BACKGROUND_REMOVE, "削除(&D)", image_button_rect(2))?;
        controls.push(background_remove);
        controls.push(b.label(
            "選択した項目の表示方法(&N):",
            background_label_rect(item_fit_row),
        )?);
        // 先頭の「既定（…）」は、既定の表示方法を入れるときに書き直す。
        let background_item_fit = ChoiceBox::create(
            b,
            ID_BACKGROUND_ITEM_FIT,
            [
                background_control_left,
                item_fit_row,
                background_control_width,
                ROW_HEIGHT,
            ],
            &ITEM_FIT_CHOICES,
            std::iter::once("").chain(BACKGROUND_FIT_LABELS),
        )?;
        controls.push(background_item_fit.hwnd());
        controls.push(b.label(
            "選択した項目の不透明度(&H):",
            background_label_rect(item_opacity_row),
        )?);
        // 「既定」の文字は、既定の不透明度を入れるときに書き直す。「指定」の右に入力欄を置く。
        let background_item_opacity_choice = RadioGroup::create(
            b,
            ID_BACKGROUND_ITEM_OPACITY_CHOICE,
            &ITEM_OPACITY_CHOICES,
            &["", "指定"],
            |index| {
                if index == 0 {
                    [
                        background_control_left,
                        item_opacity_row,
                        background_control_width,
                        ROW_HEIGHT,
                    ]
                } else {
                    [
                        background_control_left,
                        custom_opacity_row,
                        CUSTOM_OPACITY_WIDTH,
                        ROW_HEIGHT,
                    ]
                }
            },
        )?;
        controls.extend_from_slice(background_item_opacity_choice.buttons());
        let custom_opacity_left = background_control_left + CUSTOM_OPACITY_WIDTH;
        let background_item_opacity = NumberField::create(
            b,
            ID_BACKGROUND_ITEM_OPACITY,
            ID_BACKGROUND_ITEM_OPACITY_SPIN,
            [
                custom_opacity_left,
                custom_opacity_row,
                NUMBER_WIDTH,
                ROW_HEIGHT,
            ],
            BACKGROUND_ITEM_OPACITY_KIND,
        )?;
        controls.extend(background_item_opacity.controls());
        controls.push(b.label(
            "0〜1",
            opacity_note_rect(custom_opacity_row, custom_opacity_left),
        )?);
        controls.push(b.label(
            "新しいタブを開くたびに、一覧からランダムに 1 枚選ぶ（フォルダはその中の画像から選ぶ）。画像は、不透明度に応じて端末の背景色を重ねて薄くする。",
            [
                PAGE_LEFT,
                images_bottom + BUTTON_GAP,
                PAGE_WIDTH,
                2 * ROW_HEIGHT,
            ],
        )?);
        Ok(Self {
            controls,
            background_images,
            background_remove,
            background_opacity,
            background_fit,
            background_item_fit,
            background_item_opacity_choice,
            background_item_opacity,
            images: RefCell::new(Vec::new()),
        })
    }

    /// 背景画像の一覧を作り直し、`selected` 番目（一覧より後ろなら最後）を選ぶ。`None` なら選ばない。
    fn refresh_background_images(&self, selected: Option<usize>) {
        let list = self.background_images;
        let images = self.images.borrow();
        list.reset_with(images.iter().map(background_row_text));
        if let Some(selected) = selected
            && !images.is_empty()
        {
            list.select(selected.min(images.len() - 1));
        }
        drop(images);
        self.update_background_buttons();
    }

    /// 背景画像の一覧で選んでいる位置。
    fn selected_background_image(&self) -> Option<usize> {
        self.background_images
            .selected()
            .filter(|&index| index < self.images.borrow().len())
    }

    /// 背景画像の一覧で選んでいるときだけ「削除」と項目の表示方法・不透明度を触れるようにし、表示方法と
    /// 不透明度に選んでいる項目の値を見せる。不透明度の入力欄は「指定」のときだけ触れる。
    fn update_background_buttons(&self) {
        let selected = self
            .selected_background_image()
            .and_then(|index| self.images.borrow().get(index).cloned());
        let buttons = self.background_item_opacity_choice.buttons();
        for &control in [self.background_remove, self.background_item_fit.hwnd()]
            .iter()
            .chain(buttons)
        {
            // SAFETY: control はダイアログの子の有効なウィンドウ。
            unsafe { _ = EnableWindow(control, selected.is_some()) };
        }
        match &selected {
            Some(image) => self.background_item_fit.fill(image.fit),
            None => self.background_item_fit.clear(),
        }
        let opacity = selected.as_ref().map(|image| image.image_opacity);
        let custom = opacity.flatten();
        match opacity {
            Some(opacity) => self.background_item_opacity_choice.fill(opacity.is_some()),
            None => self.background_item_opacity_choice.clear(),
        }
        self.enable_item_opacity(custom.is_some());
        // 値が変わったときだけ書く（書くと EN_CHANGE が届いて、端末への反映の予約が掛かる）。借用は
        // 持っていない。
        self.background_item_opacity.fill_if_changed(custom);
    }

    /// 項目の不透明度の入力欄と上下の矢印を、`enabled` なら触れるようにする。
    fn enable_item_opacity(&self, enabled: bool) {
        for control in self.background_item_opacity.controls() {
            // SAFETY: control はダイアログの子の有効なウィンドウ。
            unsafe { _ = EnableWindow(control, enabled) };
        }
    }

    /// 項目の不透明度で「指定」を選んでいるか。
    fn custom_item_opacity(&self) -> bool {
        self.background_item_opacity_choice.read() == Some(true)
    }

    /// 編集中の既定の表示方法。
    fn default_fit(&self, ctx: &dyn DialogContext) -> BackgroundFit {
        self.background_fit
            .read()
            .unwrap_or(ctx.base().background.fit)
    }

    /// 編集中の既定の不透明度。読めなければ `None`。
    fn default_opacity(&self) -> Option<f32> {
        self.background_opacity.read().ok()
    }

    /// 項目の表示方法の先頭の選択肢と、不透明度の「既定」のラジオボタンに、編集中の既定の値を添える。
    /// 既定の不透明度が読めなければ、ラジオボタンは前の添え書きのままにする。
    fn refresh_background_defaults(&self, ctx: &dyn DialogContext) {
        let fit = default_choice_label(fit_label(self.default_fit(ctx)));
        self.background_item_fit.relabel(None, &fit);
        if let Some(opacity) = self.default_opacity()
            && let Some(button) = self.background_item_opacity_choice.button(false)
        {
            set_text(button, &default_choice_label(&format_opacity(opacity)));
        }
    }

    /// 背景画像の一覧の `index` 番目の行を、編集中の項目に合わせて書き直す（選んだままにする）。
    fn refresh_background_row(&self, index: usize) {
        let Some(text) = self.images.borrow().get(index).map(background_row_text) else {
            return;
        };
        self.background_images.replace_row(index, &text);
    }

    /// 一覧で選んでいる項目の表示方法のコンボボックスが変わった。
    fn on_background_item_fit(&self, ctx: &dyn DialogContext) {
        let Some(index) = self.selected_background_image() else {
            return;
        };
        let Some(fit) = self.background_item_fit.read() else {
            return;
        };
        self.images.borrow_mut()[index].fit = fit;
        self.refresh_background_row(index);
        ctx.preview();
    }

    /// 一覧で選んでいる項目の不透明度の入力欄が変わった。「指定」で読めれば項目に入れる。端末への反映は
    /// 少し待つ（読めなければ、反映するときにエラーを見せる）。
    fn on_background_item_opacity(&self, ctx: &dyn DialogContext) {
        if let Some(index) = self.selected_background_image()
            && self.custom_item_opacity()
            && let Ok(opacity) = self.background_item_opacity.read()
        {
            self.set_item_opacity(index, Some(opacity));
        }
        ctx.schedule_preview();
    }

    /// 一覧で選んでいる項目の不透明度の「既定」か「指定」が選ばれた。「指定」で入力欄が読めなければ、
    /// 既定の不透明度を入れる。
    fn on_background_item_opacity_choice(&self, ctx: &dyn DialogContext) {
        let Some(index) = self.selected_background_image() else {
            return;
        };
        let custom = self.custom_item_opacity();
        self.enable_item_opacity(custom);
        if custom {
            let opacity = self
                .background_item_opacity
                .read()
                .ok()
                .or_else(|| self.default_opacity())
                .unwrap_or(ctx.base().background.image_opacity);
            // 借用は持っていない（入力欄の EN_CHANGE はこの中で届く）。
            self.background_item_opacity.fill(opacity);
            self.set_item_opacity(index, Some(opacity));
        } else {
            self.background_item_opacity.clear();
            self.set_item_opacity(index, None);
        }
        ctx.preview();
    }

    /// 一覧の `index` 番目の項目の不透明度を `opacity` にし、変わったら一覧の行を書き直す。
    fn set_item_opacity(&self, index: usize, opacity: Option<f32>) {
        let changed = {
            let mut images = self.images.borrow_mut();
            let changed = images[index].image_opacity != opacity;
            images[index].image_opacity = opacity;
            changed
        };
        if changed {
            self.refresh_background_row(index);
        }
    }

    /// 背景画像の一覧のボタン `id` が押された。
    fn on_background_button(&self, ctx: &dyn DialogContext, id: i32) {
        let selected = match id {
            ID_BACKGROUND_ADD_FILES => {
                let filters = image_filters();
                let chosen = match file_dialog::open_files(ctx.hwnd(), "背景画像を追加", &filters)
                {
                    Ok(chosen) => chosen,
                    Err(e) => {
                        tracing::warn!("failed to choose background images: {e}");
                        return;
                    }
                };
                self.add_background_images(chosen)
            }
            ID_BACKGROUND_ADD_FOLDER => {
                let chosen = match file_dialog::open_folder(ctx.hwnd(), "背景画像のフォルダを追加")
                {
                    Ok(chosen) => chosen,
                    Err(e) => {
                        tracing::warn!("failed to choose a background image folder: {e}");
                        return;
                    }
                };
                self.add_background_images(chosen.into_iter().collect())
            }
            ID_BACKGROUND_REMOVE => {
                let Some(index) = self.selected_background_image() else {
                    return;
                };
                self.images.borrow_mut().remove(index);
                Some(index)
            }
            _ => return,
        };
        let Some(selected) = selected else {
            return;
        };
        self.refresh_background_images(Some(selected));
        ctx.preview();
        // SAFETY: 一覧はダイアログの子の有効なウィンドウ。押したボタンが押せなくなることがあるので、
        // 一覧へ戻す。
        unsafe { _ = SetFocus(Some(self.background_images.hwnd())) };
    }

    /// 背景画像の一覧の最後に `paths` を足す（同じものがあれば足さない）。足した最後の位置を返し、
    /// 何も足さなければ `None`。
    fn add_background_images(&self, paths: Vec<PathBuf>) -> Option<usize> {
        let mut images = self.images.borrow_mut();
        let mut added = None;
        for path in paths {
            let text = path.to_string_lossy().into_owned();
            if images
                .iter()
                .any(|image| image.path.eq_ignore_ascii_case(&text))
            {
                continue;
            }
            images.push(BackgroundImage::new(text));
            added = Some(images.len() - 1);
        }
        added
    }
}

impl Page for BackgroundPage {
    fn controls(&self) -> &[HWND] {
        &self.controls
    }

    fn number_fields(&self) -> Vec<&dyn NumberInput> {
        vec![&self.background_opacity, &self.background_item_opacity]
    }

    /// 既定の不透明度は、入力欄が読めればその値にする（項目の不透明度が読めないときは、既定の
    /// 不透明度から動かす）。
    fn step_start(&self, ctx: &dyn DialogContext) -> Config {
        let mut start = ctx.base().clone();
        if let Some(opacity) = self.default_opacity() {
            start.background.image_opacity = opacity;
        }
        start
    }

    fn fill(&self, ctx: &dyn DialogContext, config: &Config) {
        self.background_opacity
            .fill(config.background.image_opacity);
        self.background_fit.fill(config.background.fit);
        self.refresh_background_defaults(ctx);
        *self.images.borrow_mut() = config.background.images.clone();
        self.refresh_background_images(None);
    }

    fn read(
        &self,
        _ctx: &dyn DialogContext,
        config: &mut Config,
    ) -> Result<(), (&'static str, HWND)> {
        // 選んでいる項目の値は、読めたときに項目へ入れている。ここでは読めるかだけを見る。
        if self.selected_background_image().is_some() && self.custom_item_opacity() {
            self.background_item_opacity.read()?;
        }
        config.background.images = self.images.borrow().clone();
        config.background.image_opacity = self.background_opacity.read()?;
        if let Some(fit) = self.background_fit.read() {
            config.background.fit = fit;
        }
        Ok(())
    }

    fn on_command(&self, ctx: &dyn DialogContext, id: i32, code: u32) -> bool {
        match (id, code) {
            (ID_BACKGROUND_IMAGES, LBN_SELCHANGE) => self.update_background_buttons(),
            (
                ID_BACKGROUND_ADD_FILES | ID_BACKGROUND_ADD_FOLDER | ID_BACKGROUND_REMOVE,
                BN_CLICKED,
            ) => self.on_background_button(ctx, id),
            (ID_BACKGROUND_FIT, CBN_SELCHANGE) => {
                self.refresh_background_defaults(ctx);
                ctx.preview();
            }
            (ID_BACKGROUND_OPACITY, EN_CHANGE) => {
                self.refresh_background_defaults(ctx);
                ctx.schedule_preview();
            }
            (ID_BACKGROUND_ITEM_FIT, CBN_SELCHANGE) => self.on_background_item_fit(ctx),
            (ID_BACKGROUND_ITEM_OPACITY, EN_CHANGE) => self.on_background_item_opacity(ctx),
            (id, BN_CLICKED) if self.background_item_opacity_choice.owns(id) => {
                self.on_background_item_opacity_choice(ctx);
            }
            // 数の入力欄は、打ち終わるのを待って反映する。
            (id, EN_CHANGE) if self.number_fields().iter().any(|field| field.owns(id)) => {
                ctx.schedule_preview();
            }
            _ => return false,
        }
        true
    }
}

/// 画像の不透明度を、矢印で一つ動かした値。[`OPACITY_STEP`] の倍数へ動かし、範囲の端で止める。
fn step_opacity(current: f32, delta: i32) -> f32 {
    // 0.35 のように刻みの倍数を書いた値が、f32 の誤差で倍数の手前に見えないようにする。
    const TOLERANCE: f32 = 1e-3;
    let steps = current / OPACITY_STEP;
    let next = if delta > 0 {
        (steps + TOLERANCE).floor() + 1.0
    } else {
        (steps - TOLERANCE).ceil() - 1.0
    };
    // 刻みの倍数を f32 で掛けた端数（0.35000002 など）を出さないように、百分の一に丸める。
    let next = (next * OPACITY_STEP * 100.0).round() / 100.0;
    next.clamp(
        *config::IMAGE_OPACITY_RANGE.start(),
        *config::IMAGE_OPACITY_RANGE.end(),
    )
}

/// 入力欄に見せる、画像の不透明度。
fn format_opacity(opacity: f32) -> String {
    opacity.to_string()
}

/// 背景画像を選ぶダイアログの、ファイルの種類の絞り込み。
fn image_filters() -> Vec<file_dialog::Filter> {
    filters_for(IMAGE_EXTENSIONS.iter().copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrows_step_the_opacity_by_twentieths_within_the_range() {
        assert_eq!(step_opacity(0.2, 1), 0.25);
        assert_eq!(step_opacity(0.2, -1), 0.15);
        assert_eq!(step_opacity(0.8, 1), 0.85);
        assert_eq!(step_opacity(0.8, -1), 0.75);
        assert_eq!(step_opacity(0.35, 1), 0.4);
        assert_eq!(step_opacity(0.35, -1), 0.3);
        assert_eq!(step_opacity(0.33, 1), 0.35);
        assert_eq!(step_opacity(0.33, -1), 0.3);
        assert_eq!(step_opacity(1.0, 1), 1.0);
        assert_eq!(step_opacity(0.0, -1), 0.0);
        assert_eq!(format_opacity(step_opacity(0.8, 1)), "0.85");
        assert_eq!(format_opacity(1.0), "1");
    }

    #[test]
    fn background_rows_show_the_item_settings() {
        let mut image = BackgroundImage::new(r"C:\a.png");
        assert_eq!(background_row_text(&image), r"C:\a.png");
        image.fit = Some(BackgroundFit::Stretch);
        assert_eq!(background_row_text(&image), r"C:\a.png（拡大して表示）");
        image.image_opacity = Some(0.3);
        assert_eq!(
            background_row_text(&image),
            r"C:\a.png（拡大して表示、不透明度 0.3）"
        );
        image.fit = None;
        assert_eq!(background_row_text(&image), r"C:\a.png（不透明度 0.3）");
    }

    #[test]
    fn item_fits_follow_the_background_first() {
        assert_eq!(ITEM_FIT_CHOICES[0], None);
        for (index, fit) in BackgroundFit::ALL.into_iter().enumerate() {
            assert_eq!(ITEM_FIT_CHOICES[index + 1], Some(fit));
        }
        assert_eq!(ITEM_FIT_CHOICES.len(), BackgroundFit::ALL.len() + 1);
        assert_eq!(
            default_choice_label(fit_label(BackgroundFit::Tile)),
            "既定（並べて表示）"
        );
        assert_eq!(default_choice_label(&format_opacity(0.5)), "既定（0.5）");
    }

    #[test]
    fn the_ids_are_within_the_range_of_the_page() {
        let last_item_opacity_choice =
            ID_BACKGROUND_ITEM_OPACITY_CHOICE + ITEM_OPACITY_CHOICES.len() as i32 - 1;
        for id in [
            ID_BACKGROUND_IMAGES,
            ID_BACKGROUND_ADD_FILES,
            ID_BACKGROUND_ADD_FOLDER,
            ID_BACKGROUND_REMOVE,
            ID_BACKGROUND_OPACITY,
            ID_BACKGROUND_OPACITY_SPIN,
            ID_BACKGROUND_FIT,
            ID_BACKGROUND_ITEM_FIT,
            ID_BACKGROUND_ITEM_OPACITY,
            ID_BACKGROUND_ITEM_OPACITY_SPIN,
            ID_BACKGROUND_ITEM_OPACITY_CHOICE,
            last_item_opacity_choice,
        ] {
            assert!(ID_RANGE.contains(&id), "{id}");
        }
    }

    #[test]
    fn every_fit_has_a_label() {
        assert_eq!(BACKGROUND_FIT_LABELS.len(), BackgroundFit::ALL.len());
        let filters = image_filters();
        assert_eq!(filters[0].spec, "*.png;*.jpg;*.jpeg;*.bmp;*.gif;*.webp");
    }
}
