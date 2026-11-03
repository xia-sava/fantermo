//! 設定ダイアログの動作のページ。貼り付け前の確認、大きなテキストの確認、クリップボードの読み取りと書き込み。

use fantermo_core::config::{self, ClipboardRead, ClipboardWrite, Config, PasteConfirm};
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{BN_CLICKED, EN_CHANGE},
};

use super::{
    BUTTON_GAP, CONTROL_LEFT, CONTROL_WIDTH, DialogContext, LABEL_OFFSET, MARGIN, NUMBER_WIDTH,
    PAGE_LEFT, PAGE_WIDTH, Page, ROW_HEIGHT, label_rect, row,
    widgets::{
        Builder, NumberField, NumberInput, NumberKind, RadioGroup, format_integer, step_integer,
    },
};

/// このページのコントロールの ID の範囲。範囲がページの間で重ならないことと、このページの ID が範囲に
/// 入ることを単体のテストで見る。
#[cfg(test)]
pub(super) const ID_RANGE: std::ops::RangeInclusive<i32> = 300..=329;
/// `paste.confirm` のラジオボタン。[`PasteConfirm::ALL`] の順に ID が続く。
const ID_PASTE_CONFIRM: i32 = 300;
/// `clipboard.read` のラジオボタン。[`ClipboardRead::ALL`] の順に ID が続く。
const ID_CLIPBOARD_READ: i32 = 310;
/// `clipboard.write` のラジオボタン。[`ClipboardWrite::ALL`] の順に ID が続く。
const ID_CLIPBOARD_WRITE: i32 = 315;
const ID_PASTE_CONFIRM_SIZE: i32 = 320;
const ID_PASTE_CONFIRM_SIZE_SPIN: i32 = 321;

/// `paste.confirm` の選択肢の表示。[`PasteConfirm::ALL`] の順。
const PASTE_CONFIRM_LABELS: [&str; PasteConfirm::ALL.len()] = [
    "コマンドとして実行される可能性があるとき",
    "改行を含むとき",
    "確認しない",
];

/// `clipboard.read` の選択肢の表示。[`ClipboardRead::ALL`] の順。
const CLIPBOARD_READ_LABELS: [&str; ClipboardRead::ALL.len()] =
    ["確認する", "常に許可する", "常に拒否する"];

/// `clipboard.write` の選択肢の表示。[`ClipboardWrite::ALL`] の順。
const CLIPBOARD_WRITE_LABELS: [&str; ClipboardWrite::ALL.len()] = ["許可する", "拒否する"];

/// `paste.confirm_size` の入力欄。
pub(super) const PASTE_CONFIRM_SIZE_KIND: NumberKind<u32> = NumberKind {
    parse: |text| config::parse_input("paste.confirm_size", text),
    format: format_integer,
    step: |current, delta| step_integer(current, delta, &config::PASTE_CONFIRM_SIZE_RANGE),
    start: |config| config.paste.confirm_size,
    message: "大きなテキストの確認の大きさは、0 から 1048576 までの整数を入力してください。",
};

/// 動作のページ。貼り付け前の確認、大きなテキストの確認、クリップボードの読み取りと書き込み。
pub(super) struct BehaviorPage {
    controls: Vec<HWND>,
    paste_confirm: RadioGroup<PasteConfirm>,
    paste_confirm_size: NumberField<u32>,
    clipboard_read: RadioGroup<ClipboardRead>,
    clipboard_write: RadioGroup<ClipboardWrite>,
}

impl BehaviorPage {
    /// ページのコントロールを作る。
    pub(super) fn create(b: &mut Builder) -> windows::core::Result<Self> {
        let mut controls = Vec::new();
        controls.push(b.label("貼り付け前の確認(&P):", label_rect(row(0)))?);
        let paste_confirm = RadioGroup::create(
            b,
            ID_PASTE_CONFIRM,
            &PasteConfirm::ALL,
            &PASTE_CONFIRM_LABELS,
            |index| {
                [
                    CONTROL_LEFT,
                    row(0) + index * ROW_HEIGHT,
                    CONTROL_WIDTH,
                    ROW_HEIGHT,
                ]
            },
        )?;
        controls.extend_from_slice(paste_confirm.buttons());
        controls.push(b.label(
            "改行を含む文字列を貼り付ける前に、内容を編集できるダイアログで確認する。「コマンドとして実行される可能性があるとき」は、アプリが bracketed paste を有効にしていないときなど。",
            [
                PAGE_LEFT,
                row(0) + 3 * ROW_HEIGHT + MARGIN,
                PAGE_WIDTH,
                3 * ROW_HEIGHT,
            ],
        )?);
        let paste_size_row = row(0) + 6 * ROW_HEIGHT + 2 * MARGIN;
        controls.push(b.label("大きなテキストの確認(&Z):", label_rect(paste_size_row))?);
        let paste_confirm_size = NumberField::create(
            b,
            ID_PASTE_CONFIRM_SIZE,
            ID_PASTE_CONFIRM_SIZE_SPIN,
            [CONTROL_LEFT, paste_size_row, NUMBER_WIDTH, ROW_HEIGHT],
            PASTE_CONFIRM_SIZE_KIND,
        )?;
        controls.extend(paste_confirm_size.controls());
        controls.push(b.label(
            "KiB 以上は常に確認する（0 で無効）",
            [
                CONTROL_LEFT + NUMBER_WIDTH + BUTTON_GAP,
                paste_size_row + LABEL_OFFSET,
                CONTROL_WIDTH - NUMBER_WIDTH - BUTTON_GAP,
                ROW_HEIGHT,
            ],
        )?);
        let clipboard_row = paste_size_row + ROW_HEIGHT + MARGIN;
        controls.push(b.label("クリップボードの読み取り(&L):", label_rect(clipboard_row))?);
        let clipboard_read = RadioGroup::create(
            b,
            ID_CLIPBOARD_READ,
            &ClipboardRead::ALL,
            &CLIPBOARD_READ_LABELS,
            |index| {
                [
                    CONTROL_LEFT,
                    clipboard_row + index * ROW_HEIGHT,
                    CONTROL_WIDTH,
                    ROW_HEIGHT,
                ]
            },
        )?;
        controls.extend_from_slice(clipboard_read.buttons());
        controls.push(b.label(
            "アプリがクリップボードの内容を読み取ろうとしたとき（OSC 52）。",
            [
                PAGE_LEFT,
                clipboard_row + 3 * ROW_HEIGHT + MARGIN,
                PAGE_WIDTH,
                ROW_HEIGHT,
            ],
        )?);
        let write_row = clipboard_row + 4 * ROW_HEIGHT + 2 * MARGIN;
        controls.push(b.label("クリップボードの書き込み(&W):", label_rect(write_row))?);
        let clipboard_write = RadioGroup::create(
            b,
            ID_CLIPBOARD_WRITE,
            &ClipboardWrite::ALL,
            &CLIPBOARD_WRITE_LABELS,
            |index| {
                [
                    CONTROL_LEFT,
                    write_row + index * ROW_HEIGHT,
                    CONTROL_WIDTH,
                    ROW_HEIGHT,
                ]
            },
        )?;
        controls.extend_from_slice(clipboard_write.buttons());
        controls.push(b.label(
            "アプリがクリップボードへ書き込もうとしたとき（OSC 52 など）。",
            [
                PAGE_LEFT,
                write_row + 2 * ROW_HEIGHT + MARGIN,
                PAGE_WIDTH,
                ROW_HEIGHT,
            ],
        )?);
        Ok(Self {
            controls,
            paste_confirm,
            paste_confirm_size,
            clipboard_read,
            clipboard_write,
        })
    }
}

impl Page for BehaviorPage {
    fn controls(&self) -> &[HWND] {
        &self.controls
    }

    fn number_fields(&self) -> Vec<&dyn NumberInput> {
        vec![&self.paste_confirm_size]
    }

    fn fill(&self, _ctx: &dyn DialogContext, config: &Config) {
        self.paste_confirm.fill(config.paste.confirm);
        self.paste_confirm_size.fill(config.paste.confirm_size);
        self.clipboard_read.fill(config.clipboard.read);
        self.clipboard_write.fill(config.clipboard.write);
    }

    fn read(
        &self,
        _ctx: &dyn DialogContext,
        config: &mut Config,
    ) -> Result<(), (&'static str, HWND)> {
        if let Some(confirm) = self.paste_confirm.read() {
            config.paste.confirm = confirm;
        }
        config.paste.confirm_size = self.paste_confirm_size.read()?;
        if let Some(read) = self.clipboard_read.read() {
            config.clipboard.read = read;
        }
        if let Some(write) = self.clipboard_write.read() {
            config.clipboard.write = write;
        }
        Ok(())
    }

    fn on_command(&self, ctx: &dyn DialogContext, id: i32, code: u32) -> bool {
        match (id, code) {
            // ラジオボタンは、押したらすぐ反映する。
            (id, BN_CLICKED)
                if self.paste_confirm.owns(id)
                    || self.clipboard_read.owns(id)
                    || self.clipboard_write.owns(id) =>
            {
                ctx.preview();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ids_are_within_the_range_of_the_page() {
        let last_paste_confirm = ID_PASTE_CONFIRM + PasteConfirm::ALL.len() as i32 - 1;
        let last_clipboard_read = ID_CLIPBOARD_READ + ClipboardRead::ALL.len() as i32 - 1;
        let last_clipboard_write = ID_CLIPBOARD_WRITE + ClipboardWrite::ALL.len() as i32 - 1;
        for id in [
            ID_PASTE_CONFIRM,
            last_paste_confirm,
            ID_CLIPBOARD_READ,
            last_clipboard_read,
            ID_CLIPBOARD_WRITE,
            last_clipboard_write,
            ID_PASTE_CONFIRM_SIZE,
            ID_PASTE_CONFIRM_SIZE_SPIN,
        ] {
            assert!(ID_RANGE.contains(&id), "{id}");
        }
    }
}
